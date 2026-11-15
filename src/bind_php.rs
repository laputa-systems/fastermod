//! PHP binding analysis via tree-sitter CST traversal.
//!
//! PHP variables are *function-scoped*: `if`/`foreach`/`for` bodies do
//! not create scopes, and an assignment to an unbound name implicitly
//! declares it in the current function (or program) scope. Scopes are
//! therefore only: program, class member (properties, methods, class
//! constants), and function-like (declaration, method, anonymous
//! function, arrow function).
//!
//! Three namespaces are kept distinct:
//! - `$var` bindings (`vars`) — reachable only as `variable_name`.
//! - named bindings (`named`) — functions, classes, interfaces, traits,
//!   enums, global constants; reached by `name`-kind references.
//! - member bindings (`members`) — properties, methods, class constants;
//!   reached only through `->` or `self::`/`static::`/`parent::`.
//!
//! Boundaries: variable-variables (`$$x`), `extract`/`compact`, dynamic
//! calls, and unknown-receiver member access are invisible or
//! unresolvable — `unresolved_is_exact` is false so unresolved same-name
//! occurrences block renames. Cross-file `require`/`include` resolution
//! is not implemented; module-level symbols block on sibling
//! occurrences.

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
}

struct Scope {
    kind: ScopeKind,
    vars: HashMap<String, usize>,
    named: HashMap<String, usize>,
    members: HashMap<String, usize>,
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
    "interface_declaration",
    "trait_declaration",
    "enum_declaration",
];

const FN_KINDS: &[&str] = &[
    "function_definition",
    "method_declaration",
    "anonymous_function",
    "arrow_function",
];

#[derive(Clone, Copy, PartialEq)]
enum Ns {
    Var,
    Named,
    Member,
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
            named: HashMap::new(),
            members: HashMap::new(),
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
        Ns::Named => &mut st.scopes[si].named,
        Ns::Member => &mut st.scopes[si].members,
    };
    map.insert(name.to_string(), idx);
}

/// Innermost variable-holding scope (skips Member scopes, which hold
/// only properties/methods/constants).
fn var_scope(st: &State) -> usize {
    st.scopes
        .iter()
        .rposition(|s| s.kind != ScopeKind::Member)
        .unwrap_or(0)
}

fn lookup(st: &State, name: &str, ns: Ns) -> Option<usize> {
    for s in st.scopes.iter().rev() {
        let map = match ns {
            Ns::Var => {
                if s.kind == ScopeKind::Member {
                    continue;
                }
                &s.vars
            }
            Ns::Named => &s.named,
            Ns::Member => &s.members,
        };
        if let Some(&b) = map.get(name) {
            return Some(b);
        }
    }
    None
}

/// PHP functions do not see outer function variables: a `$x` read or
/// write inside a function resolves only within that function's own
/// scope (or the program scope at top level).
fn lookup_local_var(st: &State, name: &str) -> Option<usize> {
    let si = var_scope(st);
    st.scopes[si].vars.get(name).copied()
}

/// `use ($x)` capture: resolves through enclosing function scopes.
fn lookup_outer_var(st: &State, name: &str) -> Option<usize> {
    lookup(st, name, Ns::Var)
}

/// `global $x` refers to the program (root) scope only.
fn lookup_root_var(st: &State, name: &str) -> Option<usize> {
    st.scopes[0].vars.get(name).copied()
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

fn record_ref_target(
    st: &mut State,
    range: std::ops::Range<usize>,
    target: Option<usize>,
) {
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

/// The `name` child range/text of a `variable_name` node (`$x` -> `x`).
fn var_name(st: &State, node: Node) -> Option<(String, std::ops::Range<usize>)> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "name" {
            return Some((
                text(st.contents, child).to_string(),
                child.start_byte()..child.end_byte(),
            ));
        }
    }
    None
}

fn push(st: &mut State, kind: ScopeKind) {
    st.scopes.push(Scope {
        kind,
        vars: HashMap::new(),
        named: HashMap::new(),
        members: HashMap::new(),
    });
}

/// Assignment target handling: an already-bound name is a reference; an
/// unbound name is implicitly declared by the assignment (PHP semantics).
fn assign_target(st: &mut State, node: Node) {
    match node.kind() {
        "variable_name" => {
            if let Some((name, range)) = var_name(st, node) {
                if let Some(b) = lookup_local_var(st, &name) {
                    record_ref_target(st, range, Some(b));
                } else {
                    // Implicit declaration lands in the innermost
                    // function/program scope — never an outer one.
                    let si = var_scope(st);
                    let module_level = matches!(st.scopes[si].kind, ScopeKind::Root);
                    let idx = st.analysis.bindings.len();
                    st.analysis.bindings.push(BindingSite {
                        name: range,
                        text: name.clone(),
                        expansion: None,
                        module_level,
                        is_mod: false,
                    });
                    st.scopes[si].vars.insert(name, idx);
                }
            }
        }
        "list_literal" | "array_creation_expression" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named() {
                    assign_target(st, child);
                }
            }
        }
        _ => walk(node, st),
    }
}

fn walk(node: Node, st: &mut State) {
    let kind = node.kind();
    if TYPE_KINDS.contains(&kind) {
        if let Some(name) = node.child_by_field_name("name") {
            bind(st, text(st.contents, name), name.start_byte()..name.end_byte(), Ns::Named);
        }
        for i in 0..node.child_count() {
            let child = node.child(i).unwrap();
            if !child.is_named() {
                continue;
            }
            if node.field_name_for_child(i) == Some("name") {
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
    if FN_KINDS.contains(&kind) {
        if kind == "method_declaration" {
            if let Some(name) = node.child_by_field_name("name") {
                bind(st, text(st.contents, name), name.start_byte()..name.end_byte(), Ns::Member);
            }
        } else if let Some(name) = node.child_by_field_name("name") {
            bind(st, text(st.contents, name), name.start_byte()..name.end_byte(), Ns::Named);
        }
        push(st, ScopeKind::Fn);
        walk_children_except_fields(node, st, &["name"]);
        st.scopes.pop();
        return;
    }
    match kind {
        "declaration_list" => {
            push(st, ScopeKind::Member);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "property_element" => {
            if let Some(name) = node.child_by_field_name("name")
                && name.kind() == "variable_name"
                && let Some((n, range)) = var_name(st, name)
            {
                bind(st, &n, range, Ns::Member);
            }
            walk_children_except(node, st, &["name"]);
            return;
        }
        "simple_parameter" | "variadic_parameter" => {
            if let Some(name) = node.child_by_field_name("name")
                && name.kind() == "variable_name"
                && let Some((n, range)) = var_name(st, name)
            {
                bind(st, &n, range, Ns::Var);
            }
            walk_children_except(node, st, &["name"]);
            return;
        }
        "const_declaration" | "class_constant_declaration" => {
            // `const FOO = 1`: top-level -> named; inside a class body the
            // enclosing Member scope routes it to members.
            let member = st.scopes[cur(st)].kind == ScopeKind::Member;
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if !child.is_named() {
                    continue;
                }
                match child.kind() {
                    "name" => {
                        bind(st, text(st.contents, child), child.start_byte()..child.end_byte(), if member { Ns::Member } else { Ns::Named });
                    }
                    _ => walk(child, st),
                }
            }
            return;
        }
        "assignment_expression" => {
            // RHS first: `$x = $x` reads the pre-assignment binding.
            if let Some(right) = node.child_by_field_name("right") {
                walk(right, st);
            }
            if let Some(left) = node.child_by_field_name("left") {
                assign_target(st, left);
            }
            walk_children_except_fields(node, st, &["left", "right"]);
            return;
        }
        "foreach_statement" => {
            // First named child is the iterated expression (a ref); the
            // rest are loop variables bound in the current (function)
            // scope — PHP has no block scoping.
            let mut first = true;
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if !child.is_named() {
                    continue;
                }
                if first {
                    first = false;
                    walk(child, st);
                    continue;
                }
                match child.kind() {
                    "pair" => {
                        let mut c2 = child.walk();
                        for p in child.children(&mut c2) {
                            if p.is_named() {
                                assign_target(st, p);
                            }
                        }
                    }
                    "variable_name" => assign_target(st, child),
                    _ => walk(child, st),
                }
            }
            return;
        }
        "static_variable_declaration" => {
            if let Some(value) = node.child_by_field_name("value") {
                walk(value, st);
            }
            if let Some(name) = node.child_by_field_name("name")
                && name.kind() == "variable_name"
                && let Some((n, range)) = var_name(st, name)
            {
                bind(st, &n, range, Ns::Var);
            }
            walk_children_except_fields(node, st, &["name", "value"]);
            return;
        }
        "anonymous_function_use_clause" => {
            // `use ($x)`: captures resolve through enclosing scopes.
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if !child.is_named() {
                    continue;
                }
                if child.kind() == "variable_name"
                    && let Some((name, range)) = var_name(st, child)
                {
                    record_ref_target(st, range, lookup_outer_var(st, &name));
                } else {
                    walk(child, st);
                }
            }
            return;
        }
        "global_declaration" => {
            // `global $x` refers to the program scope.
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named()
                    && child.kind() == "variable_name"
                    && let Some((name, range)) = var_name(st, child)
                {
                    record_ref_target(st, range, lookup_root_var(st, &name));
                }
            }
            return;
        }
        "variable_name" => {
            // Read context (default): resolves only within the innermost
            // function/program scope — PHP functions do not capture.
            if let Some((name, range)) = var_name(st, node) {
                record_ref_target(st, range, lookup_local_var(st, &name));
            }
            return;
        }
        "member_access_expression" | "nullsafe_member_access_expression"
        | "member_call_expression" | "nullsafe_member_call_expression" => {
            // `$this->x` resolves the member name; any other receiver is
            // unknown -> unresolved blocker.
            let object = node.child_by_field_name("object");
            let name = node.child_by_field_name("name");
            let this_recv = object.is_some_and(|o| text(st.contents, o) == "$this");
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if !child.is_named() {
                    continue;
                }
                if Some(child.id()) == name.map(|n| n.id()) {
                    if let Some(n) = name {
                        let range = n.start_byte()..n.end_byte();
                        if this_recv {
                            record_ref(st, text(st.contents, n), range, Ns::Member);
                        } else {
                            record_unresolved(st, range);
                        }
                    }
                } else {
                    walk(child, st);
                }
            }
            return;
        }
        "name" => {
            // Function/constant/class-name references. A `name` under a
            // scoped-access parent resolves only for self/static/parent;
            // anything else is an unknown receiver -> unresolved.
            let range = node.start_byte()..node.end_byte();
            let scoped_self = node.parent().is_some_and(|p| {
                p.kind().contains("scoped")
                    && p
                        .children(&mut p.walk())
                        .find(|c| c.is_named())
                        .is_some_and(|c| matches!(text(st.contents, c), "self" | "static" | "parent"))
            });
            if scoped_self {
                record_ref(st, text(st.contents, node), range, Ns::Member);
            } else if node.parent().is_some_and(|p| {
                p.kind().contains("scoped") || p.kind().contains("member")
            }) {
                record_unresolved(st, range);
            } else {
                record_ref(st, text(st.contents, node), range, Ns::Named);
            }
            return;
        }
        "dynamic_variable_name" => {
            // `$$x`: the inner variable_name is an ordinary ref; the
            // resulting name is dynamic and unbindable.
            walk_children(node, st);
            return;
        }
        _ => {}
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

fn walk_children_except(node: Node, st: &mut State, fields: &[&str]) {
    walk_children_except_fields(node, st, fields);
}

fn walk_children_except_fields(node: Node, st: &mut State, skip: &[&str]) {
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

