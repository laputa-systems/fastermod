//! Java binding analysis via tree-sitter CST traversal.
//!
//! Scopes: program → class member (fields, methods, constructors, type
//! parameters) → function (parameters, locals) → block. Class members
//! are `module_level` so the workspace path guards cross-file references;
//! Java imports are not resolved, so unresolved same-name occurrences in
//! sibling files block type/member renames rather than silently going
//! stale.
//!
//! Boundaries: qualified access `obj.m()` / `obj.f` cannot be resolved
//! to a declaration (unknown receiver type, virtual dispatch). Unless
//! the receiver is `this`, the member name is recorded as an *unresolved*
//! reference, which blocks renaming that member. `instanceof` pattern
//! variables, `case` type patterns, try-with-resources, catch parameters,
//! lambdas, and enhanced-for variables are bound. `unresolved_is_exact`
//! is false: the grammar does not cover every binding form.

use std::collections::HashMap;
use std::path::Path;

use ast_grep_core::tree_sitter::LanguageExt;
use tree_sitter::{Node, Parser};

use crate::bind::{Analysis, BindingSite, RefSite};
use crate::lang::LangSpec;

#[derive(Clone, Copy, PartialEq)]
enum ScopeKind {
    /// Program top level: type declarations.
    Root,
    /// Class/interface/enum/record body: fields, methods, ctors.
    Member,
    /// Method/ctor/lambda: parameters and body locals.
    Fn,
    /// Any other block: locals.
    Block,
}

struct Scope {
    kind: ScopeKind,
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

const CLASS_KINDS: &[&str] = &[
    "class_declaration",
    "interface_declaration",
    "enum_declaration",
    "record_declaration",
    "annotation_type_declaration",
];

const FN_KINDS: &[&str] = &["method_declaration", "constructor_declaration"];

/// Kinds that open a lexical block scope.
const BLOCK_KINDS: &[&str] = &[
    "block",
    "for_statement",
    "enhanced_for_statement",
    "catch_clause",
    "try_statement",
    "try_with_resources_statement",
    "synchronized_statement",
    "switch_expression",
    "switch_block",
    "switch_block_statement_group",
    "switch_rule",
    "do_statement",
    "if_statement",
    "while_statement",
];

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
            bindings: HashMap::new(),
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

/// A reference we cannot resolve: still an `old`-spelled occurrence that
/// may denote the symbol, so it blocks rather than being ignored.
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
        bindings: HashMap::new(),
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
    if CLASS_KINDS.contains(&kind) {
        // Type name binds in the enclosing scope; the body is a member
        // scope holding fields/methods.
        if let Some(name) = node.child_by_field_name("name") {
            if name.kind() == "identifier" {
                bind_node(st, name);
            } else {
                walk(name, st);
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if !child.is_named() {
                continue;
            }
            if child.kind() == "class_body" {
                push(st, ScopeKind::Member);
                walk(child, st);
                st.scopes.pop();
            } else if child.kind() != "identifier" {
                walk(child, st);
            }
        }
        return;
    }
    match kind {
        "class_body" | "interface_body" | "enum_body" | "record_body"
        | "annotation_type_body" => {
            push(st, ScopeKind::Member);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        k if FN_KINDS.contains(&k) => {
            // Method name binds in the member scope. A constructor's
            // "name" is the class name: a reference to the type binding.
            if k == "constructor_declaration" {
                if let Some(name) = node.child_by_field_name("name") {
                    record_ref(st, text(st.contents, name), name.start_byte()..name.end_byte());
                }
            } else if let Some(name) = node.child_by_field_name("name") {
                bind_node(st, name);
            }
            push(st, ScopeKind::Fn);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "lambda_expression" => {
            push(st, ScopeKind::Fn);
            // Untyped single param arrives as `parameters: (identifier)`;
            // typed params arrive as formal_parameters.
            if let Some(params) = node.child_by_field_name("parameters") {
                if params.kind() == "identifier" {
                    bind_node(st, params);
                } else {
                    walk(params, st);
                }
            }
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named()
                    && child.id() != node.child_by_field_name("parameters").map(|n| n.id()).unwrap_or(0)
                {
                    walk(child, st);
                }
            }
            st.scopes.pop();
            return;
        }
        "formal_parameters" | "formal_parameter" | "receiver_parameter"
        | "catch_formal_parameter" => {
            bind_name_fields(st, node);
            walk_children_except_names(node, st);
            return;
        }
        "variable_declarator" => {
            // `int a = 1, b = a`: JLS scopes each declarator from its own
            // name onward, so bind the name before walking the value.
            if let Some(name) = node.child_by_field_name("name") {
                bind_node(st, name);
            }
            walk_children_except_names(node, st);
            return;
        }
        "enhanced_for_statement" => {
            // `for (T x : expr) body`: type+iterable see outer bindings;
            // the loop variable scopes over the body only.
            let name = node.child_by_field_name("name");
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if !child.is_named() {
                    continue;
                }
                if Some(child.id()) == name.map(|n| n.id()) {
                    continue;
                }
                if child.kind() == "block" || child.kind() == "statement" {
                    continue; // body walked after binding
                }
                walk(child, st);
            }
            if let Some(n) = name
                && n.kind() == "identifier"
            {
                bind_node(st, n);
            }
            if let Some(body) = node.child_by_field_name("body") {
                walk(body, st);
            }
            return;
        }
        "resource" => {
            // try-with-resources `R r = expr`: bind after the initializer.
            if let Some(value) = node.child_by_field_name("value") {
                walk(value, st);
            }
            if let Some(name) = node.child_by_field_name("name")
                && name.kind() == "identifier"
            {
                bind_node(st, name);
            }
            walk_children_except_fields(node, st, &["name", "value"]);
            return;
        }
        "enum_constant" | "record_component" => {
            if let Some(name) = node.child_by_field_name("name") {
                bind_node(st, name);
            }
            walk_children_except_names(node, st);
            return;
        }
        "type_pattern" => {
            // `case String s` / `instanceof String s`: last identifier is
            // the pattern variable; the type stays a type ref.
            let mut last_ident = None;
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if !child.is_named() {
                    continue;
                }
                if child.kind() == "identifier" {
                    last_ident = Some(child);
                } else {
                    walk(child, st);
                }
            }
            if let Some(n) = last_ident {
                bind_node(st, n);
            }
            return;
        }
        "instanceof_expression" => {
            // Pattern variable (when present) scopes over the rest of the
            // enclosing statement — approximated by the current scope.
            walk_children_except_names(node, st);
            bind_name_fields(st, node);
            return;
        }
        "method_invocation" => {
            walk_member_access(st, node, true);
            return;
        }
        "field_access" => {
            walk_member_access(st, node, false);
            return;
        }
        "scoped_type_identifier" => {
            // `a.b.C`: only the final segment names a type; package
            // prefixes are not symbol references.
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if !child.is_named() {
                    continue;
                }
                match child.kind() {
                    "scoped_type_identifier" => {}
                    _ => walk(child, st),
                }
            }
            return;
        }
        "identifier" | "type_identifier" => {
            record_ref(st, text(st.contents, node), node.start_byte()..node.end_byte());
            return;
        }
        "package_declaration" | "import_declaration" => {
            return;
        }
        k if BLOCK_KINDS.contains(&k) => {
            push(st, ScopeKind::Block);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        _ => {}
    }
    walk_children(node, st);
}

/// `obj.name(...)` / `obj.field`: the receiver is walked; the member name
/// resolves only through `this` (same class). Any other receiver type is
/// unknown, so the name is an unresolved blocker rather than a resolved
/// reference to an unrelated same-named member.
fn walk_member_access(st: &mut State, node: Node, is_invocation: bool) {
    let object = node.child_by_field_name("object");
    let name = node.child_by_field_name(if is_invocation { "name" } else { "field" });
    // Unqualified names (`helper()`, bare field reads never reach here)
    // and `this.` receivers resolve through the member scope; any other
    // receiver type is unknown.
    let resolvable = object.is_none_or(|o| text(st.contents, o) == "this");
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        if Some(child.id()) == name.map(|n| n.id()) {
            if let Some(n) = name {
                let range = n.start_byte()..n.end_byte();
                let t = text(st.contents, n);
                if resolvable {
                    record_ref(st, t, range);
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

/// Walk children, skipping `name`-field identifier leaves already bound.
fn walk_children_except_names(node: Node, st: &mut State) {
    walk_children_except_fields(node, st, &["name"]);
}

fn walk_children_except_fields(node: Node, st: &mut State, skip: &[&str]) {
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        if !child.is_named() {
            continue;
        }
        let fname = node.field_name_for_child(i);
        if fname.is_some_and(|f| skip.contains(&f)) && child.kind() == "identifier" {
            continue;
        }
        walk(child, st);
    }
}

/// Bind every `name`-field identifier child.
fn bind_name_fields(st: &mut State, node: Node) {
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        if node.field_name_for_child(i) == Some("name") && child.kind() == "identifier" {
            bind_node(st, child);
        }
    }
}
