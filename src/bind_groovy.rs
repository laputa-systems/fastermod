//! Groovy binding analysis. Groovy is dynamically dispatched: method calls
//! may resolve through `methodMissing`, categories, or runtime metaclass
//! tricks, so member access through receivers is never resolved — it is
//! recorded as an unresolved occurrence that blocks same-spelled renames.
//! Local scopes follow Java-ish rules: class bodies hold members, blocks
//! and closures hold locals, `def` declarations bind in the innermost
//! scope.

use std::collections::HashMap;
use std::path::Path;

use tree_sitter::{Node, Parser};

use crate::bind::{Analysis, BindingSite, RefSite};
use crate::lang::LangSpec;

struct Scope {
    bindings: HashMap<String, usize>,
}

struct State<'a> {
    analysis: Analysis,
    scopes: Vec<Scope>,
    contents: &'a [u8],
}

fn text<'a>(contents: &'a [u8], node: Node) -> &'a str {
    node.utf8_text(contents).unwrap_or("")
}

fn is_name_kind(kind: &str) -> bool {
    kind == "identifier" || kind == "type_identifier"
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
            bindings: HashMap::new(),
        }],
        contents: contents.as_bytes(),
    };
    walk(root, &mut st);
    // Dynamic dispatch means same-named occurrences anywhere may refer to
    // the renamed symbol; keep unresolved sets inexact so the workspace
    // path blocks rather than guessing.
    st.analysis.unresolved_is_exact = false;
    Some(st.analysis)
}

fn cur(st: &State) -> usize {
    st.scopes.len() - 1
}

fn bind(st: &mut State, name: &str, range: std::ops::Range<usize>) {
    if name.is_empty() {
        return;
    }
    let idx = st.analysis.bindings.len();
    st.analysis.bindings.push(BindingSite {
        name: range,
        text: name.to_string(),
        expansion: None,
        module_level: st.scopes.len() <= 2,
        is_mod: false,
    });
    let si = cur(st);
    st.scopes[si].bindings.insert(name.to_string(), idx);
}

fn resolve(st: &State, name: &str) -> Option<usize> {
    for s in st.scopes.iter().rev() {
        if let Some(&b) = s.bindings.get(name) {
            return Some(b);
        }
    }
    None
}

fn record_ref(st: &mut State, name: &str, range: std::ops::Range<usize>) {
    let target = resolve(st, name);
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
        bindings: HashMap::new(),
    });
}

/// Bind every `name`-field identifier child (declarators, parameters).
fn bind_name_fields(st: &mut State, node: Node) {
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        if node.field_name_for_child(i) == Some("name") && is_name_kind(child.kind()) {
            let name = text(st.contents, child);
            bind(st, name, child.start_byte()..child.end_byte());
        }
    }
}

fn walk_params(node: Node, st: &mut State) {
    // formal_parameter carries type + name; bind only the name field.
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        match child.kind() {
            "formal_parameter" => bind_name_fields(st, child),
            _ => {
                if is_name_kind(child.kind()) && node.field_name_for_child(i) == Some("name") {
                    let name = text(st.contents, child);
                    bind(st, name, child.start_byte()..child.end_byte());
                }
            }
        }
    }
}

fn walk(node: Node, st: &mut State) {
    match node.kind() {
        "class_declaration" | "interface_declaration" | "trait_declaration" | "enum_declaration" => {
            // The declared type name binds at program level.
            if let Some(name) = node.child_by_field_name("name")
                && is_name_kind(name.kind())
            {
                let n = text(st.contents, name);
                bind(st, n, name.start_byte()..name.end_byte());
            }
            for i in 0..node.child_count() {
                let child = node.child(i).unwrap();
                if child.kind() == "class_body" {
                    push(st);
                    walk_children(child, st);
                    st.scopes.pop();
                    return;
                }
            }
            walk_children(node, st);
            return;
        }
        "method_declaration" | "constructor_declaration" => {
            if let Some(name) = node.child_by_field_name("name")
                && is_name_kind(name.kind())
            {
                let n = text(st.contents, name);
                bind(st, n, name.start_byte()..name.end_byte());
            }
            push(st);
            if let Some(params) = node.child_by_field_name("parameters") {
                walk_params(params, st);
            }
            if let Some(body) = node.child_by_field_name("body") {
                walk(body, st);
            }
            st.scopes.pop();
            return;
        }
        "function_definition" => {
            // `def name(...)`: identifier child with field "name".
            if let Some(name) = node.child_by_field_name("name")
                && is_name_kind(name.kind())
            {
                let n = text(st.contents, name);
                bind(st, n, name.start_byte()..name.end_byte());
            }
            push(st);
            if let Some(params) = node.child_by_field_name("parameters") {
                walk_params(params, st);
            }
            // Body may be `closure` or `block` under field "body".
            for i in 0..node.child_count() {
                let child = node.child(i).unwrap();
                if matches!(child.kind(), "closure" | "block") {
                    walk(child, st);
                }
            }
            st.scopes.pop();
            return;
        }
        "closure" | "block" => {
            push(st);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "field_declaration" => {
            // Walk the type, then bind declarator names.
            for i in 0..node.child_count() {
                let child = node.child(i).unwrap();
                match child.kind() {
                    "variable_declarator" => bind_name_fields(st, child),
                    _ => walk(child, st),
                }
            }
            return;
        }
        "variable_declarator" => {
            // value expressions precede the declared name in source but the
            // name binds after them is evaluated — walk value first.
            if let Some(v) = node.child_by_field_name("value") {
                walk(v, st);
            }
            bind_name_fields(st, node);
            return;
        }
        "for_statement" => {
            push(st);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "enhanced_for_statement" | "for_in_clause" => {
            // `for (x in y)`: bind the loop variable, walk the iterable.
            for i in 0..node.child_count() {
                let child = node.child(i).unwrap();
                if child.kind() == "identifier" {
                    let n = text(st.contents, child);
                    bind(st, n, child.start_byte()..child.end_byte());
                } else {
                    walk(child, st);
                }
            }
            return;
        }
        "catch_clause" => {
            push(st);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "catch_formal_parameter" => {
            for i in 0..node.child_count() {
                let child = node.child(i).unwrap();
                if node.field_name_for_child(i) == Some("name") && is_name_kind(child.kind()) {
                    let name = text(st.contents, child);
                    bind(st, name, child.start_byte()..child.end_byte());
                } else {
                    walk(child, st);
                }
            }
            return;
        }
        "method_invocation" => {
            // `name(args)` or `obj.name(args)`: the invoked name resolves
            // as a plain identifier; object/field parts are member access.
            if let Some(name) = node.child_by_field_name("name")
                && is_name_kind(name.kind())
            {
                let n = text(st.contents, name);
                record_ref(st, n, name.start_byte()..name.end_byte());
            }
            for i in 0..node.child_count() {
                let child = node.child(i).unwrap();
                if Some(child.id()) == node.child_by_field_name("name").map(|n| n.id()) {
                    continue;
                }
                if child.kind() == "argument_list" || child.kind() == "object" {
                    walk(child, st);
                } else if is_name_kind(child.kind()) {
                    // e.g. `obj.name(...)`: `obj` is a normal ref.
                    let n = text(st.contents, child);
                    record_ref(st, n, child.start_byte()..child.end_byte());
                }
            }
            return;
        }
        "field_access" => {
            // `object.field`: object is a normal ref; the field name is a
            // member — unresolved so same-spelled renames block.
            if let Some(obj) = node.child_by_field_name("object") {
                walk(obj, st);
            }
            if let Some(field) = node.child_by_field_name("field")
                && is_name_kind(field.kind())
            {
                record_unresolved(st, field.start_byte()..field.end_byte());
            }
            return;
        }
        "identifier" | "type_identifier" => {
            // Bare identifier occurrence.
            let parent_kind = node.parent().map(|p| p.kind()).unwrap_or("");
            if matches!(
                parent_kind,
                "variable_declarator" | "formal_parameter" | "field_declaration"
                    | "method_declaration" | "function_definition" | "class_declaration"
            ) && node.parent().and_then(|p| p.child_by_field_name("name")).map(|n| n.id())
                == Some(node.id())
            {
                return; // declaration name already bound
            }
            let n = text(st.contents, node);
            record_ref(st, n, node.start_byte()..node.end_byte());
            return;
        }
        _ => {}
    }
    walk_children(node, st);
}

fn walk_children(node: Node, st: &mut State) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, st);
    }
}
