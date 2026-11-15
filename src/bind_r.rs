//! R binding analysis via tree-sitter CST traversal.
//!
//! R has a SINGLE namespace: functions are variables holding closures,
//! so every `identifier` shares one binding space. Scopes exist only
//! at function boundaries — `{` blocks, `if`/`for`/`while` bodies do
//! NOT create scopes (matching R semantics; `for` loop variables
//! persist in the enclosing environment).
//!
//! Assignments are `binary_operator` nodes whose operator token is
//! `<-`, `<<-`, `=`, `->` or `->>` (read from the anonymous operator
//! child). `<-`/`=`/`<<-` bind the LHS; `->`/`->>` bind the RHS.
//! `x$member` (`extract_operator`) member names are not variables and
//! are skipped; `pkg::fn` (`namespace_operator`) records an unresolved
//! blocker since package resolution is unsupported.
//!
//! `local({})`, `Recall`, `assign`/`get` environment tricks, S3/S4/R6
//! method dispatch, `...` promises and non-standard evaluation are not
//! modeled — `unresolved_is_exact` is false. File-local only.

use std::collections::HashMap;
use std::path::Path;

use tree_sitter::{Node, Parser};

use crate::bind::{Analysis, BindingSite, RefSite};
use crate::lang::LangSpec;

struct Scope {
    is_fn: bool,
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
            is_fn: false,
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

fn bind(st: &mut State, name: &str, range: std::ops::Range<usize>) {
    // R rebinding (`x <- 1; x <- 2`) is the SAME variable in the same
    // environment — record a reference to the existing binding rather
    // than a duplicate.
    let si = cur(st);
    if let Some(&existing) = st.scopes[si].vars.get(name) {
        st.analysis.refs.push(RefSite {
            name: range,
            target: Some(existing),
            expansion: None,
            remote: false,
            module_attr: None,
        });
        return;
    }
    let module_level = !st.scopes[si].is_fn;
    let idx = st.analysis.bindings.len();
    st.analysis.bindings.push(BindingSite {
        name: range,
        text: name.to_string(),
        expansion: None,
        module_level,
        is_mod: false,
    });
    st.scopes[si].vars.insert(name.to_string(), idx);
}

fn lookup(st: &State, name: &str) -> Option<usize> {
    for s in st.scopes.iter().rev() {
        if let Some(&b) = s.vars.get(name) {
            return Some(b);
        }
    }
    None
}

fn record_ref(st: &mut State, name: &str, range: std::ops::Range<usize>) {
    st.analysis.refs.push(RefSite {
        name: range,
        target: lookup(st, name),
        expansion: None,
        remote: false,
        module_attr: None,
    });
}

fn bind_node(st: &mut State, node: Node) {
    bind(
        st,
        text(st.contents, node),
        node.start_byte()..node.end_byte(),
    );
}

/// The operator token of a `binary_operator`: the anonymous child
/// between the `lhs`/`rhs` fields.
fn operator_text<'a>(st: &State<'a>, node: Node) -> &'a str {
    let lhs = node.child_by_field_name("lhs");
    let rhs = node.child_by_field_name("rhs");
    let (Some(l), Some(r)) = (lhs, rhs) else {
        return "";
    };
    let bytes = st.contents;
    let start = l.end_byte();
    let end = r.start_byte();
    if start >= end || end > bytes.len() {
        return "";
    }
    std::str::from_utf8(&bytes[start..end])
        .unwrap_or("")
        .trim()
}

fn is_assign_op(op: &str) -> bool {
    matches!(op, "<-" | "<<-" | "=" | "->" | "->>")
}

/// Bind every `identifier` leaf in an assignment target (destructuring
/// targets like `c(a, b) <- ...` bind only plain identifiers; call
/// targets are walked as expressions instead).
fn bind_target(st: &mut State, node: Node) {
    match node.kind() {
        "identifier" => bind_node(st, node),
        "binary_operator" => {
            // Nested assignment `a <- b <- 1`: recurse on both sides'
            // targets — the rhs may itself be an assignment.
            if let Some(l) = node.child_by_field_name("lhs") {
                bind_target(st, l);
            }
            if let Some(r) = node.child_by_field_name("rhs") {
                bind_target(st, r);
            }
        }
        _ => walk(node, st),
    }
}

fn walk_assignment(node: Node, st: &mut State) {
    let op = operator_text(st, node);
    let right_assign = matches!(op, "->" | "->>");
    let (target_field, value_field) = if right_assign {
        ("rhs", "lhs")
    } else {
        ("lhs", "rhs")
    };
    if let Some(value) = node.child_by_field_name(value_field) {
        walk(value, st);
    }
    if let Some(target) = node.child_by_field_name(target_field) {
        bind_target(st, target);
    }
    // Walk any remaining children (shouldn't exist for binary_operator,
    // but keeps the traversal total).
    walk_children_except_fields(st, node, &["lhs", "rhs"]);
}

fn walk(node: Node, st: &mut State) {
    let kind = node.kind();
    match kind {
        "function_definition" => {
            st.scopes.push(Scope {
                is_fn: true,
                vars: HashMap::new(),
            });
            if let Some(params) = child_of_kind(node, "parameters") {
                // parameter name:(identifier) binds; default exprs walk.
                for param in named_children(params) {
                    if param.kind() != "parameter" {
                        continue;
                    }
                    for i in 0..param.child_count() {
                        let pc = param.child(i).unwrap();
                        if !pc.is_named() {
                            continue;
                        }
                        if param.field_name_for_child(i) == Some("name")
                            && pc.kind() == "identifier"
                        {
                            bind_node(st, pc);
                        } else {
                            walk(pc, st);
                        }
                    }
                }
            }
            if let Some(body) = child_of_kind(node, "braced_expression") {
                walk(body, st);
            }
            st.scopes.pop();
            return;
        }
        "parameter" => {
            // Handled inside function_definition; standalone safety.
            walk_children(node, st);
            return;
        }
        "binary_operator" => {
            if is_assign_op(operator_text(st, node)) {
                walk_assignment(node, st);
            } else {
                walk_children(node, st);
            }
            return;
        }
        "extract_operator" => {
            // `x$member`: walk LHS object; member name is not a var.
            if let Some(l) = node.child_by_field_name("lhs") {
                walk(l, st);
            }
            if let Some(r) = node.child_by_field_name("rhs")
                && r.kind() != "identifier"
            {
                walk(r, st);
            }
            return;
        }
        "namespace_operator" => {
            // `pkg::fn`: package name skipped; function name is an
            // unresolved blocker (no cross-package resolution).
            if let Some(l) = node.child_by_field_name("lhs") {
                // package identifier — not a variable reference.
                let _ = l;
            }
            if let Some(r) = node.child_by_field_name("rhs") {
                st.analysis.refs.push(RefSite {
                    name: r.start_byte()..r.end_byte(),
                    target: None,
                    expansion: None,
                    remote: false,
                    module_attr: None,
                });
            }
            return;
        }
        "call" => {
            if let Some(f) = node.child_by_field_name("function") {
                if f.kind() == "identifier" {
                    record_ref(
                        st,
                        text(st.contents, f),
                        f.start_byte()..f.end_byte(),
                    );
                } else {
                    walk(f, st);
                }
            }
            if let Some(args) = node.child_by_field_name("arguments") {
                walk(args, st);
            }
            walk_children_except_fields(st, node, &["function", "arguments"]);
            return;
        }
        "for_statement" => {
            // Loop variable binds in the CURRENT scope (R for-vars
            // persist after the loop).
            if let Some(v) = node.child_by_field_name("variable") {
                bind_node(st, v);
            }
            walk_children_except_fields(st, node, &["variable"]);
            return;
        }
        "identifier" => {
            record_ref(st, text(st.contents, node), node.start_byte()..node.end_byte());
            return;
        }
        _ => {}
    }
    walk_children(node, st);
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
