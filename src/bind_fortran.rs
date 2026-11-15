//! Fortran binding analysis. Procedures (`function`/`subroutine`) bind
//! their name in the enclosing (module/program) scope; bodies get their
//! own scope for locals and dummy arguments. `end` statements re-name
//! the procedure — recorded as a resolved reference. Undeclared
//! assignment targets bind implicitly (Fortran implicit typing) unless
//! they already resolve. Host association across files is not modeled;
//! unresolved sets stay inexact.

use std::collections::HashMap;
use std::path::Path;

use tree_sitter::{Node, Parser};

use crate::bind::{Analysis, BindingSite, RefSite};
use crate::lang::LangSpec;

#[derive(Clone, Copy, PartialEq)]
enum ScopeKind {
    /// Module/program/unit scope.
    Unit,
    /// Procedure body scope.
    Proc,
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
            kind: ScopeKind::Unit,
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
        module_level: st.scopes.len() <= 2,
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

/// Bind `name`, or record a reference to the existing binding when the
/// same scope already declares it — Fortran redeclarations (dummy arg +
/// type statement, repeated `:: x`) denote one variable.
fn bind_or_ref(st: &mut State, name: &str, range: std::ops::Range<usize>) {
    // Same-scope redeclaration only: an outer-scope binding must not
    // swallow a new local declaration.
    let si = cur(st);
    if st.scopes[si].bindings.contains_key(name) {
        record_ref(st, name, range);
    } else {
        bind(st, name, range);
    }
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

fn push(st: &mut State, kind: ScopeKind) {
    st.scopes.push(Scope {
        kind,
        bindings: HashMap::new(),
    });
}

fn proc_name_field(st: &mut State, stmt: Node) {
    // *_statement child carries field "name" for the procedure name.
    for i in 0..stmt.child_count() {
        let child = stmt.child(i).unwrap();
        if stmt.field_name_for_child(i) == Some("name") && child.kind() == "name" {
            // `name` node wraps the identifier text.
            let n = text(st.contents, child);
            let inner = child
                .named_child(0)
                .filter(|c| c.kind() == "identifier")
                .unwrap_or(child);
            let name = if inner.kind() == "identifier" {
                text(st.contents, inner)
            } else {
                n
            };
            let range = if inner.kind() == "identifier" {
                inner.start_byte()..inner.end_byte()
            } else {
                child.start_byte()..child.end_byte()
            };
            bind(st, name, range);
        }
    }
}

fn declarator_names(st: &State, node: Node, out: &mut Vec<(String, std::ops::Range<usize>)>) {
    match node.kind() {
        "identifier" => {
            // Skip if this identifier is a type/keyword position —
            // declarator fields only reach here via field lookup anyway.
            let n = text(st.contents, node);
            out.push((n.to_string(), node.start_byte()..node.end_byte()));
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                declarator_names(st, child, out);
            }
        }
    }
}

fn walk(node: Node, st: &mut State) {
    match node.kind() {
        "function" | "subroutine" => {
            // statement child holds the name; parameters are identifiers
            // inside it (dummy args bind in the proc scope).
            let mut stmt_opt = None;
            for i in 0..node.child_count() {
                let child = node.child(i).unwrap();
                if matches!(child.kind(), "function_statement" | "subroutine_statement") {
                    stmt_opt = Some(child);
                }
            }
            if let Some(stmt) = stmt_opt {
                proc_name_field(st, stmt);
                // Dummy arguments: identifier children under parameters.
                push(st, ScopeKind::Proc);
                for i in 0..stmt.child_count() {
                    let child = stmt.child(i).unwrap();
                    if stmt.field_name_for_child(i) == Some("parameters") {
                        let mut names = Vec::new();
                        declarator_names(st, child, &mut names);
                        for (n, r) in names {
                            bind_or_ref(st, &n, r);
                        }
                    }
                }
                // Walk remaining children (declarations, assignments).
                for i in 0..node.child_count() {
                    let child = node.child(i).unwrap();
                    if Some(child.id()) == stmt_opt.map(|s| s.id()) {
                        continue;
                    }
                    walk(child, st);
                }
                st.scopes.pop();
                return;
            }
        }
        "end_function_statement" | "end_subroutine_statement" => {
            // `end function bump`: the name re-refers to the procedure.
            // (This node carries no field name for it.)
            for i in 0..node.child_count() {
                let child = node.child(i).unwrap();
                if child.kind() == "name" {
                    let n = text(st.contents, child);
                    let inner = child
                        .named_child(0)
                        .filter(|c| c.kind() == "identifier")
                        .unwrap_or(child);
                    let name = if inner.kind() == "identifier" {
                        text(st.contents, inner)
                    } else {
                        n
                    };
                    let range = if inner.kind() == "identifier" {
                        inner.start_byte()..inner.end_byte()
                    } else {
                        child.start_byte()..child.end_byte()
                    };
                    record_ref(st, name, range);
                }
            }
            return;
        }
        "variable_declaration" => {
            // Bind every declarator identifier; walk type/attribute parts.
            for i in 0..node.child_count() {
                let child = node.child(i).unwrap();
                if node.field_name_for_child(i) == Some("declarator") {
                    let mut names = Vec::new();
                    declarator_names(st, child, &mut names);
                    for (n, r) in names {
                        bind_or_ref(st, &n, r);
                    }
                } else {
                    walk(child, st);
                }
            }
            return;
        }
        "assignment_statement" => {
            // RHS first, then LHS: `x = x + 1` RHS sees outer x.
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
        "identifier" => {
            // Skip declaration-name positions already bound.
            if let Some(p) = node.parent() {
                let decl = matches!(
                    p.kind(),
                    "variable_declaration" | "function_statement" | "subroutine_statement"
                        | "end_function_statement" | "end_subroutine_statement"
                ) || (p.kind() == "parameters");
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
