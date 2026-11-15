//! Go binding analysis via tree-sitter CST traversal. Go has a single
//! package block plus lexical function/block scopes; there are no macros
//! or dynamic identifier resolutions, so the unresolved set is exact.
//!
//! Deliberate boundaries: method names (`field_identifier` in
//! `method_declaration` and selector fields) are a different namespace
//! and are never rename sites; struct/interface field names likewise.

use std::collections::HashMap;
use std::path::Path;

use ast_grep_core::tree_sitter::LanguageExt;
use tree_sitter::{Node, Parser};

use crate::bind::{Analysis, BindingSite, RefSite};
use crate::lang::LangSpec;

#[derive(Clone, Copy, PartialEq)]
enum ScopeKind {
    Package,
    Function,
    Block,
    /// if/for/switch statement scope: covers the initializer and the
    /// whole statement body (Go spec: "the scope of an identifier
    /// declared inside a function begins at the end of the ConstSpec or
    /// VarSpec ... and ends at the end of the innermost containing
    /// block"; if/for/switch init identifiers scope to the statement).
    Stmt,
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

fn is_name_kind(kind: &str) -> bool {
    kind == "identifier" || kind == "type_identifier"
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
            kind: ScopeKind::Package,
            bindings: HashMap::new(),
        }],
        contents: contents.as_bytes(),
    };
    walk(root, &mut st);
    st.analysis.unresolved_is_exact = true;
    Some(st.analysis)
}

fn cur(st: &State) -> usize {
    st.scopes.len() - 1
}

fn bind(st: &mut State, name: &str, range: std::ops::Range<usize>, module_level: bool) {
    if name == "_" {
        return;
    }
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

/// Bind every `name`-field identifier child of `node` (var_spec and
/// parameter_declaration carry repeated `name` fields).
fn bind_name_fields(st: &mut State, node: Node) {
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        if node.field_name_for_child(i) == Some("name") && is_name_kind(child.kind()) {
            bind(
                st,
                text(st.contents, child),
                child.start_byte()..child.end_byte(),
                module_level(st),
            );
        }
    }
}

fn walk(node: Node, st: &mut State) {
    match node.kind() {
        "function_declaration" => {
            if let Some(name) = node.child_by_field_name("name") {
                bind(st, text(st.contents, name), name.start_byte()..name.end_byte(), true);
            }
            st.scopes.push(Scope {
                kind: ScopeKind::Function,
                bindings: HashMap::new(),
            });
            if let Some(params) = node.child_by_field_name("parameters") {
                walk_params(params, st);
            }
            if let Some(result) = node.child_by_field_name("result") {
                walk_type_only(result, st);
            }
            if let Some(body) = node.child_by_field_name("body") {
                walk(body, st);
            }
            st.scopes.pop();
            return;
        }
        "method_declaration" => {
            // Receiver + params scope like a function; the method name is
            // a field_identifier and intentionally not a rename site.
            st.scopes.push(Scope {
                kind: ScopeKind::Function,
                bindings: HashMap::new(),
            });
            if let Some(recv) = node.child_by_field_name("receiver") {
                walk_params(recv, st);
            }
            if let Some(params) = node.child_by_field_name("parameters") {
                walk_params(params, st);
            }
            if let Some(body) = node.child_by_field_name("body") {
                walk(body, st);
            }
            st.scopes.pop();
            return;
        }
        "func_literal" => {
            st.scopes.push(Scope {
                kind: ScopeKind::Function,
                bindings: HashMap::new(),
            });
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "var_declaration" | "const_declaration" => {
            walk_children(node, st); // var_spec/const_spec bind below
            return;
        }
        "var_spec" | "const_spec" => {
            // Value/type expressions precede the spec's scope.
            if let Some(v) = node.child_by_field_name("value") {
                walk(v, st);
            }
            if let Some(t) = node.child_by_field_name("type") {
                walk_type_only(t, st);
            }
            bind_name_fields(st, node);
            return;
        }
        "type_declaration" => {
            walk_children(node, st);
            return;
        }
        "type_spec" => {
            if let Some(name) = node.child_by_field_name("name") {
                bind(st, text(st.contents, name), name.start_byte()..name.end_byte(), module_level(st));
            }
            if let Some(t) = node.child_by_field_name("type") {
                walk_type_only(t, st);
            }
            return;
        }
        "parameter_list" => {
            walk_params(node, st);
            return;
        }
        "parameter_declaration" => {
            if let Some(t) = node.child_by_field_name("type") {
                walk_type_only(t, st);
            }
            bind_name_fields(st, node);
            return;
        }
        "short_var_declaration" => {
            // RHS is evaluated before the new bindings scope begins
            // (`a, b := b, a` swaps outer values).
            if let Some(right) = node.child_by_field_name("right") {
                walk(right, st);
            }
            if let Some(left) = node.child_by_field_name("left") {
                bind_expr_list(st, left);
            }
            return;
        }
        "import_declaration" => {
            walk_import(node, st);
            return;
        }
        "if_statement" | "for_statement" | "expression_switch_statement"
        | "type_switch_statement" | "select_statement" => {
            st.scopes.push(Scope {
                kind: ScopeKind::Stmt,
                bindings: HashMap::new(),
            });
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "block" => {
            // function bodies share the function scope; every other block
            // opens a lexical scope.
            if st.scopes.last().is_some_and(|s| s.kind == ScopeKind::Function)
                && node.parent().is_some_and(|p| {
                    matches!(
                        p.kind(),
                        "function_declaration" | "method_declaration" | "func_literal"
                    )
                })
            {
                walk_children(node, st);
            } else {
                st.scopes.push(Scope {
                    kind: ScopeKind::Block,
                    bindings: HashMap::new(),
                });
                walk_children(node, st);
                st.scopes.pop();
            }
            return;
        }
        "identifier" | "type_identifier" => {
            // Plain name positions: record a resolved-or-unresolved ref.
            // Parent kinds that make this node a declaration name are
            // handled by their own arms before reaching here.
            if is_decl_name(node) {
                return;
            }
            record_ref(st, text(st.contents, node), node.start_byte()..node.end_byte());
            return;
        }
        "selector_expression" => {
            // operand is a normal ref; the field is a different namespace.
            if let Some(op) = node.child_by_field_name("operand") {
                walk(op, st);
            }
            return;
        }
        "field_identifier" | "package_identifier" | "label_name"
        | "interpreted_string_literal" | "raw_string_literal" => {
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

/// Identifier nodes that are declaration names handled by their parent —
/// skip recording them as refs when reached generically.
fn is_decl_name(node: Node) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    match parent.kind() {
        "function_declaration" | "method_declaration" | "type_spec"
        | "parameter_declaration" | "var_spec" | "const_spec" => {
            parent.child_by_field_name("name") == Some(node)
        }
        "import_spec" => parent.child_by_field_name("name") == Some(node),
        _ => false,
    }
}

fn walk_params(node: Node, st: &mut State) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() {
            walk(child, st);
        }
    }
}

/// Type positions: record type_identifier refs but never bind.
fn walk_type_only(node: Node, st: &mut State) {
    match node.kind() {
        "type_identifier" => {
            record_ref(st, text(st.contents, node), node.start_byte()..node.end_byte());
        }
        "field_identifier" | "type_parameter" => {}
        _ => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named() {
                    walk_type_only(child, st);
                }
            }
        }
    }
}

fn bind_expr_list(st: &mut State, node: Node) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        if child.kind() == "identifier" {
            bind(st, text(st.contents, child), child.start_byte()..child.end_byte(), false);
        } else {
            // `a.b := ...` is invalid Go; other lhs forms are refs.
            walk(child, st);
        }
    }
}

/// `import "path"`, `import name "path"`, `import . "path"`,
/// `import _ "path"`. Only named/aliased and plain imports bind a
/// package identifier; dot/blank imports bind nothing.
fn walk_import(node: Node, st: &mut State) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() || child.kind() != "import_spec" {
            continue;
        }
        if let Some(alias) = child.child_by_field_name("name") {
            let n = text(st.contents, alias);
            if n != "_" && n != "." {
                bind(st, n, alias.start_byte()..alias.end_byte(), true);
            }
            continue;
        }
        if let Some(path) = child.child_by_field_name("path") {
            // Default package name = last path segment.
            let raw = text(st.contents, path);
            let inner = raw.trim_matches(|c| c == '"' || c == '`');
            if let Some(seg) = inner.rsplit('/').next()
                && !seg.is_empty()
            {
                // The binding range is approximated by the path literal;
                // the package identifier has no source token of its own.
                // Use the path's inner content range for a stable anchor.
                let start = path.start_byte() + raw.find(inner).unwrap_or(0);
                bind(st, seg, start..start + seg.len(), true);
            }
        }
    }
}
