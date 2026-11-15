//! Dart binding analysis via tree-sitter CST traversal.
//!
//! Scopes: source file → type bodies (class/mixin/extension/enum) →
//! function/method → blocks (and for-statement init scopes). Dart
//! locals and parameters are block-scoped; closures capture enclosing
//! scopes, so scopes resolve outward without a def-style barrier.
//!
//! Two namespaces per scope: `vars` (fields, locals, parameters, type
//! names, top-level variables) and `methods` (functions, methods,
//! getters/setters). Call callees resolve methods-then-vars (covers
//! `Type()` constructors and function-typed values); bare identifiers
//! resolve vars-then-methods (method tear-offs).
//!
//! `member_expression` property names resolve only for implicit or
//! explicit `this` receivers; `o.m`/`o.f` on other receivers records
//! the member unresolved, blocking member renames. Constructors are
//! not name-bound (calls resolve through the class type). Import
//! directives, cascade sections, and `noSuchMethod`/dynamic dispatch
//! are only partially modeled — `unresolved_is_exact` is false.

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
    "mixin_declaration",
    "enum_declaration",
];

const FN_KINDS: &[&str] = &["function_declaration", "method_declaration"];

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

/// Function/method signature: bind `name:` identifier as a method and
/// each `formal_parameter` name as a var inside the new Fn scope.
fn walk_signature<'a>(node: Node<'a>) -> Option<Node<'a>> {
    // Returns the function_signature node, searching through wrappers
    // like method_signature.
    if node.kind() == "function_signature" {
        return Some(node);
    }
    for child in node.children(&mut node.walk()) {
        if !child.is_named() {
            continue;
        }
        if child.kind() == "function_signature" {
            return Some(child);
        }
        if let Some(found) = walk_signature(child) {
            return Some(found);
        }
    }
    None
}

fn bind_params(st: &mut State, params: Node) {
    // formal_parameter_list > formal_parameter (name: identifier | bare identifier)
    let mut cursor = params.walk();
    for p in params.children(&mut cursor) {
        if !p.is_named() || p.kind() != "formal_parameter" {
            continue;
        }
        if let Some(name) = p.child_by_field_name("name") {
            bind_node(st, name, Ns::Var);
        } else {
            // Unnamed positional `(e) => ...`: single identifier child.
            let ident = p
                .children(&mut p.walk())
                .find(|c| c.is_named() && c.kind() == "identifier");
            if let Some(id) = ident {
                bind_node(st, id, Ns::Var);
            }
        }
    }
}

fn walk(node: Node, st: &mut State) {
    let kind = node.kind();
    if TYPE_KINDS.contains(&kind) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if !child.is_named() {
                continue;
            }
            match child.kind() {
                "identifier" => {
                    let is_first = node
                        .children(&mut node.walk())
                        .find(|c| c.is_named())
                        .is_some_and(|c| c.id() == child.id());
                    if is_first {
                        bind_node(st, child, Ns::Var);
                        continue;
                    }
                }
                "class_body" => {
                    push(st, ScopeKind::Member);
                    walk(child, st);
                    st.scopes.pop();
                    continue;
                }
                _ => {}
            }
            walk(child, st);
        }
        return;
    }
    match kind {
        "extension_declaration" => {
            // name: identifier binds as a type-ish var; body is member scope.
            if let Some(name) = node.child_by_field_name("name") {
                bind_node(st, name, Ns::Var);
            }
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if !child.is_named() {
                    continue;
                }
                if child.kind() == "extension_body" {
                    push(st, ScopeKind::Member);
                    walk(child, st);
                    st.scopes.pop();
                } else {
                    walk(child, st);
                }
            }
            return;
        }
        "class_body" | "enum_body" | "mixin_body" => {
            push(st, ScopeKind::Member);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        k if FN_KINDS.contains(&k) => {
            let sig = walk_signature(node);
            if let Some(sig) = sig {
                if let Some(name) = sig.child_by_field_name("name") {
                    bind_node(st, name, Ns::Method);
                }
                push(st, ScopeKind::Fn);
                if let Some(params) = sig.child_by_field_name("parameters") {
                    bind_params(st, params);
                }
                walk_children_except_fields(st, node, &["signature"]);
                // Also walk signature children except name/parameters
                // (return type refs).
                walk_children_except_fields(st, sig, &["name", "parameters"]);
                st.scopes.pop();
                return;
            }
            push(st, ScopeKind::Fn);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "constructor_declaration" | "factory_constructor_signature" => {
            // No name binding: `Foo()` resolves via the class type var.
            push(st, ScopeKind::Fn);
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named() && child.kind() == "formal_parameter_list" {
                    bind_params(st, child);
                }
            }
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "function_expression" => {
            // Lambda: params bind in a capturing scope.
            push(st, ScopeKind::Block);
            if let Some(params) = node.child_by_field_name("parameters") {
                bind_params(st, params);
            }
            walk_children_except_fields(st, node, &["parameters"]);
            st.scopes.pop();
            return;
        }
        // Declaration containers: leaf nodes below bind their own names.
        "local_variable_declaration" | "top_level_variable_declaration"
        | "declaration" | "initialized_identifier_list"
        | "variable_declaration_list" => {
            walk_children(node, st);
            return;
        }
        "initialized_variable_declaration" | "initialized_identifier" => {
            if let Some(name) = node.child_by_field_name("name") {
                bind_node(st, name, Ns::Var);
            }
            walk_children_except_fields(st, node, &["name"]);
            return;
        }
        "identifier_list" => {
            // `var a, b;` — bare identifier children are the names.
            let mut c = node.walk();
            for id in node.children(&mut c) {
                if id.is_named() && id.kind() == "identifier" {
                    bind_node(st, id, Ns::Var);
                }
            }
            return;
        }
        "formal_parameter" => {
            if let Some(name) = node.child_by_field_name("name") {
                bind_node(st, name, Ns::Var);
                walk_children_except_fields(st, node, &["name"]);
            } else {
                walk_children(node, st);
            }
            return;
        }
        "block" => {
            push(st, ScopeKind::Block);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "for_statement" => {
            // init declaration is scoped to the whole loop.
            push(st, ScopeKind::Block);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "call_expression" => {
            walk_call(st, node);
            return;
        }
        "member_expression" => {
            walk_member(st, node);
            return;
        }
        "assignable_expression" => {
            walk_assignable(st, node);
            return;
        }
        "identifier" => {
            // Bare identifier: vars first, then methods (tear-offs).
            let range = node.start_byte()..node.end_byte();
            let name = text(st.contents, node);
            let t = lookup(st, name, Ns::Var).or_else(|| lookup(st, name, Ns::Method));
            record_ref_target(st, range, t);
            return;
        }
        "type_identifier" => {
            record_ref(st, text(st.contents, node), node.start_byte()..node.end_byte(), Ns::Var);
            return;
        }
        k if k.contains("directive") || k.starts_with("import") || k.starts_with("export") || k.starts_with("part") || k == "library_directive" => {
            return;
        }
        _ => {}
    }
    walk_children(node, st);
}

/// `fn(args)`: identifier callee -> methods-then-vars; member
/// expression callees -> member rules.
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
                "member_expression" => {
                    walk_member(st, child);
                    continue;
                }
                _ => {}
            }
        }
        walk(child, st);
    }
}

/// `obj.prop`: property resolves only for implicit/explicit `this`
/// receivers; other objects are unknown -> unresolved blocker.
fn walk_member(st: &mut State, node: Node) {
    let prop = node.child_by_field_name("property");
    let object = node.child_by_field_name("object");
    // Walk the object expression for its own refs.
    if let Some(obj) = object {
        walk(obj, st);
    }
    if let Some(p) = prop {
        let range = p.start_byte()..p.end_byte();
        let name = text(st.contents, p);
        let is_this = object.is_none()
            || object.is_some_and(|o| {
                o.kind() == "this" || o.kind() == "this_expression"
            });
        if is_this {
            let in_call = node
                .parent()
                .is_some_and(|par| par.kind() == "call_expression");
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

/// Assignment LHS: `x`, `this.f`, `o.f` forms.
fn walk_assignable(st: &mut State, node: Node) {
    let prop = node.child_by_field_name("property");
    let object = node.child_by_field_name("object");
    if prop.is_some() {
        // Member assignment — same rules as member_expression.
        if let Some(obj) = object {
            walk(obj, st);
        }
        if let Some(p) = prop {
            let range = p.start_byte()..p.end_byte();
            let name = text(st.contents, p);
            let is_this = object.is_none()
                || object.is_some_and(|o| o.kind() == "this" || o.kind() == "this_expression");
            if is_this {
                record_ref(st, name, range, Ns::Var);
            } else {
                record_unresolved(st, range);
            }
        }
        walk_children_except_fields(st, node, &["property", "object"]);
        return;
    }
    walk_children(node, st);
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
