//! Julia binding analysis via tree-sitter CST traversal.
//!
//! Namespaces: `identifier` leaves split into function names (method
//! namespace) and variables. Julia methods are overloads — every
//! `function_definition` with the same name shares ONE binding, so a
//! rename updates all overloads and call sites together.
//!
//! Scopes: module → function body → for/while/let/try blocks.
//! `for_binding` binds the loop variable; `assignment` binds its LHS
//! identifiers after walking the RHS (`x = x` sees the outer binding);
//! `compound_assignment` (`+=` etc.) LHS is a reference, not a binding.
//!
//! Qualified calls (`obj.f()`), macros, multiple dispatch on argument
//! types, `global`/`nonlocal` and generated functions are only
//! partially modeled — `unresolved_is_exact` is false so unresolved
//! same-name occurrences block renames. File-local only.

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
    funcs: HashMap<String, usize>,
}

struct State<'a> {
    analysis: Analysis,
    scopes: Vec<Scope>,
    contents: &'a [u8],
    /// Function names already bound (overloads share one binding).
    bound_funcs: std::collections::HashSet<String>,
}

fn text<'a>(contents: &'a [u8], node: Node) -> &'a str {
    node.utf8_text(contents).unwrap_or("")
}

#[derive(Clone, Copy, PartialEq)]
enum Ns {
    Var,
    Func,
}

pub(crate) fn analyze(
    spec: &LangSpec,
    contents: &str,
    _path: &Path,
) -> Option<Analysis> {
    let language = spec.grammar.ts_language();
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
            funcs: HashMap::new(),
        }],
        contents: contents.as_bytes(),
        bound_funcs: std::collections::HashSet::new(),
    };
    prescan(&mut st, root);
    walk(root, &mut st);
    st.analysis.unresolved_is_exact = false;
    Some(st.analysis)
}

fn cur(st: &State) -> usize {
    st.scopes.len() - 1
}

fn bind(st: &mut State, name: &str, range: std::ops::Range<usize>, ns: Ns) {
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
        Ns::Func => &mut st.scopes[si].funcs,
    };
    map.insert(name.to_string(), idx);
}

fn lookup(st: &State, name: &str, ns: Ns) -> Option<usize> {
    for s in st.scopes.iter().rev() {
        let map = match ns {
            Ns::Var => &s.vars,
            Ns::Func => &s.funcs,
        };
        if let Some(&b) = map.get(name) {
            return Some(b);
        }
    }
    None
}

fn record_ref(st: &mut State, name: &str, range: std::ops::Range<usize>, ns: Ns) {
    st.analysis.refs.push(RefSite {
        name: range,
        target: lookup(st, name, ns),
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

/// First `identifier` descendant (function name position).
/// The function name + parameter identifiers of a
/// `function_definition`'s `signature` (a `call_expression`).
fn signature_parts(sig: Node) -> (Option<Node>, Vec<Node>) {
    // `signature` wraps a `call_expression`: name identifier first,
    // parameters inside `argument_list`.
    let call = if sig.kind() == "signature" {
        child_of_kind(sig, "call_expression").unwrap_or(sig)
    } else {
        sig
    };
    let mut name = None;
    let mut params = Vec::new();
    let mut cursor = call.walk();
    for child in call.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        match child.kind() {
            "identifier" if name.is_none() => name = Some(child),
            "argument_list" => {
                let mut c2 = child.walk();
                for p in child.children(&mut c2) {
                    if p.is_named() && p.kind() == "identifier" {
                        params.push(p);
                    }
                }
            }
            _ => {}
        }
    }
    (name, params)
}

/// Pre-bind all function names so calls preceding definitions resolve.
fn prescan(st: &mut State, node: Node) {
    if node.kind() == "function_definition"
        && let Some(sig) = child_of_kind(node, "signature")
        && let (Some(name), _) = signature_parts(sig)
    {
        let text = text(st.contents, name);
        if st.bound_funcs.insert(text.to_string()) {
            bind_node(st, name, Ns::Func);
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() {
            prescan(st, child);
        }
    }
}

/// Bind every `identifier` leaf in an assignment LHS (destructuring).
fn bind_lhs(st: &mut State, node: Node) {
    if node.kind() == "identifier" {
        bind_node(st, node, Ns::Var);
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() {
            bind_lhs(st, child);
        }
    }
}

fn push(st: &mut State, kind: ScopeKind) {
    st.scopes.push(Scope {
        kind,
        vars: HashMap::new(),
        funcs: HashMap::new(),
    });
}

fn child_of_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    node.children(&mut node.walk())
        .find(|c| c.is_named() && c.kind() == kind)
}

fn named_children(node: Node) -> Vec<Node> {
    node.children(&mut node.walk())
        .filter(|c| c.is_named())
        .collect()
}

fn walk_function(node: Node, st: &mut State) {
    if let Some(sig) = child_of_kind(node, "signature") {
        let (name, params) = signature_parts(sig);
        push(st, ScopeKind::Fn);
        for p in params {
            bind_node(st, p, Ns::Var);
        }
        for child in named_children(node) {
            if child.id() != sig.id() {
                walk(child, st);
            }
        }
        if let Some(name) = name {
            // Every definition name references the shared overload
            // binding created by the prescan.
            record_ref(st, text(st.contents, name), name.start_byte()..name.end_byte(), Ns::Func);
        }
        st.scopes.pop();
        return;
    }
    walk_children(node, st);
}

fn walk(node: Node, st: &mut State) {
    let kind = node.kind();
    match kind {
        "function_definition" => {
            walk_function(node, st);
            return;
        }
        "for_statement" | "while_statement" | "let_block" | "try_statement" => {
            push(st, ScopeKind::Block);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "for_binding" => {
            // First child is the loop variable/pattern; the range
            // expression is walked normally.
            let kids = named_children(node);
            if let Some(&first) = kids.first() {
                bind_lhs(st, first);
            }
            for &k in &kids[1..] {
                walk(k, st);
            }
            return;
        }
        "assignment" => {
            // Children: LHS, operator, RHS. RHS first so `x = x`
            // sees the outer binding, then bind LHS identifiers.
            let kids = named_children(node);
            if let Some(&rhs) = kids.last() {
                walk(rhs, st);
            }
            if let Some(&lhs) = kids.first() {
                bind_lhs(st, lhs);
            }
            for &k in &kids[1..kids.len().saturating_sub(1)] {
                walk(k, st);
            }
            return;
        }
        "compound_assignment_expression" => {
            // `x += y`: first child is a reference, not a binding.
            if let Some(&lhs) = named_children(node).first() {
                walk(lhs, st);
            }
            let kids = named_children(node);
            for &k in &kids[1..] {
                walk(k, st);
            }
            return;
        }
        "call_expression" => {
            // First identifier child is the function name.
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if !child.is_named() {
                    continue;
                }
                if child.kind() == "identifier" {
                    record_ref(
                        st,
                        text(st.contents, child),
                        child.start_byte()..child.end_byte(),
                        Ns::Func,
                    );
                    continue;
                }
                walk(child, st);
            }
            return;
        }
        "identifier" => {
            record_ref(st, text(st.contents, node), node.start_byte()..node.end_byte(), Ns::Var);
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


