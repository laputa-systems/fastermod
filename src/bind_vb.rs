//! VB.NET binding analysis. VB is case-insensitive: binding keys are
//! lowercased so `Helper`, `HELPER` and `helper` resolve to one symbol;
//! declaration text keeps its original casing for rename selection.
//! Member access (`obj.Member`) is never resolved — recorded unresolved
//! so same-spelled renames block rather than guess at receiver types.

use std::collections::HashMap;
use std::path::Path;

use tree_sitter::{Node, Parser};

use crate::bind::{Analysis, BindingSite, RefSite};
use crate::lang::LangSpec;

#[derive(Clone, Copy, PartialEq)]
enum ScopeKind {
    Program,
    Local,
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

fn key(name: &str) -> String {
    name.to_lowercase()
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
            kind: ScopeKind::Program,
            bindings: HashMap::new(),
        }],
        contents: contents.as_bytes(),
    };
    walk(root, &mut st);
    // Late binding, `CallByName`, and cross-module members are invisible;
    // keep unresolved sets inexact so same-spelled occurrences block.
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
        module_level: st.scopes.len() <= 2,
        is_mod: false,
    });
    let si = cur(st);
    st.scopes[si].bindings.insert(key(name), idx);
}

fn resolve(st: &State, name: &str) -> Option<usize> {
    let k = key(name);
    for s in st.scopes.iter().rev() {
        if let Some(&b) = s.bindings.get(&k) {
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

fn record_unresolved(st: &mut State, range: std::ops::Range<usize>) {
    st.analysis.refs.push(RefSite {
        name: range,
        target: None,
        expansion: None,
        remote: false,
        module_attr: None,
    });
}

fn bind_field(st: &mut State, node: Node, field: &str) {
    if let Some(n) = node.child_by_field_name(field)
        && n.kind() == "identifier"
    {
        let name = text(st.contents, n);
        bind(st, name, n.start_byte()..n.end_byte());
    }
}

fn walk(node: Node, st: &mut State) {
    match node.kind() {
        "method_declaration" => {
            bind_field(st, node, "name");
            st.scopes.push(Scope {
                kind: ScopeKind::Local,
                bindings: HashMap::new(),
            });
            if let Some(params) = node.child_by_field_name("parameters") {
                for i in 0..params.child_count() {
                    let p = params.child(i).unwrap();
                    if p.kind() == "parameter" {
                        bind_field(st, p, "name");
                    }
                }
            }
            for i in 0..node.child_count() {
                let child = node.child(i).unwrap();
                if child.kind() == "statement" || child.kind() == "block" {
                    walk(child, st);
                }
            }
            st.scopes.pop();
            return;
        }
        "dim_statement" => {
            // Initializer evaluates before the new name binds (`Dim x = x`
            // sees the outer x).
            if let Some(init) = node.child_by_field_name("initializer") {
                walk(init, st);
            }
            bind_field(st, node, "name");
            return;
        }
        "invocation" => {
            if let Some(target) = node.child_by_field_name("target") {
                match target.kind() {
                    "identifier" => {
                        let n = text(st.contents, target);
                        record_ref(st, n, target.start_byte()..target.end_byte());
                    }
                    "member_access" => {
                        if let Some(obj) = target.child_by_field_name("object") {
                            walk(obj, st);
                        }
                        if let Some(member) = target.child_by_field_name("member")
                            && member.kind() == "identifier"
                        {
                            record_unresolved(st, member.start_byte()..member.end_byte());
                        }
                    }
                    _ => walk(target, st),
                }
            }
            if let Some(args) = node.child_by_field_name("arguments") {
                walk(args, st);
            }
            return;
        }
        "member_access" => {
            if let Some(obj) = node.child_by_field_name("object") {
                walk(obj, st);
            }
            if let Some(member) = node.child_by_field_name("member")
                && member.kind() == "identifier"
            {
                record_unresolved(st, member.start_byte()..member.end_byte());
            }
            return;
        }
        "identifier" => {
            // Skip declaration-name positions handled by their parents.
            if let Some(p) = node.parent() {
                let is_decl_name = (p.kind() == "method_declaration" || p.kind() == "parameter"
                    || p.kind() == "dim_statement")
                    && p.child_by_field_name("name").map(|n| n.id()) == Some(node.id());
                if is_decl_name {
                    return;
                }
                // member_access member names are handled above.
                if p.kind() == "member_access"
                    && p.child_by_field_name("member").map(|n| n.id()) == Some(node.id())
                {
                    return;
                }
            }
            let n = text(st.contents, node);
            record_ref(st, n, node.start_byte()..node.end_byte());
            return;
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, st);
    }
}
