//! Kotlin binding analysis via tree-sitter CST traversal.
//!
//! Scopes: source file → declaration containers (class/object bodies)
//! → function → block-ish bodies (control_structure_body, when, try,
//! catch, lambda bodies). Kotlin declarations are block-scoped, so
//! control bodies push scopes; `for` variables scope to the loop.
//!
//! Two namespaces per scope: `vars` (properties, local `val`/`var`,
//! parameters, type names) and `methods` (functions, including
//! constructors are not bound — the name references the type).
//! `simple_identifier` refs resolve as vars; `call_expression` callees
//! and navigation suffixes in call position resolve as methods.
//!
//! Boundaries: navigation `a.b` on receivers other than `this` records
//! the member name unresolved (unknown type), blocking member renames.
//! `import` resolution, extension functions, delegated properties,
//! `it` implicit lambda params, and string-template edge cases are not
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
    "object_declaration",
    "interface_declaration",
    "enum_class_declaration",
];

const FN_KINDS: &[&str] = &[
    "function_declaration",
    "secondary_constructor",
    "function_literal",
    "lambda_literal",
];

const BLOCK_KINDS: &[&str] = &[
    "control_structure_body",
    "when_expression",
    "try_expression",
    "catch_block",
    "finally_block",
    "block",
    "do_while_statement",
    "while_statement",
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

/// Bind declaration names: direct `simple_identifier`/`identifier`
/// children plus nested declaration structures (destructuring). Never
/// descends into expressions — initializers are siblings of
/// `variable_declaration`, not children.
fn bind_decl_names(st: &mut State, node: Node) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        match child.kind() {
            "simple_identifier" | "identifier" => bind_node(st, child, Ns::Var),
            "variable_declaration" | "destructuring_declaration"
            | "destructuring_declaration_entry" => bind_decl_names(st, child),
            _ => {}
        }
    }
}

fn walk(node: Node, st: &mut State) {
    let kind = node.kind();
    if TYPE_KINDS.contains(&kind) {
        // Type name is a `type_identifier` child binding in the
        // enclosing vars namespace; class_body is the member scope.
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if !child.is_named() {
                continue;
            }
            if child.kind() == "type_identifier" {
                // The first type_identifier is the declared name; nested
                // ones (supertypes) are refs — handled by walking.
                // Distinguish: the name is the first named child.
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
        "class_body" | "enum_class_body" => {
            push(st, ScopeKind::Member);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        k if FN_KINDS.contains(&k) => {
            // Function name binds in the enclosing methods namespace.
            // secondary_constructor/lambdas have no name.
            let named = matches!(k, "function_declaration");
            if named && let Some(name) = first_named_of_kind(node, &["simple_identifier"]) {
                bind_node(st, name, Ns::Method);
            }
            push(st, ScopeKind::Fn);
            if named {
                walk_children_except_first_ident(node, st);
            } else {
                walk_children(node, st);
            }
            st.scopes.pop();
            return;
        }
        "property_declaration" => {
            // `val/var name [: type] [= init]`: names live in the
            // variable_declaration child; type/initializer are siblings.
            if let Some(vd) = find_child(node, "variable_declaration") {
                bind_decl_names(st, vd);
                walk_children_except(st, node, vd);
            } else {
                walk_children(node, st);
            }
            return;
        }
        "class_parameter" => {
            // Primary-constructor `val/var prop: T` becomes a member
            // property; plain params bind as vars usable in initializers.
            if let Some(name) = first_named_of_kind(node, &["simple_identifier"]) {
                bind_node(st, name, Ns::Var);
            }
            walk_children_except_first_ident(node, st);
            return;
        }
        "variable_declaration" => {
            bind_decl_names(st, node);
            // Walk non-name children for initializer/type refs — but
            // bind_decl_names already recursed; instead walk only
            // children that are not simple_identifier/binding_pattern.
            walk_non_name_children(node, st);
            return;
        }
        "parameter" => {
            // `name: Type` or `name: Type = default`
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if !child.is_named() {
                    continue;
                }
                if child.kind() == "simple_identifier" {
                    bind_node(st, child, Ns::Var);
                } else {
                    walk(child, st);
                }
            }
            return;
        }
        "function_value_parameters" | "lambda_parameters" => {
            walk_children(node, st);
            return;
        }
        "for_statement" => {
            // `for (x in expr) body`: iterable sees outer bindings; the
            // loop variable scopes over the body.
            let mut seen_decl = false;
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if !child.is_named() {
                    continue;
                }
                match child.kind() {
                    "variable_declaration" => {
                        seen_decl = true;
                        // defer binding until after the iterable expr
                    }
                    "control_structure_body" | "block" => {
                        if seen_decl {
                            // find and bind the deferred declaration
                            let mut c2 = node.walk();
                            for d in node.children(&mut c2) {
                                if d.is_named() && d.kind() == "variable_declaration" {
                                    bind_decl_names(st, d);
                                }
                            }
                            seen_decl = false;
                        }
                        push(st, ScopeKind::Block);
                        walk(child, st);
                        st.scopes.pop();
                    }
                    _ => walk(child, st),
                }
            }
            return;
        }
        "catch_block" => {
            // `(e: Exception)`: first simple_identifier is the param.
            let mut bound = false;
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if !child.is_named() {
                    continue;
                }
                if !bound && child.kind() == "simple_identifier" {
                    bind_node(st, child, Ns::Var);
                    bound = true;
                } else {
                    walk(child, st);
                }
            }
            return;
        }
        "call_expression" => {
            walk_call(st, node);
            return;
        }
        "navigation_expression" | "safe_navigation_expression" => {
            walk_navigation(st, node);
            return;
        }
        "simple_identifier" | "identifier" => {
            record_ref(st, text(st.contents, node), node.start_byte()..node.end_byte(), Ns::Var);
            return;
        }
        "type_identifier" => {
            record_ref(st, text(st.contents, node), node.start_byte()..node.end_byte(), Ns::Var);
            return;
        }
        k if BLOCK_KINDS.contains(&k) => {
            push(st, ScopeKind::Block);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "package_header" | "import_list" | "import_header" => {
            return;
        }
        _ => {}
    }
    walk_children(node, st);
}

fn first_named_of_kind<'a>(node: Node<'a>, kinds: &[&str]) -> Option<Node<'a>> {
    node.children(&mut node.walk())
        .find(|c| c.is_named() && kinds.contains(&c.kind()))
}

fn find_child<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    node.children(&mut node.walk())
        .find(|c| c.is_named() && c.kind() == kind)
}

/// After binding a function's name, walk children skipping that first
/// simple_identifier (the name) so it isn't double-counted as a ref.
fn walk_children_except_first_ident(node: Node, st: &mut State) {
    let mut skipped = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        if !skipped && child.kind() == "simple_identifier" {
            skipped = true;
            continue;
        }
        walk(child, st);
    }
}

/// `expr(args)`: callee simple_identifier -> methods; navigation ->
/// member rules; anything else walked.
fn walk_call(st: &mut State, node: Node) {
    let callee = node.child_by_field_name("function")
        .or_else(|| first_named_of_kind(node, &["simple_identifier", "navigation_expression", "call_expression"]));
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        if Some(child.id()) == callee.map(|n| n.id()) {
            match child.kind() {
                "simple_identifier" | "identifier" => {
                    let range = child.start_byte()..child.end_byte();
                    let name = text(st.contents, child);
                    let t = lookup(st, name, Ns::Method).or_else(|| lookup(st, name, Ns::Var));
                    record_ref_target(st, range, t);
                }
                "navigation_expression" | "safe_navigation_expression" => {
                    walk_navigation(st, child);
                }
                _ => walk(child, st),
            }
        } else {
            walk(child, st);
        }
    }
}

/// `a.b` / `a?.b`: the receiver chain is walked; the terminal member
/// name resolves only for `this` receivers (methods in call position,
/// vars otherwise). Other receivers -> unresolved blocker.
fn walk_navigation(st: &mut State, node: Node) {
    // Find the terminal simple_identifier (last one) and the receiver.
    let mut idents = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() && matches!(child.kind(), "simple_identifier" | "identifier") {
            idents.push(child);
        }
    }
    let in_call = node
        .parent()
        .is_some_and(|p| p.kind() == "call_expression");
    // Walk everything except the terminal identifier.
    let terminal = idents.last().copied();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        if Some(child.id()) == terminal.map(|n| n.id()) {
            continue;
        }
        walk(child, st);
    }
    if let Some(t) = terminal {
        let range = t.start_byte()..t.end_byte();
        let name = text(st.contents, t);
        // `this` receiver: first child text == "this".
        let first = node
            .children(&mut node.walk())
            .find(|c| c.is_named());
        let this_recv = first.is_some_and(|f| {
            f.id() != t.id() && text(st.contents, f) == "this"
        });
        if this_recv {
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
        if !child.is_named() || child.id() == skip.id() {
            continue;
        }
        walk(child, st);
    }
}

/// Walk children of a variable_declaration skipping name-ish nodes
/// (already bound by bind_decl_names).
fn walk_non_name_children(node: Node, st: &mut State) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        match child.kind() {
            "simple_identifier" | "identifier" | "binding_pattern_kind"
            | "type_identifier" => {}
            _ => walk(child, st),
        }
    }
}
