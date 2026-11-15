//! OCaml binding analysis. `let name pat = expr` binds `name` in the
//! enclosing scope after `expr` is evaluated; `let rec` binds before.
//! `let ... in body` scopes its bindings to `body` only. Function
//! parameters bind over the body. Module-qualified access (`M.x`) is
//! never resolved — members are recorded unresolved so same-spelled
//! renames block rather than guess across compilation units.

use std::collections::HashMap;
use std::path::Path;

use tree_sitter::{Node, Parser};

use crate::bind::{Analysis, BindingSite, RefSite};
use crate::lang::LangSpec;

struct Scope {
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
            bindings: HashMap::new(),
        }],
        contents: contents.as_bytes(),
    };
    walk(root, &mut st);
    // First-class modules, objects, and GADTs make same-named occurrences
    // potentially relevant anywhere; keep unresolved sets inexact.
    st.analysis.unresolved_is_exact = false;
    Some(st.analysis)
}

fn cur(st: &State) -> usize {
    st.scopes.len() - 1
}

fn bind(st: &mut State, name: &str, range: std::ops::Range<usize>) {
    if name.is_empty() {
        return;
    }
    let idx = st.analysis.bindings.len();
    st.analysis.bindings.push(BindingSite {
        name: range,
        text: name.to_string(),
        expansion: None,
        module_level: st.scopes.len() == 1,
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

fn push(st: &mut State) {
    st.scopes.push(Scope {
        bindings: HashMap::new(),
    });
}

fn names_in(st: &State, node: Node, out: &mut Vec<(String, std::ops::Range<usize>)>) {
    match node.kind() {
        "value_name" => {
            let n = text(st.contents, node);
            out.push((n.to_string(), node.start_byte()..node.end_byte()));
        }
        "constructor_name" | "module_name" | "mod_constructor_name" | "unit" | "record_pattern" => {}
        _ => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                names_in(st, child, out);
            }
        }
    }
}

fn do_bind(st: &mut State, names: &[(String, std::ops::Range<usize>)]) {
    for (n, r) in names {
        bind(st, n, r.clone());
    }
}

fn let_binding_child(vd: Node) -> Option<Node> {
    let mut cursor = vd.walk();
    vd.children(&mut cursor).find(|c| c.kind() == "let_binding")
}

fn is_rec(vd: Node) -> bool {
    let mut cursor = vd.walk();
    vd.children(&mut cursor).any(|c| c.kind() == "rec")
}

/// Bind pattern names of a let_binding (the defined name(s)).
fn pattern_names(st: &State, lb: Node) -> Vec<(String, std::ops::Range<usize>)> {
    let mut out = Vec::new();
    if let Some(pat) = lb.child_by_field_name("pattern") {
        names_in(st, pat, &mut out);
    }
    out
}

/// Bind parameter names of a let_binding (function args).
fn param_names(st: &State, lb: Node) -> Vec<(String, std::ops::Range<usize>)> {
    let mut out = Vec::new();
    for i in 0..lb.child_count() {
        let child = lb.child(i).unwrap();
        if child.kind() == "parameter" {
            names_in(st, child, &mut out);
        }
    }
    out
}

fn has_params(lb: Node) -> bool {
    (0..lb.child_count()).any(|i| lb.child(i).unwrap().kind() == "parameter")
}

/// Handle a `value_definition` (`let [rec] lb`). When `local_names` is
/// true the bound names go into a fresh scope that the caller pops after
/// walking the `in` body (let_expression); otherwise they persist in the
/// current scope.
fn walk_value_definition(st: &mut State, vd: Node, local_names: bool) {
    let Some(lb) = let_binding_child(vd) else {
        let mut cursor = vd.walk();
        for child in vd.children(&mut cursor) {
            walk(child, st);
        }
        return;
    };
    let rec = is_rec(vd);
    let params = has_params(lb);
    let names = pattern_names(st, lb);

    if local_names {
        push(st);
    }
    if rec {
        do_bind(st, &names);
    }
    if params {
        // Function: params scope over the body; the defined name binds
        // in the enclosing (or let_expression-local) scope.
        push(st);
        do_bind(st, &param_names(st, lb));
        if let Some(body) = lb.child_by_field_name("body") {
            walk(body, st);
        }
        st.scopes.pop();
        if !rec {
            do_bind(st, &names);
        }
    } else {
        if let Some(body) = lb.child_by_field_name("body") {
            walk(body, st);
        }
        if !rec {
            do_bind(st, &names);
        }
    }
    // local_names: caller pops the scope after the `in` body.
}

fn walk(node: Node, st: &mut State) {
    match node.kind() {
        "value_definition" => {
            walk_value_definition(st, node, false);
            return;
        }
        "let_expression" => {
            // `let vd in body`: vd's names scope over body only.
            let mut vd_opt = None;
            let mut body_opt = None;
            for i in 0..node.child_count() {
                let child = node.child(i).unwrap();
                match child.kind() {
                    "value_definition" => vd_opt = Some(child),
                    _ => {
                        if node.field_name_for_child(i) == Some("body") {
                            body_opt = Some(child);
                        }
                    }
                }
            }
            if let Some(vd) = vd_opt {
                walk_value_definition(st, vd, true);
            } else {
                for i in 0..node.child_count() {
                    let child = node.child(i).unwrap();
                    if child.kind() != "in" && node.field_name_for_child(i) != Some("body") {
                        walk(child, st);
                    }
                }
            }
            if let Some(body) = body_opt {
                walk(body, st);
            }
            if vd_opt.is_some() {
                st.scopes.pop();
            }
            return;
        }
        "value_path" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                match child.kind() {
                    "value_name" => {
                        let n = text(st.contents, child);
                        record_ref(st, n, child.start_byte()..child.end_byte());
                    }
                    "module_path" => {
                        // `M.x`: `M` may name a compilation unit; the
                        // `value_name` sibling is the member — unresolved.
                        let mut c2 = child.walk();
                        for mc in child.children(&mut c2) {
                            if mc.kind() == "module_name" {
                                let n = text(st.contents, mc);
                                record_ref(st, n, mc.start_byte()..mc.end_byte());
                            }
                        }
                    }
                    _ => walk(child, st),
                }
            }
            return;
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, st);
    }
}
