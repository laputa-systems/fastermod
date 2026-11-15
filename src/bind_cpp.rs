//! C++ binding analysis via tree-sitter CST traversal.
//!
//! Scopes: translation unit → namespace bodies → class/struct/union
//! member scopes → function bodies → compound statements. Ordinary
//! identifiers (functions, variables, typedefs, tags, enum constants)
//! share one `vars` namespace per scope; members are visible
//! unqualified inside member functions through normal scope-chain
//! lookup, matching C++ name lookup closely enough for local rename.
//!
//! `field_expression` (`s.x`, `p->x`, `this->x`) resolves the member
//! name only for `this` receivers; other receivers are unknown and
//! record the member unresolved, blocking member renames. Qualified
//! `A::b` access resolves only through ordinary scope lookup, so
//! cross-class qualified members stay unresolved and block.
//!
//! Declarator chains are unwrapped to find bound identifiers.
//! `unresolved_is_exact` is false: templates, SFINAE, operator
//! overloads, macros, and dynamic constructs are only partially
//! modeled. All bindings are file-local — cross-file C++ resolution
//! (headers, namespaces across TUs) is not modeled.

use std::collections::HashMap;
use std::path::Path;

use tree_sitter::{Node, Parser};

use crate::bind::{Analysis, BindingSite, RefSite};
use crate::lang::LangSpec;

#[derive(Clone, Copy, PartialEq)]
enum ScopeKind {
    Root,
    Fn,
    Block,
    Member,
}

struct Scope {
    kind: ScopeKind,
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

/// `this->x` refers to the innermost class's members only — local
/// variables in intervening function/block scopes cannot be members.
fn member_lookup(st: &State, name: &str) -> Option<usize> {
    st.scopes
        .iter()
        .rev()
        .find(|s| s.kind == ScopeKind::Member)
        .and_then(|s| s.vars.get(name).copied())
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

fn push(st: &mut State, kind: ScopeKind) {
    st.scopes.push(Scope {
        kind,
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

/// Unwrap declarator wrappers to the node carrying the declared name.
fn declarator_name<'a>(node: Node<'a>) -> Option<Node<'a>> {
    match node.kind() {
        "identifier" | "field_identifier" | "type_identifier" => Some(node),
        "qualified_identifier" => node
            .children(&mut node.walk())
            .filter(|c| c.is_named())
            .last(),
        "init_declarator" => node
            .child_by_field_name("declarator")
            .and_then(declarator_name),
        "function_declarator" | "pointer_declarator" | "array_declarator"
        | "parenthesized_declarator" | "reference_declarator"
        | "attributed_declarator" => node
            .child_by_field_name("declarator")
            .and_then(declarator_name)
            .or_else(|| {
                node.children(&mut node.walk())
                    .find(|c| c.is_named())
                    .and_then(declarator_name)
            }),
        _ => None,
    }
}

/// Bind declarator names directly under a declaration-like node and
/// walk the rest for refs. `declarator` field children and bare
/// identifier siblings (comma-separated declarators) are names.
fn handle_declaration(st: &mut State, node: Node) {
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        if !child.is_named() {
            continue;
        }
        let fname = node.field_name_for_child(i);
        if fname == Some("declarator") {
            if let Some(name) = declarator_name(child) {
                bind_node(st, name);
            }
            if child.kind() == "init_declarator" {
                // Walk the value expression, then only the declarator
                // subtree (array sizes etc.) — not the value again.
                if let Some(v) = child.child_by_field_name("value") {
                    walk(v, st);
                }
                if let Some(d) = child.child_by_field_name("declarator") {
                    walk_declarator_rest(st, d);
                }
            } else {
                walk_declarator_rest(st, child);
            }
        } else if matches!(child.kind(), "identifier" | "field_identifier")
            && fname.is_none()
        {
            // Comma-separated extra declarators without field names.
            bind_node(st, child);
        } else {
            walk(child, st);
        }
    }
}

fn walk(node: Node, st: &mut State) {
    let kind = node.kind();
    if kind.starts_with("preproc") {
        return;
    }
    match kind {
        "function_definition" => {
            // declarator: function_declarator > declarator: identifier,
            // parameters: parameter_list, body: compound_statement.
            let mut name_node = None;
            let mut param_list = None;
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if !child.is_named() {
                    continue;
                }
                match child.kind() {
                    "function_declarator" => {
                        if let Some(d) = child.child_by_field_name("declarator") {
                            name_node = declarator_name(d);
                        }
                        if let Some(p) = child.child_by_field_name("parameters") {
                            param_list = Some(p);
                        }
                        // Walk parameter types for refs later.
                    }
                    "type_identifier" | "primitive_type" | "sized_type_specifier"
                    | "struct_specifier" | "enum_specifier" | "union_specifier" => {
                        walk(child, st);
                    }
                    "compound_statement" => {}
                    _ => walk(child, st),
                }
            }
            if let Some(n) = name_node {
                bind_node(st, n);
            }
            push(st, ScopeKind::Fn);
            if let Some(pl) = param_list {
                bind_params(st, pl);
            }
            // Walk body.
            if let Some(body) = node.child_by_field_name("body") {
                walk(body, st);
            }
            st.scopes.pop();
            return;
        }
        "declaration" | "field_declaration" | "type_definition" => {
            handle_declaration(st, node);
            return;
        }
        "class_specifier" | "struct_specifier" | "union_specifier" => {
            if let Some(name) = node.child_by_field_name("name") {
                bind_node(st, name);
            }
            let name_id = node.child_by_field_name("name").map(|n| n.id());
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if !child.is_named() {
                    continue;
                }
                if child.kind() == "field_declaration_list" {
                    push(st, ScopeKind::Member);
                    walk(child, st);
                    st.scopes.pop();
                } else if Some(child.id()) != name_id {
                    walk(child, st);
                }
            }
            return;
        }
        "namespace_definition" | "template_declaration" | "alias_declaration" => {
            // Transparent scopes: namespace bodies get a block scope so
            // members don't leak to siblings; template/alias walk through.
            if kind == "namespace_definition" {
                push(st, ScopeKind::Block);
                walk_children(node, st);
                st.scopes.pop();
            } else {
                walk_children(node, st);
            }
            return;
        }
        "enum_specifier" => {
            if let Some(name) = node.child_by_field_name("name") {
                bind_node(st, name);
            }
            walk_children(node, st);
            return;
        }
        "enumerator" => {
            if let Some(name) = node.child_by_field_name("name") {
                bind_node(st, name);
            }
            if let Some(val) = node.child_by_field_name("value") {
                walk(val, st);
            }
            return;
        }
        "field_expression" => {
            // `arg.field` / `arg->field` / `this->x`: resolve only for
            // `this` receivers; others are unknown -> unresolved.
            let arg = node.child_by_field_name("argument");
            let is_this = arg.is_some_and(|a| a.kind() == "this");
            if let Some(a) = arg {
                walk(a, st);
            }
            if let Some(field) = node.child_by_field_name("field") {
                let range = field.start_byte()..field.end_byte();
                if is_this {
                    let name = text(st.contents, field);
                    let target = member_lookup(st, name);
                    st.analysis.refs.push(RefSite {
                        name: range,
                        target,
                        expansion: None,
                        remote: false,
                        module_attr: None,
                    });
                } else {
                    record_unresolved(st, range);
                }
            }
            walk_children_except_fields(st, node, &["argument", "field"]);
            return;
        }
        "compound_statement" => {
            push(st, ScopeKind::Block);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "for_statement" => {
            // init declaration scoped to the whole loop.
            push(st, ScopeKind::Block);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "parameter_declaration" => {
            // Only reached outside function definitions (prototypes):
            // walk the type for refs, never bind the name.
            if let Some(t) = node.child_by_field_name("type") {
                walk(t, st);
            }
            return;
        }
        "identifier" | "field_identifier" | "type_identifier" => {
            record_ref(st, text(st.contents, node), node.start_byte()..node.end_byte());
            return;
        }
        "this" | "new_expression" | "delete_expression" => {
            if kind == "this" {
                return;
            }
        }
        "label_statement" | "goto_statement" | "case_statement" => {
            // Labels are a separate namespace; skip to avoid false
            // unresolved blockers (case labels' expressions still need
            // walking — handled below for case only).
            if kind == "case_statement" {
                walk_children(node, st);
            }
            return;
        }
        _ => {}
    }
    walk_children(node, st);
}

fn bind_params(st: &mut State, param_list: Node) {
    let mut cursor = param_list.walk();
    for p in param_list.children(&mut cursor) {
        if !p.is_named() {
            continue;
        }
        match p.kind() {
            "parameter_declaration" | "variadic_parameter" => {
                if let Some(d) = p.child_by_field_name("declarator") {
                    if let Some(name) = declarator_name(d) {
                        bind_node(st, name);
                    }
                } else if let Some(d) = first_declarator_child(p)
                    && let Some(name) = declarator_name(d)
                {
                    bind_node(st, name);
                }
                // Walk parameter type for refs.
                if let Some(t) = p.child_by_field_name("type") {
                    walk(t, st);
                }
            }
            "parameter_list" => bind_params(st, p),
            _ => walk(p, st),
        }
    }
}

fn first_declarator_child<'a>(node: Node<'a>) -> Option<Node<'a>> {
    node.children(&mut node.walk())
        .find(|c| c.is_named() && c.kind() != "type_identifier" && c.kind() != "primitive_type")
}

fn walk_declarator_rest(st: &mut State, node: Node) {
    let name = declarator_name(node);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        if Some(child.id()) == name.map(|n| n.id()) {
            continue;
        }
        walk_declarator_rest_or_walk(st, child, name);
    }
}

fn walk_declarator_rest_or_walk(st: &mut State, node: Node, name: Option<Node>) {
    if Some(node.id()) == name.map(|n| n.id()) {
        return;
    }
    if matches!(
        node.kind(),
        "identifier" | "field_identifier" | "type_identifier"
    ) && node.child_count() == 0
    {
        // A different leaf identifier inside the declarator (e.g. an
        // array size expression) — walk normally.
        record_ref(st, text(st.contents, node), node.start_byte()..node.end_byte());
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() {
            walk_declarator_rest_or_walk(st, child, name);
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
