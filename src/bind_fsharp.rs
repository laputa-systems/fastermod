//! F# binding analysis. `let` bindings are sequential: each definition
//! sees earlier bindings in the same block, and `let rec` groups bind
//! before their bodies. Function parameters scope over the body.
//! Dotted `long_identifier` access (`Module.member`) records the member
//! unresolved — cross-module identity is never guessed.

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

fn record_unresolved(st: &mut State, range: std::ops::Range<usize>) {
    st.analysis.refs.push(RefSite {
        name: range,
        target: None,
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

/// First `identifier` descendant — the defined name in declaration-left
/// nodes, or a use-site identifier.
fn first_identifier(node: Node) -> Option<Node> {
    if node.kind() == "identifier" {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(found) = first_identifier(child) {
            return Some(found);
        }
    }
    None
}

/// Parameter names inside `argument_patterns`: every identifier that is
/// not part of a type annotation or literal pattern. Conservative: bind
/// all `long_identifier`/`identifier` leaves under argument_patterns.
fn bind_params(st: &mut State, node: Node) {
    match node.kind() {
        "identifier" => {
            let n = text(st.contents, node);
            bind(st, n, node.start_byte()..node.end_byte());
        }
        "const" | "type" | "type_annotation" | "as_pattern" => {}
        _ => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                bind_params(st, child);
            }
        }
    }
}

fn walk(node: Node, st: &mut State) {
    match node.kind() {
        "function_or_value_defn" => {
            // `let [rec] left = body`; left is function_declaration_left
            // (has argument_patterns) or value_declaration_left.
            let mut left_opt = None;
            let mut body_opt = None;
            let mut rec = false;
            for i in 0..node.child_count() {
                let child = node.child(i).unwrap();
                match child.kind() {
                    "function_declaration_left" | "value_declaration_left" => {
                        left_opt = Some(child)
                    }
                    "rec" => rec = true,
                    _ => {
                        if node.field_name_for_child(i) == Some("body") {
                            body_opt = Some(child);
                        }
                    }
                }
            }
            let Some(left) = left_opt else {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    walk(child, st);
                }
                return;
            };
            let is_function = left.kind() == "function_declaration_left";
            let name_node = first_identifier(left);
            let name_range = name_node.map(|n| (text(st.contents, n).to_string(), n.start_byte()..n.end_byte()));

            if rec && let Some((n, r)) = &name_range {
                bind(st, n, r.clone());
            }
            if is_function {
                push(st);
                // argument_patterns child of left holds the params.
                let mut cursor = left.walk();
                for child in left.children(&mut cursor) {
                    if child.kind() == "argument_patterns" {
                        bind_params(st, child);
                    }
                }
                if let Some(body) = body_opt {
                    walk(body, st);
                }
                st.scopes.pop();
            } else if let Some(body) = body_opt {
                walk(body, st);
            }
            if !rec && let Some((n, r)) = &name_range {
                bind(st, n, r.clone());
            }
            return;
        }
        "long_identifier" | "long_identifier_or_op" => {
            // Dotted path: first identifier resolves, later segments are
            // members — unresolved so same-spelled renames block.
            let mut cursor = node.walk();
            let mut first = true;
            for child in node.children(&mut cursor) {
                if child.kind() == "identifier" {
                    let n = text(st.contents, child);
                    if first {
                        record_ref(st, n, child.start_byte()..child.end_byte());
                        first = false;
                    } else {
                        record_unresolved(st, child.start_byte()..child.end_byte());
                    }
                } else {
                    walk(child, st);
                }
            }
            return;
        }
        "identifier" => {
            // Declaration positions are consumed by function_or_value_defn;
            // a bare identifier elsewhere is a use.
            if let Some(p) = node.parent()
                && matches!(p.kind(), "function_declaration_left" | "value_declaration_left" | "identifier_pattern" | "argument_patterns")
            {
                return;
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
