//! Swift binding analysis via tree-sitter CST traversal.
//!
//! Scopes: source file → type bodies (class/struct/enum/extension/
//! protocol) → function/init → statements blocks (if/for/while/switch
//! bodies and function bodies). Swift `let`/`var` are block-scoped;
//! nested functions and closures capture enclosing locals, so scopes
//! resolve outward without a def-style barrier.
//!
//! Two namespaces per scope: `vars` (properties, locals, parameters,
//! type names) and `methods` (functions). `call_expression` callees
//! resolve methods-then-vars (covers `Type()` initializers and
//! function-typed values); `navigation_expression` suffixes resolve
//! only for `self` receivers.
//!
//! Boundaries: `obj.m`/`obj.p` on non-self receivers records the member
//! name unresolved (unknown type), blocking member renames. Closure
//! parameter lists, property wrappers, result builders, `@` attributes,
//! operators, and string-interpolation edge cases are only partially
//! modeled — `unresolved_is_exact` is false so unresolved same-name
//! occurrences block renames.

use std::collections::HashMap;
use std::path::Path;

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
    "enum_declaration",
    "protocol_declaration",
    "extension_declaration",
];

const FN_KINDS: &[&str] = &["function_declaration", "init_declaration"];

#[derive(Clone, Copy, PartialEq, Debug)]
enum Ns {
    Var,
    Method,
}

pub(crate) fn analyze(
    spec: &LangSpec,
    contents: &str,
    _path: &Path,
) -> Option<Analysis> {
    let language: tree_sitter::Language = spec.grammar.ts_language();
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

/// Bind `bound_identifier` names inside `pattern` nodes (property
/// declarations, for-in items, optional bindings).
fn bind_pattern_names(st: &mut State, node: Node) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        // `bound_identifier:` is a field name; the node kind is
        // simple_identifier (or nested patterns for tuples).
        if child.kind() == "simple_identifier" || child.kind() == "bound_identifier" {
            bind_node(st, child, Ns::Var);
        } else if matches!(
            child.kind(),
            "pattern" | "tuple_pattern" | "value_binding_pattern" | "optional_pattern"
        ) {
            bind_pattern_names(st, child);
        }
    }
}

fn walk(node: Node, st: &mut State) {
    let kind = node.kind();
    if TYPE_KINDS.contains(&kind) {
        // Type name is a type_identifier child; type_body is member scope.
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if !child.is_named() {
                continue;
            }
            if child.kind() == "type_identifier" {
                let is_first = node
                    .children(&mut node.walk())
                    .find(|c| c.is_named())
                    .is_some_and(|c| c.id() == child.id());
                if is_first {
                    bind_node(st, child, Ns::Var);
                    continue;
                }
            }
            if child.kind() == "class_body" {
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
        "class_body" => {
            push(st, ScopeKind::Member);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        k if FN_KINDS.contains(&k) => {
            // function_declaration binds its simple_identifier name in
            // methods; init_declaration has no name.
            if k == "function_declaration"
                && let Some(name) = first_kind(node, "simple_identifier")
            {
                bind_node(st, name, Ns::Method);
                push(st, ScopeKind::Fn);
                walk_children_except(st, node, name);
                st.scopes.pop();
                return;
            }
            push(st, ScopeKind::Fn);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "parameter" => {
            // `external name: localName: Type` — bind the `name:` field
            // (the internal name), not external_name.
            if let Some(name) = node.child_by_field_name("name") {
                bind_node(st, name, Ns::Var);
            }
            walk_children_except_fields(st, node, &["name", "external_name"]);
            return;
        }
        "property_declaration" => {
            if let Some(name) = node.child_by_field_name("name") {
                if name.kind() == "pattern" {
                    bind_pattern_names(st, name);
                } else {
                    walk(name, st);
                }
            }
            walk_children_except_fields(st, node, &["name"]);
            return;
        }
        "pattern" => {
            bind_pattern_names(st, node);
            return;
        }
        "for_statement" => {
            // `for x in coll`: item pattern binds in loop scope.
            if let Some(item) = node.child_by_field_name("item") {
                bind_pattern_names(st, item);
            }
            push(st, ScopeKind::Block);
            walk_children_except_fields(st, node, &["item"]);
            st.scopes.pop();
            return;
        }
        "statements" => {
            // if/for/while/switch bodies and function bodies: block scope.
            push(st, ScopeKind::Block);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "call_expression" => {
            walk_call(st, node);
            return;
        }
        "navigation_expression" => {
            walk_navigation(st, node);
            return;
        }
        "simple_identifier" => {
            record_ref(st, text(st.contents, node), node.start_byte()..node.end_byte(), Ns::Var);
            return;
        }
        "type_identifier" => {
            record_ref(st, text(st.contents, node), node.start_byte()..node.end_byte(), Ns::Var);
            return;
        }
        "bound_identifier" => {
            // Stray pattern name outside declaration context: treat as
            // a reference rather than inventing a binding.
            record_ref(st, text(st.contents, node), node.start_byte()..node.end_byte(), Ns::Var);
            return;
        }
        _ if kind.contains("import") => {
            return;
        }
        _ => {}
    }
    walk_children(node, st);
}

fn first_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    node.children(&mut node.walk())
        .find(|c| c.is_named() && c.kind() == kind)
}

/// `name(args)`: callee simple_identifier -> methods-then-vars;
/// navigation callees -> member rules.
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
                "simple_identifier" => {
                    let range = child.start_byte()..child.end_byte();
                    let name = text(st.contents, child);
                    let t = lookup(st, name, Ns::Method).or_else(|| lookup(st, name, Ns::Var));
                    record_ref_target(st, range, t);
                    continue;
                }
                "navigation_expression" => {
                    walk_navigation(st, child);
                    continue;
                }
                _ => {}
            }
        }
        walk(child, st);
    }
}

/// `target.suffix`: suffix resolves only for `self` targets; other
/// receiver types are unknown -> unresolved blocker.
fn walk_navigation(st: &mut State, node: Node) {
    // navigation_expression field `suffix` is a navigation_suffix node
    // wrapping the member simple_identifier.
    let suffix_field = node.child_by_field_name("suffix");
    let name_node = suffix_field.and_then(|ns| {
        ns.children(&mut ns.walk())
            .find(|c| c.is_named() && c.kind() == "simple_identifier")
            .or(Some(ns))
    });
    let in_call = node
        .parent()
        .is_some_and(|p| p.kind() == "call_expression");
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named()
            && suffix_field.is_none_or(|sf| child.id() != sf.id())
        {
            walk(child, st);
        }
    }
    if let Some(s) = name_node {
        let range = s.start_byte()..s.end_byte();
        let name = text(st.contents, s);
        // The receiver is everything before the suffix: `self` only.
        let has_self = node
            .children(&mut node.walk())
            .any(|c| c.kind() == "self_expression");
        let only_self_parts = node
            .children(&mut node.walk())
            .filter(|c| c.is_named())
            .filter(|c| suffix_field.is_none_or(|sf| c.id() != sf.id()))
            .all(|c| matches!(c.kind(), "self_expression" | "navigation_suffix" | "simple_identifier"));
        if has_self && only_self_parts {
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
}

fn walk_children(node: Node, st: &mut State) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() {
            walk(child, st);
        }
    }
}

fn walk_children_except(st: &mut State, node: Node, skip: Node) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() && child.id() != skip.id() {
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
