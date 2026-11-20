//! Zig binding analysis via tree-sitter CST traversal.
//!
//! Scopes: source file → container bodies (struct/enum/union/opaque)
//! → function bodies → blocks (if/for/while/switch bodies). Zig
//! `var`/`const` declarations are block-scoped; `for`/`switch` payload
//! captures bind in the loop/switch scope.
//!
//! One `vars` namespace per scope covers functions, parameters,
//! variables, container fields and type names — Zig has no separate
//! value/type namespace for rename purposes here. `field_expression`
//! (`obj.member`, `ptr->member`) records the member unresolved because
//! receiver types are not modeled, so member renames block.
//! `unresolved_is_exact` is false: comptime evaluation, `@import`
//! namespacing, generics, and `anytype` dispatch are only partially
//! modeled. All bindings are file-local.

use std::collections::HashMap;
use std::path::Path;

use tree_sitter::{Node, Parser};

use crate::bind::{Analysis, BindingSite, RefSite};
use crate::lang::LangSpec;

struct Scope {
    vars: HashMap<String, usize>,
}

struct State<'a> {
    analysis: Analysis,
    scopes: Vec<Scope>,
    contents: &'a [u8],
}

fn text<'a>(contents: &'a [u8], node: Node) -> &'a str {
    node.utf8_text(contents).unwrap_or("")
}

const CONTAINER_KINDS: &[&str] = &[
    "struct_declaration",
    "enum_declaration",
    "union_declaration",
    "opaque_declaration",
];

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
            vars: HashMap::new(),
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

fn bind(st: &mut State, name: &str, range: std::ops::Range<usize>) {
    let idx = st.analysis.bindings.len();
    st.analysis.bindings.push(BindingSite {
        name: range,
        text: name.to_string(),
        expansion: None,
        module_level: false,
        is_mod: false,
    });
    let si = cur(st);
    st.scopes[si].vars.insert(name.to_string(), idx);
}

fn lookup(st: &State, name: &str) -> Option<usize> {
    for s in st.scopes.iter().rev() {
        if let Some(&b) = s.vars.get(name) {
            return Some(b);
        }
    }
    None
}

fn record_ref(st: &mut State, name: &str, range: std::ops::Range<usize>) {
    let target = lookup(st, name);
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

fn push(st: &mut State) {
    st.scopes.push(Scope {
        vars: HashMap::new(),
    });
}

fn bind_node(st: &mut State, node: Node) {
    bind(
        st,
        text(st.contents, node),
        node.start_byte()..node.end_byte(),
    );
}

fn walk(node: Node, st: &mut State) {
    let kind = node.kind();
    if CONTAINER_KINDS.contains(&kind) {
        push(st);
        walk_children(node, st);
        st.scopes.pop();
        return;
    }
    match kind {
        "function_declaration" | "test_declaration" => {
            if let Some(name) = node.child_by_field_name("name") {
                bind_node(st, name);
            }
            push(st);
            // parameters > parameter name:(identifier)
            if let Some(params) = node.child_by_field_name("parameters") {
                bind_params(st, params);
            }
            walk_children_except_fields(st, node, &["name", "parameters"]);
            st.scopes.pop();
            return;
        }
        "parameters" => {
            bind_params(st, node);
            return;
        }
        "parameter" => {
            if let Some(name) = node.child_by_field_name("name") {
                bind_node(st, name);
            }
            walk_children_except_fields(st, node, &["name"]);
            return;
        }
        "variable_declaration" => {
            // children: identifier name then init expression.
            let mut cursor = node.walk();
            let mut first = true;
            for child in node.children(&mut cursor) {
                if !child.is_named() {
                    continue;
                }
                if first && child.kind() == "identifier" {
                    first = false;
                    bind_node(st, child);
                    continue;
                }
                walk(child, st);
            }
            return;
        }
        "container_field" => {
            if let Some(name) = node.child_by_field_name("name") {
                bind_node(st, name);
            }
            walk_children_except_fields(st, node, &["name"]);
            return;
        }
        "block" | "block_expression" | "switch_case" => {
            push(st);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "for_statement" | "while_statement" | "if_statement" => {
            // payload captures bind, then body gets a scope.
            push(st);
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if !child.is_named() {
                    continue;
                }
                if child.kind() == "payload" {
                    bind_payload(st, child);
                } else {
                    walk(child, st);
                }
            }
            st.scopes.pop();
            return;
        }
        "field_expression" => {
            if let Some(obj) = node.child_by_field_name("object") {
                walk(obj, st);
            }
            if let Some(member) = node.child_by_field_name("member") {
                record_unresolved(st, member.start_byte()..member.end_byte());
            }
            walk_children_except_fields(st, node, &["object", "member"]);
            return;
        }
        "identifier" => {
            record_ref(st, text(st.contents, node), node.start_byte()..node.end_byte());
            return;
        }
        k if k.starts_with("builtin") || k == "primitive_type" => {
            return;
        }
        _ => {}
    }
    walk_children(node, st);
}

fn bind_params(st: &mut State, params: Node) {
    let mut cursor = params.walk();
    for p in params.children(&mut cursor) {
        if !p.is_named() {
            continue;
        }
        walk(p, st);
    }
}

/// `|x, y|` payload: identifier children bind in current scope.
fn bind_payload(st: &mut State, payload: Node) {
    let mut cursor = payload.walk();
    for child in payload.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        if child.kind() == "identifier" {
            bind_node(st, child);
        } else {
            bind_payload(st, child);
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
