//! Lua binding analysis via tree-sitter CST traversal.
//!
//! Scopes: the chunk is the outermost scope; `block` opens a lexical
//! scope except a function body block, which shares its function scope
//! (parameters stay visible). `for` statements open a scope covering
//! the clause and body. `local` declarations bind in the enclosing
//! scope; plain assignments are references, not bindings.
//!
//! Boundaries: `_ENV` manipulation, `load`/`loadstring`, `setfenv`,
//! `rawset(_G, ...)` and other dynamic global resolution are invisible,
//! so `unresolved_is_exact` is false and unresolved same-name
//! occurrences block the rename. Table fields (`t.f`, `{f=1}`), method
//! names (`t:m`), and labels/gotos are separate namespaces.

use std::collections::HashMap;
use std::path::Path;

use ast_grep_core::tree_sitter::LanguageExt;
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
            bindings: HashMap::new(),
        }],
        contents: contents.as_bytes(),
    };
    walk(root, &mut st);
    Some(st.analysis)
}

fn cur(st: &State) -> usize {
    st.scopes.len() - 1
}

fn bind(st: &mut State, name: &str, range: std::ops::Range<usize>, module_level: bool) {
    let idx = st.analysis.bindings.len();
    st.analysis.bindings.push(BindingSite {
        name: range,
        text: name.to_string(),
        expansion: None,
        module_level,
        is_mod: false,
    });
    let si = cur(st);
    st.scopes[si].bindings.insert(name.to_string(), idx);
}

fn module_level(st: &State) -> bool {
    st.scopes.len() == 1
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

fn walk(node: Node, st: &mut State) {
    match node.kind() {
        // `function name() ... end` and `local function name()`. The name
        // binds in the enclosing scope; parameters and body get a
        // function scope. A method/dot-index name defines a table
        // field — no identifier binding — so just walk it as refs.
        "function_declaration" | "function_definition" => {
            if node.kind() == "function_declaration" {
                if let Some(name) = node.child_by_field_name("name") {
                    if name.kind() == "identifier" {
                        bind(
                            st,
                            text(st.contents, name),
                            name.start_byte()..name.end_byte(),
                            module_level(st),
                        );
                    } else {
                        walk(name, st);
                    }
                }
            }
            st.scopes.push(Scope {
                bindings: HashMap::new(),
            });
            if let Some(params) = node.child_by_field_name("parameters") {
                bind_params(params, st);
            }
            if let Some(body) = node.child_by_field_name("body") {
                walk(body, st);
            }
            st.scopes.pop();
            return;
        }
        "local_declaration" => {
            // children: variable_declaration | function_declaration.
            // function_declaration handles itself; variable_declaration
            // binds its variable_list names.
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if !child.is_named() {
                    continue;
                }
                if child.kind() == "variable_declaration" {
                    bind_local_decl(child, st);
                } else {
                    walk(child, st);
                }
            }
            return;
        }
        "variable_declaration" => {
            bind_local_decl(node, st);
            return;
        }
        "block" => {
            // Function body blocks share the function scope pushed by
            // function_declaration/function_definition.
            let shares_fn_scope = node
                .parent()
                .is_some_and(|p| matches!(p.kind(), "function_declaration" | "function_definition"));
            if shares_fn_scope {
                walk_children(node, st);
            } else {
                st.scopes.push(Scope {
                    bindings: HashMap::new(),
                });
                walk_children(node, st);
                st.scopes.pop();
            }
            return;
        }
        "for_statement" => {
            st.scopes.push(Scope {
                bindings: HashMap::new(),
            });
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "for_numeric_clause" => {
            // start/end/step expressions see outer bindings; the loop
            // variable scopes over the body only.
            walk_children_except_names(node, st);
            bind_name_fields(st, node);
            return;
        }
        "for_generic_clause" => {
            // names live on the nested variable_list child; the iterated
            // expression list is evaluated outside the loop scope.
            let mut lists = Vec::new();
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if !child.is_named() {
                    continue;
                }
                if child.kind() == "variable_list" {
                    lists.push(child);
                } else {
                    walk(child, st);
                }
            }
            for list in lists {
                bind_name_fields(st, list);
            }
            return;
        }
        "dot_index_expression" | "method_index_expression" => {
            // `t.f` / `t:m`: the table operand is a ref; the field/method
            // name is a table member, a different namespace.
            if let Some(table) = node.child_by_field_name("table") {
                walk(table, st);
            }
            return;
        }
        "field" => {
            // `{f = 1}`: `name` is a table key; walk the value only.
            walk_children_except_names(node, st);
            return;
        }
        "label_statement" | "goto_statement" => {
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

fn walk_children(node: Node, st: &mut State) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() {
            walk(child, st);
        }
    }
}

/// Walk children, skipping `name`-field identifier leaves that parent
/// arms have already consumed as bindings.
fn walk_children_except_names(node: Node, st: &mut State) {
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        if !child.is_named() {
            continue;
        }
        if node.field_name_for_child(i) == Some("name") && child.kind() == "identifier" {
            continue;
        }
        walk(child, st);
    }
}

/// Bind every `name`-field identifier child (parameters, for clauses).
fn bind_name_fields(st: &mut State, node: Node) {
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        if node.field_name_for_child(i) == Some("name") && child.kind() == "identifier" {
            bind(
                st,
                text(st.contents, child),
                child.start_byte()..child.end_byte(),
                module_level(st),
            );
        }
    }
}

fn bind_params(node: Node, st: &mut State) {
    bind_name_fields(st, node);
}

/// `local a, b = ...`: the variable_list's `name` identifiers bind;
/// the expression_list values are walked for refs.
fn bind_local_decl(node: Node, st: &mut State) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        match child.kind() {
            "assignment_statement" => {
                // variable_list/expression_list are positional children,
                // not named fields, in this grammar. Walk the RHS before
                // binding the LHS: `local a, b = b, a` reads the outer
                // bindings, matching Lua's "scope begins after the
                // declaration" rule.
                let mut parts: Vec<Node> = child
                    .children(&mut child.walk())
                    .filter(|c| c.is_named())
                    .collect();
                parts.sort_by_key(|c| {
                    if c.kind() == "variable_list" { 1 } else { 0 }
                });
                for part in parts {
                    if part.kind() == "variable_list" {
                        bind_name_fields(st, part);
                    } else {
                        walk(part, st);
                    }
                }
            }
            _ => walk(child, st),
        }
    }
}
