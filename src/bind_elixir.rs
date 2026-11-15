//! Elixir binding analysis via tree-sitter CST traversal.
//!
//! The Elixir grammar represents almost everything as `call` nodes:
//! `def name(args) do ... end` is `call(target: def, arguments:
//! [call(target: name, arguments: params)], do_block: body)`.
//!
//! Scopes: source → defmodule do_block (member) → def do_block
//! (function) → do_block of if/case/cond/fn/for/with (block). `=`
//! match expressions bind identifiers on the left after walking the
//! right side; `<-` generators bind their left side. `def`/`defp`/
//! `defmacro` names bind as methods; zero-arity defs bind a bare
//! identifier argument.
//!
//! `Mod.fun` dot-calls record the remote name unresolved (module
//! identity is not modeled), blocking same-name renames.
//! `unresolved_is_exact` is false: macros, `apply`/`send`, sigils and
//! `unquote` fragments are only partially modeled. File-local only.

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

#[derive(Clone, Copy, PartialEq)]
enum Ns {
    Var,
    Method,
}

const DEF_TARGETS: &[&str] = &["def", "defp", "defmacro", "defmacrop"];
const COMPREHENSION_TARGETS: &[&str] = &["for", "with", "into"];

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

/// The `target:` identifier text of a call node, if any.
fn call_target<'a>(st: &State<'a>, node: Node) -> Option<&'a str> {
    node.child_by_field_name("target")
        .filter(|t| t.kind() == "identifier")
        .map(|t| text(st.contents, t))
}

/// First `arguments` child of a call (a node kind, not a field).
fn call_arguments<'a>(node: Node<'a>) -> Option<Node<'a>> {
    node.children(&mut node.walk())
        .find(|c| c.is_named() && c.kind() == "arguments")
}

/// `do_block` child of a call.
fn call_do_block<'a>(node: Node<'a>) -> Option<Node<'a>> {
    node.children(&mut node.walk())
        .find(|c| c.is_named() && c.kind() == "do_block")
}

fn walk(node: Node, st: &mut State) {
    let kind = node.kind();
    match kind {
        "call" => {
            walk_call(st, node);
            return;
        }
        "do_block" => {
            // Block-scoped body (if/case/cond/fn/for/with bodies and
            // def bodies reach here through their call handlers which
            // push the right scope first — a bare do_block is a block).
            push(st, ScopeKind::Block);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "binary_operator" => {
            walk_binary_dispatch(st, node);
            return;
        }
        "identifier" => {
            let range = node.start_byte()..node.end_byte();
            let name = text(st.contents, node);
            let t = lookup(st, name, Ns::Var).or_else(|| lookup(st, name, Ns::Method));
            record_ref_target(st, range, t);
            return;
        }
        "dot" => {
            // `left.right`: remote call/member — right name unresolved.
            let right = node.child_by_field_name("right");
            let left = node.child_by_field_name("left");
            if let Some(l) = left {
                walk(l, st);
            }
            if let Some(r) = right {
                record_unresolved(st, r.start_byte()..r.end_byte());
            }
            return;
        }
        "alias" | "atom" | "string" | "sigil" => {
            return;
        }
        _ => {}
    }
    walk_children(node, st);
}

fn walk_call(st: &mut State, node: Node) {
    let target = call_target(st, node).unwrap_or("");
    match target {
        t if DEF_TARGETS.contains(&t) => {
            // def name(args) do body end — name is first argument call's
            // target (or bare identifier for zero-arity defs).
            let args = call_arguments(node);
            let mut name_node = None;
            let mut param_holder = None;
            if let Some(a) = args {
                let mut c = a.walk();
                for child in a.children(&mut c) {
                    if !child.is_named() {
                        continue;
                    }
                    match child.kind() {
                        "call" => {
                            name_node = child.child_by_field_name("target");
                            param_holder = child.child_by_field_name("arguments");
                            break;
                        }
                        "identifier" => {
                            name_node = Some(child);
                            break;
                        }
                        _ => {}
                    }
                }
            }
            if let Some(n) = name_node {
                bind_node(st, n, Ns::Method);
            }
            push(st, ScopeKind::Fn);
            if let Some(p) = param_holder {
                bind_params(st, p);
            }
            if let Some(db) = call_do_block(node) {
                // def body: function scope already pushed; do_block adds
                // a block scope on top — harmless.
                walk(db, st);
            }
            st.scopes.pop();
            return;
        }
        "defmodule" | "defprotocol" => {
            // Module body: member scope for defs.
            if let Some(db) = call_do_block(node) {
                push(st, ScopeKind::Member);
                walk(db, st);
                st.scopes.pop();
            }
            if let Some(a) = call_arguments(node) {
                walk_children(a, st);
            }
            return;
        }
        t if COMPREHENSION_TARGETS.contains(&t) => {
            // for/with generators bind via `<-` binary_operators;
            // whole comprehension shares one block scope.
            push(st, ScopeKind::Block);
            if let Some(a) = call_arguments(node) {
                walk(a, st);
            }
            if let Some(db) = call_do_block(node) {
                walk(db, st);
            }
            st.scopes.pop();
            return;
        }
        "fn" => {
            // Anonymous fn clauses: params then body, block scope.
            push(st, ScopeKind::Block);
            if let Some(a) = call_arguments(node) {
                bind_params(st, a);
            }
            if let Some(db) = call_do_block(node) {
                walk(db, st);
            }
            st.scopes.pop();
            return;
        }
        _ => {}
    }
    // Ordinary call: target identifier -> vars-then-methods (local
    // calls); dot targets handled by their own arm via children.
    if let Some(t) = node.child_by_field_name("target") {
        match t.kind() {
            "identifier" => {
                let range = t.start_byte()..t.end_byte();
                let name = text(st.contents, t);
                let tgt = lookup(st, name, Ns::Var).or_else(|| lookup(st, name, Ns::Method));
                record_ref_target(st, range, tgt);
            }
            _ => walk(t, st),
        }
    }
    if let Some(a) = call_arguments(node) {
        walk(a, st);
    }
    // do_block on non-special calls (if/case/cond/receive bodies).
    if let Some(db) = call_do_block(node) {
        push(st, ScopeKind::Block);
        walk_children(db, st);
        st.scopes.pop();
    }
    // Other children (e.g. stab clauses).
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        if child.is_named()
            && Some(child.id()) != node.child_by_field_name("target").map(|n| n.id())
            && Some(child.id()) != call_arguments(node).map(|n| n.id())
            && Some(child.id()) != call_do_block(node).map(|n| n.id())
        {
            walk(child, st);
        }
    }
}

/// `x = expr` binds left-side identifiers (after RHS); `<-`
/// generators bind their left side. Other operators are plain refs.
fn walk_binary(st: &mut State, node: Node) -> bool {
    let op = node.child_by_field_name("operator");
    let op_text = op.map(|o| text(st.contents, o)).unwrap_or("");
    let left = node.child_by_field_name("left");
    let right = node.child_by_field_name("right");
    match op_text {
        "=" => {
            if let Some(r) = right {
                walk(r, st);
            }
            if let Some(l) = left {
                bind_match_pattern(st, l);
            }
            return true;
        }
        "<-" => {
            if let Some(r) = right {
                walk(r, st);
            }
            if let Some(l) = left {
                bind_match_pattern(st, l);
            }
            return true;
        }
        _ => {}
    }
    false
}

/// Bind identifiers inside a match/generator left-hand pattern.
/// `^x` pins are references, not bindings — handled conservatively by
/// walking them as refs when encountered as `pin` nodes.
fn bind_match_pattern(st: &mut State, node: Node) {
    match node.kind() {
        "identifier" => {
            bind_node(st, node, Ns::Var);
        }
        "pin" => {
            walk_children(node, st);
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named() {
                    bind_match_pattern(st, child);
                }
            }
        }
    }
}

fn bind_params(st: &mut State, args: Node) {
    // arguments children: identifiers are param names; calls with
    // defaults `x \\ default` appear as binary_operator — bind left.
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        match child.kind() {
            "identifier" => bind_node(st, child, Ns::Var),
            "binary_operator" => {
                if let Some(l) = child.child_by_field_name("left") {
                    bind_match_pattern(st, l);
                }
                if let Some(r) = child.child_by_field_name("right") {
                    walk(r, st);
                }
            }
            _ => walk(child, st),
        }
    }
}

fn walk_binary_dispatch(st: &mut State, node: Node) {
    if !walk_binary(st, node) {
        walk_children(node, st);
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
