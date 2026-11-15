//! MATLAB/Octave binding analysis. Script-level assignments bind in the
//! file (workspace) scope; `function` definitions bind their name at file
//! scope and open a local scope for outputs, arguments, and body
//! variables. `obj.field` access records the field unresolved — struct
//! and object member identity is not modeled.

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

fn bind_idents(st: &mut State, node: Node) {
    // Bind every identifier descendant that is not inside a nested
    // expression position — callers pass declaration containers only.
    match node.kind() {
        "identifier" => {
            let n = text(st.contents, node);
            bind(st, n, node.start_byte()..node.end_byte());
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                bind_idents(st, child);
            }
        }
    }
}

fn walk(node: Node, st: &mut State) {
    match node.kind() {
        "function_definition" => {
            // name binds at file scope.
            if let Some(name) = node.child_by_field_name("name")
                && name.kind() == "identifier"
            {
                let n = text(st.contents, name);
                bind(st, n, name.start_byte()..name.end_byte());
            }
            st.scopes.push(Scope {
                bindings: HashMap::new(),
            });
            // Outputs and arguments bind in the function scope; the
            // grammar gives these wrapper nodes no field names.
            for i in 0..node.child_count() {
                let child = node.child(i).unwrap();
                match child.kind() {
                    "function_output" | "function_arguments" => bind_idents(st, child),
                    "block" => walk(child, st),
                    _ => {}
                }
            }
            st.scopes.pop();
            return;
        }
        "assignment" => {
            // RHS first, then LHS binds/refs.
            if let Some(rhs) = node.child_by_field_name("right") {
                walk(rhs, st);
            }
            if let Some(lhs) = node.child_by_field_name("left") {
                if lhs.kind() == "identifier" {
                    let n = text(st.contents, lhs);
                    if resolve(st, n).is_some() {
                        record_ref(st, n, lhs.start_byte()..lhs.end_byte());
                    } else {
                        bind(st, n, lhs.start_byte()..lhs.end_byte());
                    }
                } else {
                    walk(lhs, st);
                }
            }
            return;
        }
        "function_call" => {
            if let Some(name) = node.child_by_field_name("name")
                && name.kind() == "identifier"
            {
                let n = text(st.contents, name);
                record_ref(st, n, name.start_byte()..name.end_byte());
            }
            for i in 0..node.child_count() {
                let child = node.child(i).unwrap();
                if child.kind() == "arguments" {
                    walk(child, st);
                }
            }
            return;
        }
        "field_expression" => {
            if let Some(obj) = node.child_by_field_name("object") {
                walk(obj, st);
            }
            if let Some(field) = node.child_by_field_name("field")
                && field.kind() == "identifier"
            {
                record_unresolved(st, field.start_byte()..field.end_byte());
            }
            return;
        }
        "identifier" => {
            if let Some(p) = node.parent() {
                let decl = matches!(
                    p.kind(),
                    "assignment" | "function_definition" | "function_output"
                        | "function_arguments" | "field_expression"
                ) && (p.child_by_field_name("left").map(|n| n.id()) == Some(node.id())
                    || p.child_by_field_name("name").map(|n| n.id()) == Some(node.id())
                    || p.child_by_field_name("output").map(|n| n.id()) == Some(node.id())
                    || p.child_by_field_name("arguments").map(|n| n.id()) == Some(node.id())
                    || p.child_by_field_name("field").map(|n| n.id()) == Some(node.id()));
                if decl {
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
