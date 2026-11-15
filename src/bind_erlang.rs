//! Erlang binding analysis via tree-sitter CST traversal.
//!
//! Two namespaces: `var` nodes (clause-scoped, single-assignment
//! variables) and `atom` nodes (function names, literals and module
//! atoms — atoms are globally interned so a same-spelled atom is the
//! same entity and renames propagate to literals, which is correct).
//!
//! Scopes: module (function names) → clause (argument and match-bound
//! variables). Each `function_clause` gets its own variable scope;
//! `case`/`receive`/`try` clause patterns bind per-clause. `match_expr`
//! walks the RHS before binding LHS pattern variables, matching
//! Erlang's single-assignment semantics.
//!
//! Records, maps, binaries, comprehensions, `fun` expressions and BIF
//! dispatch are walked generically; unresolved same-name occurrences
//! block renames (`unresolved_is_exact` is false). File-local only.

use std::collections::HashMap;
use std::path::Path;

use tree_sitter::{Node, Parser};

use crate::bind::{Analysis, BindingSite, RefSite};
use crate::lang::LangSpec;

#[derive(Clone, Copy, PartialEq)]
enum ScopeKind {
    Root,
    Clause,
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
    /// Ranges already bound by the prescan.
    bound: std::collections::HashSet<(usize, usize)>,
    /// Nonzero while walking a pattern subtree: `var` leaves are
    /// binders, not references.
    in_pattern: usize,
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
        bound: std::collections::HashSet::new(),
        in_pattern: 0,
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
    bind_keyed(st, name, name.to_string(), range, ns)
}

fn bind_keyed(
    st: &mut State,
    name: &str,
    key: String,
    range: std::ops::Range<usize>,
    ns: Ns,
) {
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
        Ns::Func => &mut st.scopes[si].funcs,
    };
    map.insert(key, idx);
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

/// Function lookup is arity-keyed (`name/arity`): Erlang `helper/1`
/// and `helper/2` are distinct functions. `arity` of `None` matches
/// only when exactly one binding exists for the name.
fn lookup_func(st: &State, name: &str, arity: Option<usize>) -> Option<usize> {
    if let Some(a) = arity {
        let key = format!("{name}/{a}");
        for s in st.scopes.iter().rev() {
            if let Some(&b) = s.funcs.get(&key) {
                return Some(b);
            }
        }
        return None;
    }
    let mut found = None;
    for s in st.scopes.iter().rev() {
        for (k, &b) in &s.funcs {
            let matches_name = k == name
                || k.strip_suffix(|c: char| c.is_ascii_digit())
                    .and_then(|p| p.strip_suffix('/'))
                    == Some(name);
            if matches_name && found.replace(b).is_some() {
                return None;
            }
        }
    }
    found
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

fn record_func_ref(st: &mut State, name: &str, arity: Option<usize>, range: std::ops::Range<usize>) {
    st.analysis.refs.push(RefSite {
        name: range,
        target: lookup_func(st, name, arity),
        expansion: None,
        remote: false,
        module_attr: None,
    });
}

/// Count `args`-field children of an `expr_args` node (call arity).
fn args_arity(args: Node) -> usize {
    let mut n = 0;
    for i in 0..args.child_count() {
        if args.field_name_for_child(i) == Some("args") {
            n += 1;
        }
    }
    n
}

/// Arity of a `call` node: its `args:(expr_args)` child, or 0.
fn call_arity(call: Node) -> usize {
    let mut cursor = call.walk();
    for child in call.children(&mut cursor) {
        if child.is_named() && child.kind() == "expr_args" {
            return args_arity(child);
        }
    }
    0
}

fn bind_node(st: &mut State, node: Node, ns: Ns) {
    bind(
        st,
        text(st.contents, node),
        node.start_byte()..node.end_byte(),
        ns,
    );
}

/// Bind every `var` leaf in a pattern subtree (clause args, match LHS,
/// case/receive clause patterns).
fn bind_pattern_vars(st: &mut State, node: Node) {
    if node.kind() == "var" {
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

/// Walk a pattern subtree recording non-`var` nodes (atoms in
/// patterns are literal atoms → function-namespace refs).
fn walk_pattern_exprs(node: Node, st: &mut State) {
    if node.kind() == "var" {
        return;
    }
    st.in_pattern += 1;
    walk(node, st);
    st.in_pattern -= 1;
}

fn walk_clause(clause: Node, st: &mut State, bind_name: bool) {
    // name:(atom) binds at module level on the first clause; later
    // clauses of the same function reference that binding. Args bind
    // in the clause scope. The key is `name/arity`.
    let arity = clause
        .child_by_field_name("args")
        .map(args_arity)
        .unwrap_or(0);
    if let Some(name) = clause.child_by_field_name("name") {
        let text = text(st.contents, name);
        let range = name.start_byte()..name.end_byte();
        if bind_name && !st.bound.contains(&(range.start, range.end)) {
            bind_keyed(st, text, format!("{text}/{arity}"), range, Ns::Func);
        } else if !bind_name {
            record_func_ref(st, text, Some(arity), range);
        }
    }
    st.scopes.push(Scope {
        kind: ScopeKind::Clause,
        vars: HashMap::new(),
        funcs: HashMap::new(),
    });
    if let Some(args) = clause.child_by_field_name("args") {
        bind_pattern_vars(st, args);
    }
    if let Some(body) = clause.child_by_field_name("body") {
        walk(body, st);
    }
    walk_children_except_fields(st, clause, &["name", "args", "body"]);
    st.scopes.pop();
}

/// Pre-bind every `fun_decl`'s first clause name so forward
/// references (`-export`, behaviour callbacks, earlier clauses calling
/// later-defined functions) resolve instead of blocking.
fn prescan(st: &mut State, node: Node) {
    if node.kind() == "fun_decl" {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.is_named() && child.kind() == "function_clause" {
                let arity = child
                    .child_by_field_name("args")
                    .map(args_arity)
                    .unwrap_or(0);
                if let Some(name) = child.child_by_field_name("name") {
                    let text = text(st.contents, name);
                    let range = name.start_byte()..name.end_byte();
                    if !st.bound.contains(&(range.start, range.end)) {
                        bind_keyed(st, text, format!("{text}/{arity}"), range, Ns::Func);
                    }
                }
                break;
            }
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() {
            prescan(st, child);
        }
    }
}

fn walk(node: Node, st: &mut State) {
    let kind = node.kind();
    match kind {
        "fun_decl" => {
            // All clauses of one `fun_decl` are a single function:
            // bind the name once, later clause names reference it.
            let mut first = true;
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named() && child.kind() == "function_clause" {
                    walk_clause(child, st, first);
                    first = false;
                } else if child.is_named() {
                    walk(child, st);
                }
            }
            return;
        }
        "cr_clause" => {
            // case/receive/try clause: pat binds vars over body.
            st.scopes.push(Scope {
                kind: ScopeKind::Clause,
                vars: HashMap::new(),
                funcs: HashMap::new(),
            });
            if let Some(pat) = node.child_by_field_name("pat") {
                bind_pattern_vars(st, pat);
                walk_pattern_exprs(pat, st);
            }
            if let Some(body) = node.child_by_field_name("body") {
                walk(body, st);
            }
            walk_children_except_fields(st, node, &["pat", "body"]);
            st.scopes.pop();
            return;
        }
        "match_expr" => {
            // `LHS = RHS`: RHS first, then bind LHS pattern vars.
            if let Some(rhs) = node.child_by_field_name("rhs") {
                walk(rhs, st);
            }
            if let Some(lhs) = node.child_by_field_name("lhs") {
                bind_pattern_vars(st, lhs);
                walk_pattern_exprs(lhs, st);
            }
            walk_children_except_fields(st, node, &["lhs", "rhs"]);
            return;
        }
        "var" => {
            if st.in_pattern == 0 {
                record_ref(
                    st,
                    text(st.contents, node),
                    node.start_byte()..node.end_byte(),
                    Ns::Var,
                );
            }
            return;
        }
        "atom" => {
            // Atoms in call names, `fa` export entries and clause names
            // are handled by their parents. Every other atom is a
            // literal (or an unmodeled reference such as `fun M:F/A`)
            // — record an unresolved blocker so a same-name function
            // rename cannot silently miss it, but never edit it.
            let parent = node.parent();
            let special = parent.is_some_and(|p| {
                matches!(p.kind(), "call" | "fa" | "function_clause" | "remote_module")
                    || p.kind() == "fun_decl"
            });
            if !special {
                st.analysis.refs.push(RefSite {
                    name: node.start_byte()..node.end_byte(),
                    target: None,
                    expansion: None,
                    remote: false,
                    module_attr: None,
                });
            }
            return;
        }
        "call" => {
            // `expr:(atom)` is the function name; record with arity.
            let mut skip_expr = false;
            if let Some(expr) = node.child_by_field_name("expr")
                && expr.kind() == "atom"
            {
                let name = text(st.contents, expr);
                record_func_ref(
                    st,
                    name,
                    Some(call_arity(node)),
                    expr.start_byte()..expr.end_byte(),
                );
                skip_expr = true;
            }
            walk_children_except_fields(st, node, if skip_expr { &["expr"] } else { &[] });
            return;
        }
        "fa" => {
            // `-export([name/arity])` entries.
            if let Some(fun) = node.child_by_field_name("fun")
                && fun.kind() == "atom"
            {
                let arity = node
                    .child_by_field_name("arity")
                    .and_then(|a| {
                        let mut c = a.walk();
                        a.children(&mut c).find(|ch| ch.is_named())
                    })
                    .and_then(|n| text(st.contents, n).parse::<usize>().ok());
                let name = text(st.contents, fun);
                record_func_ref(st, name, arity, fun.start_byte()..fun.end_byte());
            }
            walk_children_except_fields(st, node, &["fun", "arity"]);
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
