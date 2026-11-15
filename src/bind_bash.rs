//! Bash binding analysis via tree-sitter CST traversal.
//!
//! Bash has two namespaces: command names (`word` under `command_name`,
//! bound by `function_definition`) and variables (`variable_name`,
//! bound by `variable_assignment`/`declaration_command`/`for`).
//!
//! Scoping is deliberately minimal and matches shell semantics: a
//! single global scope plus one scope per function body. `if`/`for`/
//! `while`/`do_group`/subshell bodies do NOT create variable scopes —
//! bash has no block scoping. `local`/`declare`/`typeset` assignments
//! bind in the nearest function scope; bare assignments bind globally
//! unless the name already resolves (then they are references to the
//! existing binding).
//!
//! `$x`/`${x}` expansions record variable refs; positional/special
//! parameters (`$1`, `$@`, `$?`) are separate node kinds and ignored.
//! `eval`, indirect expansion `${!x}`, `declare -n` namerefs, `trap`
//! strings, and aliases are only partially modeled —
//! `unresolved_is_exact` is false so unresolved same-name occurrences
//! block renames.

use std::collections::HashMap;
use std::path::Path;

use ast_grep_core::tree_sitter::LanguageExt;
use tree_sitter::{Node, Parser};

use crate::bind::{Analysis, BindingSite, RefSite};
use crate::lang::LangSpec;

#[derive(Clone, Copy, PartialEq)]
enum ScopeKind {
    Global,
    Fn,
}

struct Scope {
    kind: ScopeKind,
    funcs: HashMap<String, usize>,
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

#[derive(Clone, Copy, PartialEq)]
enum Ns {
    Func,
    Var,
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
            kind: ScopeKind::Global,
            funcs: HashMap::new(),
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

fn bind(st: &mut State, name: &str, range: std::ops::Range<usize>, ns: Ns) {
    let idx = st.analysis.bindings.len();
    st.analysis.bindings.push(BindingSite {
        name: range,
        text: name.to_string(),
        expansion: None,
        module_level: false,
        is_mod: false,
    });
    let si = cur(st);
    let map = match ns {
        Ns::Func => &mut st.scopes[si].funcs,
        Ns::Var => &mut st.scopes[si].vars,
    };
    map.insert(name.to_string(), idx);
}

fn lookup(st: &State, name: &str, ns: Ns) -> Option<usize> {
    for s in st.scopes.iter().rev() {
        let map = match ns {
            Ns::Func => &s.funcs,
            Ns::Var => &s.vars,
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

fn bind_node(st: &mut State, node: Node, ns: Ns) {
    bind(
        st,
        text(st.contents, node),
        node.start_byte()..node.end_byte(),
        ns,
    );
}

/// Nearest function scope index, or 0 (global) when at top level.
fn fn_scope(st: &State) -> usize {
    st.scopes
        .iter()
        .rposition(|s| s.kind == ScopeKind::Fn)
        .unwrap_or(0)
}

/// `local`/`declare`/`typeset`/`export` assignment: binds in the
/// nearest function scope (or global at top level).
fn bind_declared(st: &mut State, name_node: Node) {
    let name = text(st.contents, name_node).to_string();
    let range = name_node.start_byte()..name_node.end_byte();
    let idx = st.analysis.bindings.len();
    st.analysis.bindings.push(BindingSite {
        name: range,
        text: name,
        expansion: None,
        module_level: false,
        is_mod: false,
    });
    let si = fn_scope(st);
    st.scopes[si].vars.insert(
        st.analysis.bindings[idx].text.clone(),
        idx,
    );
}

/// Bare assignment / for-variable: if the name already resolves it is
/// a reference to that binding; otherwise it creates a global.
fn assign_or_ref(st: &mut State, name_node: Node) {
    let name = text(st.contents, name_node);
    let range = name_node.start_byte()..name_node.end_byte();
    if let Some(target) = lookup(st, name, Ns::Var) {
        st.analysis.refs.push(RefSite {
            name: range,
            target: Some(target),
            expansion: None,
            remote: false,
            module_attr: None,
        });
    } else {
        // New global binding at scope 0.
        let n = name.to_string();
        let idx = st.analysis.bindings.len();
        st.analysis.bindings.push(BindingSite {
            name: range,
            text: n.clone(),
            expansion: None,
            module_level: false,
            is_mod: false,
        });
        st.scopes[0].vars.insert(n, idx);
    }
}

fn in_declaration_command(node: Node) -> bool {
    let mut cur = node.parent();
    while let Some(p) = cur {
        if p.kind() == "declaration_command" {
            return true;
        }
        if matches!(p.kind(), "program" | "function_definition" | "compound_statement") {
            return false;
        }
        cur = p.parent();
    }
    false
}

fn walk(node: Node, st: &mut State) {
    let kind = node.kind();
    match kind {
        "function_definition" => {
            // name: (word); body: compound_statement -> function scope.
            if let Some(name) = node.child_by_field_name("name") {
                bind_node(st, name, Ns::Func);
            }
            push_fn(st);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "variable_assignment" => {
            if let Some(name) = node.child_by_field_name("name") {
                if in_declaration_command(node) {
                    bind_declared(st, name);
                } else {
                    assign_or_ref(st, name);
                }
            }
            // Walk the value for $var refs; skip the name field.
            walk_children_except_fields(st, node, &["name"]);
            return;
        }
        "variable_name" => {
            // `$x` reference — unless consumed as an assignment name
            // (handled above via field skip, so any remaining
            // variable_name is a use).
            record_ref(st, text(st.contents, node), node.start_byte()..node.end_byte(), Ns::Var);
            return;
        }
        "for_statement" => {
            if let Some(var) = node.child_by_field_name("variable") {
                assign_or_ref(st, var);
            }
            walk_children_except_fields(st, node, &["variable"]);
            return;
        }
        "command_name" => {
            // `word` child is the command/function being invoked.
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named() && child.kind() == "word" {
                    record_ref(st, text(st.contents, child), child.start_byte()..child.end_byte(), Ns::Func);
                } else if child.is_named() {
                    walk(child, st);
                }
            }
            return;
        }
        "declaration_command" => {
            // `local`/`declare`/etc: walk children; variable_assignment
            // arms handle binding via in_declaration_command.
            walk_children(node, st);
            return;
        }
        "alias_definition" | "trap" => {
            return;
        }
        _ => {}
    }
    walk_children(node, st);
}

fn push_fn(st: &mut State) {
    st.scopes.push(Scope {
        kind: ScopeKind::Fn,
        funcs: HashMap::new(),
        vars: HashMap::new(),
    });
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
