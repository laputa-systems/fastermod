//! Python binding analysis over the tree-sitter CST. Hand-rolled scope
//! resolution for Python's scoping rules: module / function / lambda /
//! class / comprehension scopes, `global`/`nonlocal` redirects, walrus
//! targets, and `import *` pessimism. Match-statement patterns are opaque:
//! their capture bindings are unmodeled, which can only make resolution
//! incomplete — post-edit verification still guards correctness.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use ast_grep_core::tree_sitter::LanguageExt;
use tree_sitter::{Node, Parser};

use crate::bind::{Analysis, BindingSite, RefSite};
use crate::lang::LangSpec;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ScopeKind {
    Module,
    Function,
    Lambda,
    Class,
    Comprehension,
}

struct Scope {
    kind: ScopeKind,
    bindings: HashMap<String, usize>,
    globals: HashSet<String>,
    nonlocals: HashSet<String>,
}

impl Scope {
    fn new(kind: ScopeKind) -> Self {
        Self {
            kind,
            bindings: HashMap::new(),
            globals: HashSet::new(),
            nonlocals: HashSet::new(),
        }
    }
}

pub(crate) fn analyze(
    spec: &LangSpec,
    contents: &str,
    _path: &std::path::Path,
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
        scopes: vec![Scope::new(ScopeKind::Module)],
        has_match: false,
        has_star_import: false,
        contents: contents.as_bytes(),
    };
    walk(root, &mut st);
    st.analysis.unresolved_is_exact = !(st.has_match || st.has_star_import);
    Some(st.analysis)
}

struct State<'a> {
    analysis: Analysis,
    scopes: Vec<Scope>,
    has_match: bool,
    has_star_import: bool,
    contents: &'a [u8],
}

fn text<'a>(contents: &'a [u8], node: Node) -> &'a str {
    node.utf8_text(contents).unwrap_or("")
}

fn walk(node: Node, st: &mut State) {
    match node.kind() {
        "function_definition" => walk_function(node, st),
        "lambda" => walk_lambda(node, st),
        "class_definition" => walk_class(node, st),
        "list_comprehension" | "set_comprehension" | "dictionary_comprehension"
        | "generator_expression" => {
            st.scopes.push(Scope::new(ScopeKind::Comprehension));
            walk_children(node, st);
            st.scopes.pop();
        }
        "match_statement" => {
            st.has_match = true;
            if let Some(subject) = node.child_by_field_name("subject") {
                walk(subject, st);
            }
            for child in node.children(&mut node.walk()) {
                if child.kind() == "block" {
                    walk(child, st);
                }
            }
        }
        "case_clause" => {
            // Opaque patterns: captures are unmodeled bindings; value
            // patterns are unmodeled refs. Skipping can only lose
            // references, which post-edit verification catches.
            for child in node.children(&mut node.walk()) {
                if matches!(child.kind(), "block" | "if_clause") {
                    walk(child, st);
                }
            }
        }
        "assignment" => {
            if let Some(left) = node.child_by_field_name("left") {
                walk_pattern(left, st);
            }
            if let Some(t) = node.child_by_field_name("type") {
                walk(t, st);
            }
            if let Some(right) = node.child_by_field_name("right") {
                walk(right, st);
            }
        }
        "augmented_assignment" => {
            // `x += 1` loads then stores the existing binding; it does not
            // introduce a new declaration site.
            if let Some(left) = node.child_by_field_name("left") {
                walk(left, st);
            }
            if let Some(right) = node.child_by_field_name("right") {
                walk(right, st);
            }
        }
        "for_statement" => {
            if let Some(left) = node.child_by_field_name("left") {
                walk_pattern(left, st);
            }
            if let Some(right) = node.child_by_field_name("right") {
                walk(right, st);
            }
            if let Some(body) = node.child_by_field_name("body") {
                walk(body, st);
            }
            if let Some(alt) = node.child_by_field_name("alternative") {
                walk(alt, st);
            }
        }
        "for_in_clause" => {
            if let Some(left) = node.child_by_field_name("left") {
                walk_pattern(left, st);
            }
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named() && Some(child) != node.child_by_field_name("left") {
                    walk(child, st);
                }
            }
        }
        "with_item" => {
            // value may be an `as_pattern` (expr as target); walk handles it.
            if let Some(value) = node.child_by_field_name("value") {
                walk(value, st);
            }
        }
        "as_pattern" => {
            // children: expression, 'as', as_pattern_target
            let mut cursor = node.walk();
            let children: Vec<_> = node.children(&mut cursor).collect();
            for (i, child) in children.iter().enumerate() {
                if child.kind() == "as_pattern_target" {
                    walk_pattern(*child, st);
                } else if (i == 0 || child.is_named()) && child.kind() != "as" {
                    walk(*child, st);
                }
            }
        }
        "as_pattern_target" => walk_pattern(node, st),
        "except_clause" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if !child.is_named() {
                    continue;
                }
                match child.kind() {
                    "block" => walk(child, st),
                    _ => {
                        // value or alias field — distinguish by field name
                        if node.child_by_field_name("alias") == Some(child) {
                            walk_pattern(child, st);
                        } else {
                            walk(child, st);
                        }
                    }
                }
            }
        }
        "import_statement" => walk_import(node, st, false),
        "import_from_statement" => walk_import(node, st, true),
        "global_statement" | "nonlocal_statement" => {
            let is_global = node.kind() == "global_statement";
            for child in node.children(&mut node.walk()) {
                if child.is_named() && child.kind() == "identifier" {
                    let name = text(st.contents, child).to_string();
                    let target = resolve_redirected(st, &name, is_global);
                    st.analysis.refs.push(RefSite {
                        name: child.start_byte()..child.end_byte(),
                        target,
                        expansion: None,
                        remote: false,
                    });
                }
            }
            if let Some(scope) = st
                .scopes
                .iter_mut()
                .rev()
                .find(|s| matches!(s.kind, ScopeKind::Function | ScopeKind::Lambda))
            {
                for child in node.children(&mut node.walk()) {
                    if child.is_named() && child.kind() == "identifier" {
                        let name = text(st.contents, child).to_string();
                        if is_global {
                            scope.globals.insert(name);
                        } else {
                            scope.nonlocals.insert(name);
                        }
                    }
                }
            }
        }
        "named_expression" => {
            if let Some(name) = node.child_by_field_name("name") {
                // Walrus in a comprehension binds in the containing scope.
                bind_target(st, text(st.contents, name), name.start_byte()..name.end_byte(), true);
            }
            if let Some(value) = node.child_by_field_name("value") {
                walk(value, st);
            }
        }
        "type_alias_statement" => {
            if let Some(left) = node.child_by_field_name("left") {
                // `type` node wraps the alias name identifier
                let mut cursor = left.walk();
                for child in left.children(&mut cursor) {
                    if child.is_named() {
                        if child.kind() == "identifier" {
                            bind_here(st, text(st.contents, child), child.start_byte()..child.end_byte());
                        } else {
                            walk(child, st);
                        }
                    }
                }
            }
            if let Some(right) = node.child_by_field_name("right") {
                walk(right, st);
            }
        }
        "identifier" => {
            let name = text(st.contents, node).to_string();
            record_ref(st, &name, node.start_byte()..node.end_byte());
        }
        "attribute" => {
            // object is a load ref; the attribute name identifier is not.
            if let Some(object) = node.child_by_field_name("object") {
                walk(object, st);
            }
            // attribute field intentionally skipped.
        }
        "keyword_argument" => {
            // name field is not a ref; value is.
            if let Some(value) = node.child_by_field_name("value") {
                walk(value, st);
            }
        }
        _ => walk_children(node, st),
    }
}

fn walk_children(node: Node, st: &mut State) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() {
            walk(child, st);
        }
    }
}

/// Binding-target pattern walk: bare identifiers bind; attribute /
/// subscript / call targets are load-context expressions.
fn walk_pattern(node: Node, st: &mut State) {
    match node.kind() {
        "identifier" | "as_pattern_target" => {
            if node.child_count() == 0 {
                let name = text(st.contents, node).to_string();
                bind_here(st, &name, node.start_byte()..node.end_byte());
            } else {
                // aliased pattern (e.g. tuple target): recurse
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.is_named() {
                        walk_pattern(child, st);
                    }
                }
            }
        }
        "tuple_pattern" | "list_pattern" | "pattern_list" | "parenthesized_expression"
        | "splat_pattern" | "list_splat_pattern" | "dictionary_splat_pattern"
        | "list_splat" | "type" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named() {
                    walk_pattern(child, st);
                }
            }
        }
        "attribute" => {
            if let Some(object) = node.child_by_field_name("object") {
                walk(object, st);
            }
        }
        _ => {
            // subscript, call, literals: everything inside is load context
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named() {
                    walk(child, st);
                }
            }
        }
    }
}

fn walk_function(node: Node, st: &mut State) {
    // name binds in the enclosing scope
    if let Some(name) = node.child_by_field_name("name") {
        bind_here(st, text(st.contents, name), name.start_byte()..name.end_byte());
    }
    // decorators, type params, defaults and annotations evaluate in the
    // enclosing scope — walk everything except parameters/body first.
    let params = node.child_by_field_name("parameters");
    let body = node.child_by_field_name("body");
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        if Some(child) == params || Some(child) == body {
            continue;
        }
        walk(child, st);
    }
    // parameter defaults/annotations are enclosing-scope refs; param
    // names bind in the new function scope.
    let mut param_names: Vec<(String, Range<usize>)> = Vec::new();
    if let Some(p) = params {
        collect_params(p, st, &mut param_names);
        walk_params_refs(p, st, &param_names);
    }
    // push scope, pre-scan body for global/nonlocal
    let mut scope = Scope::new(ScopeKind::Function);
    if let Some(b) = body {
        prescan_global_nonlocal(b, st.contents, &mut scope);
    }
    let scope_idx = st.scopes.len();
    st.scopes.push(scope);
    for (name, range) in param_names {
        let binding_idx = st.analysis.bindings.len();
        st.analysis.bindings.push(BindingSite {
            name: range,
            text: name.clone(),
            expansion: None,
        });
        st.scopes[scope_idx].bindings.insert(name, binding_idx);
    }
    if let Some(b) = body {
        walk(b, st);
    }
    st.scopes.pop();
}

fn walk_lambda(node: Node, st: &mut State) {
    let params = node.child_by_field_name("parameters");
    let mut param_names: Vec<(String, Range<usize>)> = Vec::new();
    if let Some(p) = params {
        collect_params(p, st, &mut param_names);
        walk_params_refs(p, st, &param_names);
    }
    let scope_idx = st.scopes.len();
    st.scopes.push(Scope::new(ScopeKind::Lambda));
    for (name, range) in param_names {
        let binding_idx = st.analysis.bindings.len();
        st.analysis.bindings.push(BindingSite {
            name: range,
            text: name.clone(),
            expansion: None,
        });
        st.scopes[scope_idx].bindings.insert(name, binding_idx);
    }
    // lambda body = all named children except parameters
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() && Some(child) != params {
            walk(child, st);
        }
    }
    st.scopes.pop();
}

fn walk_class(node: Node, st: &mut State) {
    if let Some(name) = node.child_by_field_name("name") {
        bind_here(st, text(st.contents, name), name.start_byte()..name.end_byte());
    }
    // superclasses/type params are enclosing-scope refs
    let body = node.child_by_field_name("body");
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() || Some(child) == body {
            continue;
        }
        walk(child, st);
    }
    st.scopes.push(Scope::new(ScopeKind::Class));
    if let Some(b) = body {
        walk(b, st);
    }
    st.scopes.pop();
}

/// Collect parameter binding names. `parameter` is a supertype; real
/// nodes are identifier / typed_parameter / default_parameter /
/// typed_default_parameter / tuple_pattern / splats.
fn collect_params(node: Node, st: &State, out: &mut Vec<(String, Range<usize>)>) {
    match node.kind() {
        "identifier" => out.push((text(st.contents, node).to_string(), node.start_byte()..node.end_byte())),
        "typed_parameter" | "typed_default_parameter" | "default_parameter" => {
            // name field when present; otherwise first identifier child
            if let Some(name) = node.child_by_field_name("name") {
                collect_binding_name(name, st, out);
            } else {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.is_named() && child.kind() == "identifier" {
                        out.push((
                            text(st.contents, child).to_string(),
                            child.start_byte()..child.end_byte(),
                        ));
                        break;
                    }
                }
            }
        }
        "tuple_pattern" | "list_pattern" | "pattern_list" | "parenthesized_expression"
        | "list_splat_pattern" | "dictionary_splat_pattern" | "splat_pattern" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named() {
                    collect_params(child, st, out);
                }
            }
        }
        _ => {}
    }
}

fn collect_binding_name(node: Node, st: &State, out: &mut Vec<(String, Range<usize>)>) {
    match node.kind() {
        "identifier" => out.push((text(st.contents, node).to_string(), node.start_byte()..node.end_byte())),
        _ => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named() {
                    collect_binding_name(child, st, out);
                }
            }
        }
    }
}

/// Walk a parameters subtree resolving load-context refs (defaults,
/// annotations, splat expressions) while skipping binding-name nodes.
fn walk_params_refs(node: Node, st: &mut State, names: &[(String, Range<usize>)]) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_param_child(child, st, names);
    }
}

fn walk_param_child(node: Node, st: &mut State, names: &[(String, Range<usize>)]) {
    let r = node.start_byte()..node.end_byte();
    if names.iter().any(|(_, nr)| *nr == r) {
        return;
    }
    match node.kind() {
        "typed_parameter" | "typed_default_parameter" | "default_parameter" => {
            // name already skipped by range; walk type/value subtrees
            for field in ["type", "value"] {
                if let Some(f) = node.child_by_field_name(field) {
                    walk(f, st);
                }
            }
            // name field may itself be a pattern — already collected;
            // any deeper identifiers inside it that aren't the binding
            // name are impossible, so nothing else to do.
        }
        "parameters" | "lambda_parameters" | "tuple_pattern" | "list_pattern"
        | "pattern_list" | "parenthesized_expression" | "list_splat_pattern"
        | "dictionary_splat_pattern" | "splat_pattern" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named() {
                    walk_param_child(child, st, names);
                }
            }
        }
        _ => walk(node, st),
    }
}

fn walk_import(node: Node, st: &mut State, from: bool) {
    let module_name = if from {
        node.child_by_field_name("module_name")
    } else {
        None
    };
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() || Some(child) == module_name {
            continue;
        }
        match child.kind() {
            "dotted_name" | "relative_import" => {
                // `import a` binds `a` but the module name must be
                // preserved: rename produces `import a as NEW`.
                // `import a.b` binds `a` yet `a.b as NEW` would rebind NEW
                // to the submodule — there is no local spelling that
                // preserves both, so no binding is recorded and uses of
                // `a` stay unresolved (blocking the rename).
                // `from m import a` binds `a` → `a as NEW` is correct.
                let idents = count_idents(child, st);
                if idents == 1 {
                    bind_imported(child, st);
                }
            }
            "aliased_import" => {
                if let Some(alias) = child.child_by_field_name("alias") {
                    bind_here(st, text(st.contents, alias), alias.start_byte()..alias.end_byte());
                } else if let Some(name) = child.child_by_field_name("name") {
                    bind_imported(name, st);
                }
            }
            "wildcard_import" => st.has_star_import = true,
            _ => {}
        }
    }
}

fn count_idents(node: Node, st: &State) -> usize {
    let mut n = 0;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        if child.kind() == "identifier" {
            n += 1;
        } else {
            n += count_idents(child, st);
        }
    }
    let _ = st;
    n
}

/// Bind an imported name that must keep its imported spelling on rename
/// (`OLD` → `OLD as NEW`).
fn bind_imported(node: Node, st: &mut State) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        if child.kind() == "identifier" {
            let name = text(st.contents, child).to_string();
            let range = child.start_byte()..child.end_byte();
            let scope_idx = bind_target_scope(st, &name, false);
            let binding_idx = st.analysis.bindings.len();
            st.analysis.bindings.push(BindingSite {
                name: range,
                text: name.clone(),
                expansion: Some(crate::bind::Expansion::ImportAlias),
            });
            st.scopes[scope_idx].bindings.insert(name, binding_idx);
            return;
        }
        bind_imported(child, st);
        return;
    }
}

fn bind_here(st: &mut State, name: &str, range: Range<usize>) {
    bind_target(st, name, range, false);
}

/// Binding site honoring `global`/`nonlocal` redirects. `skip_comprehension`
/// is used for walrus targets (they bind in the containing scope).
fn bind_target(st: &mut State, name: &str, range: Range<usize>, skip_comprehension: bool) {
    let scope_idx = bind_target_scope(st, name, skip_comprehension);
    let binding_idx = st.analysis.bindings.len();
    st.analysis.bindings.push(BindingSite {
        name: range,
        text: name.to_string(),
        expansion: None,
    });
    st.scopes[scope_idx].bindings.insert(name.to_string(), binding_idx);
}

fn bind_target_scope(st: &State, name: &str, skip_comprehension: bool) -> usize {
    for i in (0..st.scopes.len()).rev() {
        let s = &st.scopes[i];
        if s.kind == ScopeKind::Module {
            return 0;
        }
        if skip_comprehension && s.kind == ScopeKind::Comprehension {
            continue;
        }
        if s.globals.contains(name) {
            return 0;
        }
        if s.nonlocals.contains(name) {
            for j in (0..i).rev() {
                let t = &st.scopes[j];
                if matches!(
                    t.kind,
                    ScopeKind::Function | ScopeKind::Lambda | ScopeKind::Comprehension
                ) {
                    return j;
                }
            }
            return i;
        }
        return i;
    }
    0
}

fn resolve_redirected(st: &State, name: &str, is_global: bool) -> Option<usize> {
    if is_global {
        return st.scopes[0].bindings.get(name).copied();
    }
    for s in st.scopes.iter().rev().skip(1) {
        if matches!(s.kind, ScopeKind::Class | ScopeKind::Module) {
            continue;
        }
        if let Some(&b) = s.bindings.get(name) {
            return Some(b);
        }
    }
    None
}

fn resolve(st: &State, name: &str, from_scope: usize) -> Option<usize> {
    let mut i = from_scope;
    loop {
        let s = &st.scopes[i];
        if s.globals.contains(name) {
            return st.scopes[0].bindings.get(name).copied();
        }
        if s.nonlocals.contains(name) {
            for j in (0..i).rev() {
                let t = &st.scopes[j];
                if matches!(
                    t.kind,
                    ScopeKind::Function | ScopeKind::Lambda | ScopeKind::Comprehension
                ) && t.bindings.contains_key(name)
                {
                    return t.bindings.get(name).copied();
                }
            }
            return None;
        }
        // Class scopes are skipped for refs nested inside functions but
        // consulted for refs directly in the class body.
        let skip_class =
            st.scopes[from_scope].kind != ScopeKind::Class && s.kind == ScopeKind::Class;
        if !skip_class && let Some(&b) = s.bindings.get(name) {
            return Some(b);
        }
        if i == 0 {
            return None;
        }
        i -= 1;
    }
}

fn record_ref(st: &mut State, name: &str, range: Range<usize>) {
    let scope_idx = st.scopes.len() - 1;
    let target = resolve(st, name, scope_idx);
    st.analysis.refs.push(RefSite {
        name: range,
        target,
        expansion: None,
        remote: false,
    });
}

fn prescan_global_nonlocal(node: Node, contents: &[u8], scope: &mut Scope) {
    match node.kind() {
        "global_statement" | "nonlocal_statement" => {
            let is_global = node.kind() == "global_statement";
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named()
                    && child.kind() == "identifier"
                    && let Ok(name) = child.utf8_text(contents)
                {
                    if is_global {
                        scope.globals.insert(name.to_string());
                    } else {
                        scope.nonlocals.insert(name.to_string());
                    }
                }
            }
        }
        "function_definition" | "class_definition" | "lambda" => {}
        _ => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named() {
                    prescan_global_nonlocal(child, contents, scope);
                }
            }
        }
    }
}
