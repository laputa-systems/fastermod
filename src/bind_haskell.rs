//! Haskell binding analysis via tree-sitter CST traversal.
//!
//! The grammar uses `variable` for term-level identifiers (functions
//! and locals share one namespace) and `name` for type/constructor
//! names — kept as a second namespace. `signature`/`function` pairs
//! declare module-level names; `bind` nodes (let/where) bind locals.
//!
//! Scopes: module → function (patterns + match) → let/lambda/case
//! bodies. Haskell `let`/`where` are recursive; pattern variables in
//! `case` alternatives and lambda binders scope over their bodies.
//! `do`-block `let` bindings are modeled at do-body granularity
//! (slightly wider than Haskell's rest-of-do scoping — documented
//! limitation).
//!
//! Qualified names (`M.f`), record fields, `QualifiedDo`, arrows in
//! types, and Template Haskell are only partially modeled —
//! `unresolved_is_exact` is false so unresolved same-name occurrences
//! block renames. All bindings are file-local.

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
}

struct Scope {
    kind: ScopeKind,
    vars: HashMap<String, usize>,
    tys: HashMap<String, usize>,
}

struct State<'a> {
    analysis: Analysis,
    scopes: Vec<Scope>,
    contents: &'a [u8],
    bound: std::collections::HashSet<(usize, usize)>,
}

fn text<'a>(contents: &'a [u8], node: Node) -> &'a str {
    node.utf8_text(contents).unwrap_or("")
}

#[derive(Clone, Copy, PartialEq)]
enum Ns {
    Var,
    Ty,
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
            tys: HashMap::new(),
        }],
        contents: contents.as_bytes(),
        bound: std::collections::HashSet::new(),
    };
    prescan_module(&mut st, root);
    walk(root, &mut st);
    st.analysis.unresolved_is_exact = false;
    Some(st.analysis)
}

fn cur(st: &State) -> usize {
    st.scopes.len() - 1
}

fn bind(st: &mut State, name: &str, range: std::ops::Range<usize>, ns: Ns) {
    st.bound.insert((range.start, range.end));
    let kind = st.scopes[cur(st)].kind;
    let module_level = kind == ScopeKind::Root;
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
        Ns::Ty => &mut st.scopes[si].tys,
    };
    map.insert(name.to_string(), idx);
}

fn lookup(st: &State, name: &str, ns: Ns) -> Option<usize> {
    for s in st.scopes.iter().rev() {
        let map = match ns {
            Ns::Var => &s.vars,
            Ns::Ty => &s.tys,
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

fn bind_node(st: &mut State, node: Node, ns: Ns) {
    bind(
        st,
        text(st.contents, node),
        node.start_byte()..node.end_byte(),
        ns,
    );
}

/// Pre-bind module-level declaration names so that `signature`
/// occurrences (which precede their `function`/`bind` definitions)
/// resolve instead of blocking renames.
fn prescan_module(st: &mut State, root: Node) {
    let mut cursor = root.walk();
    for decls in root.children(&mut cursor) {
        if !decls.is_named() || decls.kind() != "declarations" {
            continue;
        }
        let mut c2 = decls.walk();
        for d in decls.children(&mut c2) {
            if !d.is_named() {
                continue;
            }
            match d.kind() {
                "function" | "bind" => {
                    if let Some(name) = d.child_by_field_name("name") {
                        bind_node(st, name, Ns::Var);
                    }
                }
                "data_declaration" | "newtype_declaration"
                | "type_synonym_declaration" => {
                    if let Some(name) = first_name_child(d) {
                        bind_node(st, name, Ns::Ty);
                    }
                }
                _ => {}
            }
        }
    }
}

fn first_name_child<'a>(node: Node<'a>) -> Option<Node<'a>> {
    node.children(&mut node.walk())
        .find(|c| c.is_named() && c.kind() == "name")
}

fn push(st: &mut State, kind: ScopeKind) {
    st.scopes.push(Scope {
        kind,
        vars: HashMap::new(),
        tys: HashMap::new(),
    });
}

/// Bind every `variable` leaf inside a pattern subtree (function
/// patterns, let binds handled separately, case/lambda patterns).
fn bind_pattern_vars(st: &mut State, node: Node) {
    if node.kind() == "variable" {
        bind_node(st, node, Ns::Var);
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() {
            bind_pattern_vars(st, child);
        }
    }
}

fn walk(node: Node, st: &mut State) {
    let kind = node.kind();
    match kind {
        "function" => {
            // name:(variable) binds at module level; patterns bind in
            // the function scope covering the match.
            if let Some(name) = node.child_by_field_name("name")
                && !st
                    .bound
                    .contains(&(name.start_byte(), name.end_byte()))
            {
                bind_node(st, name, Ns::Var);
            }
            push(st, ScopeKind::Fn);
            if let Some(pats) = node.child_by_field_name("patterns") {
                bind_pattern_vars(st, pats);
            }
            if let Some(m) = node.child_by_field_name("match") {
                walk(m, st);
            }
            walk_children_except_fields(st, node, &["name", "patterns", "match"]);
            st.scopes.pop();
            return;
        }
        "signature" => {
            // Type signature: name is a ref to the function, not a new
            // binding — walk normally (variable arm records the ref).
            walk_children(node, st);
            return;
        }
        "bind" => {
            // let/where binding: name:(variable), match: expr.
            // Match first? Haskell let is recursive — order irrelevant
            // for name resolution; walk match then bind name.
            if let Some(m) = node.child_by_field_name("match") {
                walk(m, st);
            }
            if let Some(name) = node.child_by_field_name("name")
                && !st
                    .bound
                    .contains(&(name.start_byte(), name.end_byte()))
            {
                bind_node(st, name, Ns::Var);
            }
            walk_children_except_fields(st, node, &["name", "match"]);
            return;
        }
        "let_in" => {
            // `let ... in ...`: bindings and body share one scope
            // (Haskell let is recursive). `local_binds` is walked
            // inline so its bindings land in this scope, not a
            // nested one that would pop before the body.
            push(st, ScopeKind::Block);
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if !child.is_named() {
                    continue;
                }
                if child.kind() == "local_binds" {
                    walk_children(child, st);
                } else {
                    walk(child, st);
                }
            }
            st.scopes.pop();
            return;
        }
        "where" | "decls" => {
            // let/where bodies: bindings share one scope with the body.
            push(st, ScopeKind::Block);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "do" => {
            push(st, ScopeKind::Block);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "lambda" | "case_alternative" => {
            push(st, ScopeKind::Block);
            // Pattern variables bind over the body.
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if !child.is_named() {
                    continue;
                }
                match child.kind() {
                    "patterns" | "pattern" => bind_pattern_vars(st, child),
                    _ => walk(child, st),
                }
            }
            st.scopes.pop();
            return;
        }
        "apply" => {
            if let Some(f) = node.child_by_field_name("function") {
                walk(f, st);
            }
            if let Some(a) = node.child_by_field_name("argument") {
                walk(a, st);
            }
            walk_children_except_fields(st, node, &["function", "argument"]);
            return;
        }
        "variable" => {
            record_ref(st, text(st.contents, node), node.start_byte()..node.end_byte(), Ns::Var);
            return;
        }
        "name" => {
            record_ref(st, text(st.contents, node), node.start_byte()..node.end_byte(), Ns::Ty);
            return;
        }
        "data_declaration" | "newtype_declaration" | "type_synonym_declaration" => {
            // `data Foo ...` / `type T = ...`: the type name binds in
            // the types namespace.
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if !child.is_named() {
                    continue;
                }
                if child.kind() == "name" {
                    let is_first = node
                        .children(&mut node.walk())
                        .find(|c| c.is_named())
                        .is_some_and(|c| c.id() == child.id());
                    if is_first
                        && !st
                            .bound
                            .contains(&(child.start_byte(), child.end_byte()))
                    {
                        bind_node(st, child, Ns::Ty);
                        continue;
                    }
                }
                walk(child, st);
            }
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
