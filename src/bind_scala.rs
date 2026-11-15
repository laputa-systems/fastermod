//! Scala binding analysis via tree-sitter CST traversal.
//!
//! Scopes: compilation unit → template bodies (class/trait/object/
//! enum) → function bodies → blocks. Scala `val`/`var` are
//! block-scoped; `for` enumerators and lambda parameters bind in
//! their loop/function scopes. Nested defs capture enclosing scope,
//! so scopes resolve outward without a barrier.
//!
//! Two namespaces per scope: `vars` (vals, vars, parameters, type
//! names, class parameters) and `methods` (defs). Call callees
//! resolve methods-then-vars; bare identifiers resolve
//! vars-then-methods (eta-expansion). `field_expression` resolves the
//! field only for `this` receivers; `obj.m`/`Type.m` on other
//! receivers records the member unresolved, blocking member renames.
//!
//! Pattern bindings (`val (a, b) = ...`), `given`/`using` clauses,
//! `match`/`case` binders, implicits, and macro expansion are only
//! partially modeled — `unresolved_is_exact` is false.
//! All bindings are file-local.

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
    "class_definition",
    "trait_definition",
    "object_definition",
    "enum_definition",
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

/// Bind identifier leaves inside a pattern (val/var/for/case patterns).
fn bind_pattern(st: &mut State, node: Node) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        if child.kind() == "identifier" {
            bind_node(st, child, Ns::Var);
        } else if matches!(
            child.kind(),
            "pattern" | "tuple_pattern" | "case_pattern" | "bind_pattern"
        ) {
            bind_pattern(st, child);
        }
    }
}

fn walk(node: Node, st: &mut State) {
    let kind = node.kind();
    if TYPE_KINDS.contains(&kind) {
        // name binds as a type var in the enclosing scope; everything
        // else (class_parameters, template_body) lives in a member
        // scope so constructor params are visible to methods.
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if !child.is_named() {
                continue;
            }
            if child.kind() == "identifier"
                && node
                    .children(&mut node.walk())
                    .find(|c| c.is_named())
                    .is_some_and(|c| c.id() == child.id())
            {
                bind_node(st, child, Ns::Var);
            }
        }
        push(st, ScopeKind::Member);
        let name_id = node
            .children(&mut node.walk())
            .find(|c| c.is_named() && c.kind() == "identifier")
            .map(|n| n.id());
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.is_named() && Some(child.id()) != name_id {
                walk(child, st);
            }
        }
        st.scopes.pop();
        return;
    }
    match kind {
        "template_body" => {
            push(st, ScopeKind::Member);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "function_definition" => {
            if let Some(name) = node.child_by_field_name("name") {
                bind_node(st, name, Ns::Method);
            }
            push(st, ScopeKind::Fn);
            if let Some(params) = node.child_by_field_name("parameters") {
                bind_params(st, params);
            }
            // class_parameters bind in the enclosing member scope —
            // handled by caller walking them before this arm? They are
            // siblings, not children of function_definition.
            walk_children_except_fields(st, node, &["name", "parameters"]);
            st.scopes.pop();
            return;
        }
        "parameters" | "class_parameters" => {
            let mut cursor = node.walk();
            for p in node.children(&mut cursor) {
                if p.is_named() {
                    walk(p, st);
                }
            }
            return;
        }
        "parameter" | "class_parameter" => {
            if let Some(name) = node.child_by_field_name("name") {
                bind_node(st, name, Ns::Var);
                walk_children_except_fields(st, node, &["name"]);
            } else {
                walk_children(node, st);
            }
            return;
        }
        "val_definition" | "var_definition" => {
            if let Some(pat) = node.child_by_field_name("pattern") {
                if pat.kind() == "identifier" {
                    bind_node(st, pat, Ns::Var);
                } else {
                    bind_pattern(st, pat);
                }
            }
            walk_children_except_fields(st, node, &["pattern"]);
            return;
        }
        "block" => {
            push(st, ScopeKind::Block);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "for_expression" => {
            push(st, ScopeKind::Block);
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if !child.is_named() {
                    continue;
                }
                if child.kind() == "enumerators" {
                    // enumerator (identifier) generator body
                    let mut c2 = child.walk();
                    for e in child.children(&mut c2) {
                        if !e.is_named() {
                            continue;
                        }
                        if e.kind() == "enumerator" {
                            // first identifier child is the loop var
                            if let Some(v) = e
                                .children(&mut e.walk())
                                .find(|c| c.is_named() && c.kind() == "identifier")
                            {
                                bind_node(st, v, Ns::Var);
                            }
                            walk_children_except_first_ident(st, e);
                        } else {
                            walk(e, st);
                        }
                    }
                } else {
                    walk(child, st);
                }
            }
            st.scopes.pop();
            return;
        }
        "lambda_expression" => {
            push(st, ScopeKind::Block);
            if let Some(params) = node.child_by_field_name("parameters") {
                bind_params(st, params);
            }
            walk_children_except_fields(st, node, &["parameters"]);
            st.scopes.pop();
            return;
        }
        "call_expression" => {
            walk_call(st, node);
            return;
        }
        "field_expression" => {
            walk_field(st, node);
            return;
        }
        "identifier" => {
            let range = node.start_byte()..node.end_byte();
            let name = text(st.contents, node);
            let t = lookup(st, name, Ns::Var).or_else(|| lookup(st, name, Ns::Method));
            record_ref_target(st, range, t);
            return;
        }
        "type_identifier" | "operator_identifier" => {
            if kind == "type_identifier" {
                record_ref(st, text(st.contents, node), node.start_byte()..node.end_byte(), Ns::Var);
            }
            return;
        }
        k if k.starts_with("import") || k == "package_clause" || k == "package_definition" => {
            return;
        }
        _ => {}
    }
    walk_children(node, st);
}

fn walk_children_except_first_ident(st: &mut State, node: Node) {
    let mut cursor = node.walk();
    let mut skipped = false;
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        if !skipped && child.kind() == "identifier" {
            skipped = true;
            continue;
        }
        walk(child, st);
    }
}

fn bind_params(st: &mut State, params: Node) {
    let mut cursor = params.walk();
    for p in params.children(&mut cursor) {
        if p.is_named() {
            walk(p, st);
        }
    }
}

/// `f(args)`: identifier callee -> methods-then-vars; field callees ->
/// member rules.
fn walk_call(st: &mut State, node: Node) {
    let mut cursor = node.walk();
    let mut first = true;
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        if first {
            first = false;
            match child.kind() {
                "identifier" => {
                    let range = child.start_byte()..child.end_byte();
                    let name = text(st.contents, child);
                    let t = lookup(st, name, Ns::Method).or_else(|| lookup(st, name, Ns::Var));
                    record_ref_target(st, range, t);
                    continue;
                }
                "field_expression" => {
                    walk_field(st, child);
                    continue;
                }
                _ => {}
            }
        }
        walk(child, st);
    }
}

/// `value.field`: resolves only for `this` receivers.
fn walk_field(st: &mut State, node: Node) {
    let value = node.child_by_field_name("value");
    let field = node.child_by_field_name("field");
    if let Some(v) = value {
        walk(v, st);
    }
    if let Some(f) = field {
        let range = f.start_byte()..f.end_byte();
        let name = text(st.contents, f);
        let is_this = value.is_some_and(|v| {
            v.kind() == "identifier" && text(st.contents, v) == "this"
        });
        if is_this {
            let in_call = node
                .parent()
                .is_some_and(|p| p.kind() == "call_expression");
            let target = if in_call {
                lookup(st, name, Ns::Method).or_else(|| lookup(st, name, Ns::Var))
            } else {
                lookup(st, name, Ns::Var).or_else(|| lookup(st, name, Ns::Method))
            };
            record_ref_target(st, range, target);
        } else {
            record_unresolved(st, range);
        }
    }
    walk_children_except_fields(st, node, &["value", "field"]);
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
