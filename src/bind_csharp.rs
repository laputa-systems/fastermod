//! C# binding analysis via tree-sitter CST traversal.
//!
//! Scopes: compilation unit → declaration list (type members) →
//! method/constructor/lambda/local-function → block. Local functions
//! are closures over enclosing locals; lambdas likewise. Two
//! namespaces per scope: `vars` (fields, locals, parameters, constants,
//! enum members, type names) and `methods` (methods, constructors are
//! not bound — their name references the type, local functions).
//!
//! Boundaries: `obj.M()` on a receiver that is not `this` records the
//! member name as unresolved (unknown type / virtual dispatch), which
//! blocks renaming that member. Extension methods, `nameof` targets on
//! unresolved receivers, pointer members, and `dynamic` dispatch are
//! not resolved — `unresolved_is_exact` is false. `using`/namespace
//! import resolution is not implemented; module-level symbols block on
//! sibling occurrences.

use std::collections::HashMap;
use std::path::Path;

use ast_grep_core::tree_sitter::LanguageExt;
use tree_sitter::{Node, Parser};

use crate::bind::{Analysis, BindingSite, RefSite};
use crate::lang::LangSpec;

#[derive(Clone, Copy, PartialEq)]
enum ScopeKind {
    Root,
    Member,
    Fn,
    Block,
}

struct Scope {
    kind: ScopeKind,
    vars: HashMap<String, usize>,
    methods: HashMap<String, usize>,
}

struct State<'a> {
    analysis: Analysis,
    scopes: Vec<Scope>,
    contents: &'a [u8],
}

fn text<'a>(contents: &'a [u8], node: Node) -> &'a str {
    node.utf8_text(contents).unwrap_or("")
}

const TYPE_KINDS: &[&str] = &[
    "class_declaration",
    "struct_declaration",
    "interface_declaration",
    "record_declaration",
    "enum_declaration",
];

const FN_KINDS: &[&str] = &[
    "method_declaration",
    "local_function_statement",
    "constructor_declaration",
    "destructor_declaration",
    "lambda_expression",
];

const BLOCK_KINDS: &[&str] = &[
    "block",
    "for_statement",
    "foreach_statement",
    "catch_clause",
    "try_statement",
    "lock_statement",
    "using_statement",
    "fixed_statement",
    "checked_statement",
    "unchecked_statement",
    "unsafe_statement",
    "switch_statement",
    "switch_expression",
    "do_statement",
    "while_statement",
    "for_each_statement",
];

#[derive(Clone, Copy, PartialEq)]
enum Ns {
    Var,
    Method,
}

pub(crate) fn analyze(
    spec: &LangSpec,
    contents: &str,
    _path: &Path,
) -> Option<Analysis> {
    let language: tree_sitter::Language = spec.lang.get_ts_language();
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    let tree = parser.parse(contents, None)?;
    let root = tree.root_node();
    if root.has_error() {
        return None;
    }
    let mut st = State {
        analysis: Analysis::default(),
        scopes: vec![Scope {
            kind: ScopeKind::Root,
            vars: HashMap::new(),
            methods: HashMap::new(),
        }],
        contents: contents.as_bytes(),
    };
    walk(root, &mut st);
    st.analysis.unresolved_is_exact = false;
    Some(st.analysis)
}

fn cur(st: &State) -> usize {
    st.scopes.len() - 1
}

fn bind(st: &mut State, name: &str, range: std::ops::Range<usize>, ns: Ns) {
    let kind = st.scopes[cur(st)].kind;
    let module_level = matches!(kind, ScopeKind::Root | ScopeKind::Member);
    let idx = st.analysis.bindings.len();
    st.analysis.bindings.push(BindingSite {
        name: range,
        text: name.to_string(),
        expansion: None,
        module_level,
        is_mod: false,
    });
    let si = cur(st);
    let map = match ns {
        Ns::Var => &mut st.scopes[si].vars,
        Ns::Method => &mut st.scopes[si].methods,
    };
    map.insert(name.to_string(), idx);
}

fn lookup(st: &State, name: &str, ns: Ns) -> Option<usize> {
    for s in st.scopes.iter().rev() {
        let map = match ns {
            Ns::Var => &s.vars,
            Ns::Method => &s.methods,
        };
        if let Some(&b) = map.get(name) {
            return Some(b);
        }
    }
    None
}

fn record_ref(st: &mut State, name: &str, range: std::ops::Range<usize>, ns: Ns) {
    let target = lookup(st, name, ns);
    st.analysis.refs.push(RefSite {
        name: range,
        target,
        expansion: None,
        remote: false,
        module_attr: None,
    });
}

fn record_ref_target(st: &mut State, range: std::ops::Range<usize>, target: Option<usize>) {
    st.analysis.refs.push(RefSite {
        name: range,
        target,
        expansion: None,
        remote: false,
        module_attr: None,
    });
}

fn record_unresolved(st: &mut State, range: std::ops::Range<usize>) {
    st.analysis.refs.push(RefSite {
        name: range,
        target: None,
        expansion: None,
        remote: false,
        module_attr: None,
    });
}

fn push(st: &mut State, kind: ScopeKind) {
    st.scopes.push(Scope {
        kind,
        vars: HashMap::new(),
        methods: HashMap::new(),
    });
}

fn bind_node(st: &mut State, node: Node, ns: Ns) {
    bind(
        st,
        text(st.contents, node),
        node.start_byte()..node.end_byte(),
        ns,
    );
}

fn walk(node: Node, st: &mut State) {
    let kind = node.kind();
    if TYPE_KINDS.contains(&kind) {
        if let Some(name) = node.child_by_field_name("name") {
            bind_node(st, name, Ns::Var);
        }
        for i in 0..node.child_count() {
            let child = node.child(i).unwrap();
            if !child.is_named() || node.field_name_for_child(i) == Some("name") {
                continue;
            }
            if child.kind() == "declaration_list" {
                push(st, ScopeKind::Member);
                walk(child, st);
                st.scopes.pop();
            } else {
                walk(child, st);
            }
        }
        return;
    }
    match kind {
        "declaration_list" => {
            push(st, ScopeKind::Member);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "delegate_declaration" => {
            if let Some(name) = node.child_by_field_name("name") {
                bind_node(st, name, Ns::Var);
            }
            walk_children_except_fields(st, node, &["name"]);
            return;
        }
        k if FN_KINDS.contains(&k) => {
            if matches!(k, "constructor_declaration" | "destructor_declaration") {
                // The ctor/dtor name is the type name: a reference, not a
                // new binding.
                if let Some(name) = node.child_by_field_name("name") {
                    record_ref(st, text(st.contents, name), name.start_byte()..name.end_byte(), Ns::Var);
                }
            } else if let Some(name) = node.child_by_field_name("name") {
                bind_node(st, name, Ns::Method);
            }
            push(st, ScopeKind::Fn);
            walk_children_except_fields(st, node, &["name"]);
            st.scopes.pop();
            return;
        }
        "variable_declarator" => {
            // `int a = 1, b = a`: bind the name, then walk value/type.
            if let Some(name) = node.child_by_field_name("name") {
                bind_node(st, name, Ns::Var);
            }
            walk_children_except_fields(st, node, &["name"]);
            return;
        }
        "parameter" | "lambda_parameter" => {
            if let Some(name) = node.child_by_field_name("name") {
                bind_node(st, name, Ns::Var);
            }
            walk_children_except_fields(st, node, &["name"]);
            return;
        }
        "implicit_parameter" => {
            // `q => ...`: the node text is the parameter name.
            bind_node(st, node, Ns::Var);
            return;
        }
        "parameter_list" => {
            walk_children(node, st);
            return;
        }
        "catch_declaration" => {
            if let Some(name) = node.child_by_field_name("name") {
                bind_node(st, name, Ns::Var);
            }
            walk_children_except_fields(st, node, &["name"]);
            return;
        }
        "property_declaration" | "indexer_declaration" | "event_declaration" => {
            if let Some(name) = node.child_by_field_name("name") {
                bind_node(st, name, Ns::Var);
            }
            walk_children_except_fields(st, node, &["name"]);
            return;
        }
        "enum_member_declaration" => {
            if let Some(name) = node.child_by_field_name("name") {
                bind_node(st, name, Ns::Var);
            }
            walk_children_except_fields(st, node, &["name"]);
            return;
        }
        "foreach_statement" | "for_each_statement" => {
            // `foreach (var x in expr)`: iterable sees outer bindings;
            // the loop variable scopes over the body only.
            let left = node.child_by_field_name("left");
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if !child.is_named() {
                    continue;
                }
                if Some(child.id()) == left.map(|n| n.id()) {
                    continue;
                }
                if child.kind() == "block" {
                    continue;
                }
                walk(child, st);
            }
            if let Some(l) = left {
                if l.kind() == "identifier" {
                    bind_node(st, l, Ns::Var);
                } else {
                    walk(l, st);
                }
            }
            if let Some(body) = node.child_by_field_name("body") {
                walk(body, st);
            }
            return;
        }
        "invocation_expression" => {
            walk_invocation(st, node);
            return;
        }
        "member_access_expression" => {
            walk_member_access(st, node, false);
            return;
        }
        "identifier" => {
            record_ref(st, text(st.contents, node), node.start_byte()..node.end_byte(), Ns::Var);
            return;
        }
        k if BLOCK_KINDS.contains(&k) => {
            push(st, ScopeKind::Block);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "using_directive" | "using_static_directive" | "extern_alias_directive"
        | "package_directive" => {
            return;
        }
        _ => {}
    }
    walk_children(node, st);
}

/// `F(args)` / `recv.M(args)`: the callee resolves in the methods
/// namespace; member-access callees go through receiver rules.
fn walk_invocation(st: &mut State, node: Node) {
    let function = node.child_by_field_name("function");
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        if Some(child.id()) == function.map(|n| n.id()) {
            match child.kind() {
                "identifier" => {
                    record_ref(st, text(st.contents, child), child.start_byte()..child.end_byte(), Ns::Method);
                }
                "member_access_expression" => {
                    walk_member_access(st, child, true);
                }
                _ => walk(child, st),
            }
        } else {
            walk(child, st);
        }
    }
}

/// `expr.name`: `this.` receivers resolve the member name (methods for
/// invocations, vars otherwise with method-group fallback); any other
/// receiver type is unknown -> unresolved blocker.
fn walk_member_access(st: &mut State, node: Node, is_invocation: bool) {
    let expr = node.child_by_field_name("expression");
    let name = node.child_by_field_name("name");
    let this_recv = expr.is_some_and(|e| {
        let t = text(st.contents, e);
        t == "this" || e.kind() == "this_expression"
    });
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        if Some(child.id()) == name.map(|n| n.id()) {
            if let Some(n) = name {
                let range = n.start_byte()..n.end_byte();
                let t = text(st.contents, n);
                if this_recv {
                    // `this.M()` -> method; `this.f` -> field, but a
                    // method group `this.M` also resolves methods.
                    let target = if is_invocation {
                        lookup(st, t, Ns::Method)
                    } else {
                        lookup(st, t, Ns::Var).or_else(|| lookup(st, t, Ns::Method))
                    };
                    record_ref_target(st, range, target);
                } else {
                    record_unresolved(st, range);
                }
            }
        } else {
            walk(child, st);
        }
    }
}

fn walk_children(node: Node, st: &mut State) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() {
            walk(child, st);
        }
    }
}

fn walk_children_except_fields(st: &mut State, node: Node, skip: &[&str]) {
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        if !child.is_named() {
            continue;
        }
        let fname = node.field_name_for_child(i);
        if fname.is_some_and(|f| skip.contains(&f)) {
            continue;
        }
        walk(child, st);
    }
}
