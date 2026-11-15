//! Rust binding analysis over the tree-sitter CST. Hand-rolled scope
//! resolution: module/block/function/impl/trait/enum/match-arm scopes in a
//! persistent arena, item hoisting via per-container pre-scans, `let` /
//! pattern / closure / `for` bindings, `use` trees, module-path
//! resolution (`m::f`, `self::x`, `crate::y`), struct shorthand, and
//! method-call identifiers. Macros are opaque: every identifier inside a
//! `token_tree` is recorded as an *unresolved* reference, so any rename
//! whose old spelling appears in a macro is blocked rather than silently
//! incomplete. `unresolved_is_exact` is therefore always `false`.

use std::collections::HashMap;
use std::ops::Range;

use ast_grep_core::tree_sitter::LanguageExt;
use tree_sitter::{Node, Parser};

use crate::bind::{Analysis, BindingSite, Expansion, RefSite};
use crate::lang::LangSpec;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ScopeKind {
    Module,
    Block,
    Function,
    Impl,
    Trait,
    Enum,
    Arm,
    Closure,
}

struct ArenaScope {
    kind: ScopeKind,
    parent: Option<usize>,
    bindings: HashMap<String, usize>,
}

struct State<'a> {
    analysis: Analysis,
    arena: Vec<ArenaScope>,
    /// Stack of arena scope ids, innermost last.
    stack: Vec<usize>,
    /// Arena scope id for each binding, aligned with `analysis.bindings`.
    binding_scope: Vec<usize>,
    /// Inner item-namespace scope for path-able items (mod/trait/enum),
    /// aligned with `analysis.bindings`.
    binding_inner: Vec<Option<usize>>,
    contents: &'a [u8],
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
        arena: Vec::new(),
        stack: Vec::new(),
        binding_scope: Vec::new(),
        binding_inner: Vec::new(),
        contents: contents.as_bytes(),
    };
    walk(root, &mut st);
    st.analysis.unresolved_is_exact = false;
    Some(st.analysis)
}

fn text<'a>(contents: &'a [u8], node: Node) -> &'a str {
    node.utf8_text(contents).unwrap_or("")
}

fn is_ident_kind(kind: &str) -> bool {
    matches!(kind, "identifier" | "type_identifier")
}

fn push_scope(st: &mut State, kind: ScopeKind) -> usize {
    let id = st.arena.len();
    st.arena.push(ArenaScope {
        kind,
        parent: st.stack.last().copied(),
        bindings: HashMap::new(),
    });
    st.stack.push(id);
    id
}

fn cur_scope(st: &State) -> usize {
    *st.stack.last().expect("scope stack")
}

fn walk_function(node: Node, st: &mut State) {
    // The name was bound by the enclosing container's item prescan.
    push_scope(st, ScopeKind::Function);
    for field in ["type_parameters", "parameters", "return_type", "where_clause"] {
        if let Some(f) = node.child_by_field_name(field) {
            walk(f, st);
        }
    }
    if let Some(body) = node.child_by_field_name("body") {
        walk(body, st);
    }
    st.stack.pop();
}

fn walk(node: Node, st: &mut State) {
    match node.kind() {
        "function_item" => walk_function(node, st),
        "function_signature_item" => {
            walk_children_except(node, st, &["name"]);
        }
        "parameter" => {
            if let Some(pattern) = node.child_by_field_name("pattern") {
                walk_pattern(pattern, st);
            }
            if let Some(t) = node.child_by_field_name("type") {
                walk(t, st);
            }
        }
        "self_parameter" | "variadic_parameter" | "lifetime_parameter" => {}
        "type_parameter" => {
            if let Some(name) = node.child_by_field_name("name") {
                bind_here(st, text(st.contents, name), name.start_byte()..name.end_byte());
            }
            walk_children_except(node, st, &["name"]);
        }
        "closure_expression" => {
            push_scope(st, ScopeKind::Closure);
            if let Some(params) = node.child_by_field_name("parameters") {
                let mut cursor = params.walk();
                for child in params.children(&mut cursor) {
                    if !child.is_named() {
                        continue;
                    }
                    if child.kind() == "parameter" {
                        walk(child, st);
                    } else {
                        walk_pattern(child, st);
                    }
                }
            }
            if let Some(body) = node.child_by_field_name("body") {
                walk(body, st);
            }
            if let Some(rt) = node.child_by_field_name("return_type") {
                walk(rt, st);
            }
            st.stack.pop();
        }
        "struct_item" | "union_item" => {
            walk_children_except(node, st, &["name"]);
        }
        "enum_item" => {
            let inner = push_scope(st, ScopeKind::Enum);
            link_inner_scope(st, node, inner);
            walk_children_except(node, st, &["name"]);
            st.stack.pop();
        }
        "enum_variant" => {
            if let Some(name) = node.child_by_field_name("name") {
                bind_here(st, text(st.contents, name), name.start_byte()..name.end_byte());
            }
            if let Some(value) = node.child_by_field_name("value") {
                walk(value, st);
            }
            walk_children_except(node, st, &["name", "value"]);
        }
        "impl_item" => {
            for field in ["trait", "type", "type_parameters"] {
                if let Some(f) = node.child_by_field_name(field) {
                    walk(f, st);
                }
            }
            if let Some(body) = node.child_by_field_name("body") {
                walk_container(body, ScopeKind::Impl, st);
            }
        }
        "trait_item" => {
            for field in ["bounds", "type_parameters"] {
                if let Some(f) = node.child_by_field_name(field) {
                    walk(f, st);
                }
            }
            if let Some(body) = node.child_by_field_name("body") {
                let inner = push_scope(st, ScopeKind::Trait);
                link_inner_scope(st, node, inner);
                prescan_items(body, st);
                walk_children(body, st);
                st.stack.pop();
            }
        }
        "mod_item" => {
            if let Some(body) = node.child_by_field_name("body") {
                let inner = push_scope(st, ScopeKind::Module);
                link_inner_scope(st, node, inner);
                prescan_items(body, st);
                walk_children(body, st);
                st.stack.pop();
            }
        }
        "type_item" | "const_item" | "static_item" | "macro_definition" => {
            walk_children_except(node, st, &["name"]);
        }
        "associated_type" => {
            walk_children_except(node, st, &["name"]);
        }
        "type_binding" => {
            if let Some(t) = node.child_by_field_name("type") {
                walk(t, st);
            }
        }
        "use_declaration" => walk_use(node, st),
        "let_declaration" | "let_condition" => {
            // value evaluates before the pattern binds
            if let Some(value) = node.child_by_field_name("value") {
                walk(value, st);
            }
            if let Some(pattern) = node.child_by_field_name("pattern") {
                walk_pattern(pattern, st);
            }
            for field in ["type", "alternative"] {
                if let Some(f) = node.child_by_field_name(field) {
                    walk(f, st);
                }
            }
        }
        "for_expression" => {
            if let Some(value) = node.child_by_field_name("value") {
                walk(value, st);
            }
            push_scope(st, ScopeKind::Block);
            if let Some(pattern) = node.child_by_field_name("pattern") {
                walk_pattern(pattern, st);
            }
            if let Some(body) = node.child_by_field_name("body") {
                walk(body, st);
            }
            st.stack.pop();
        }
        "if_expression" => {
            push_scope(st, ScopeKind::Block);
            if let Some(cond) = node.child_by_field_name("condition") {
                walk(cond, st);
            }
            if let Some(cons) = node.child_by_field_name("consequence") {
                walk(cons, st);
            }
            st.stack.pop();
            if let Some(alt) = node.child_by_field_name("alternative") {
                walk(alt, st);
            }
        }
        "while_expression" => {
            push_scope(st, ScopeKind::Block);
            if let Some(cond) = node.child_by_field_name("condition") {
                walk(cond, st);
            }
            if let Some(body) = node.child_by_field_name("body") {
                walk(body, st);
            }
            st.stack.pop();
        }
        "match_expression" => walk_children(node, st),
        "match_arm" => {
            push_scope(st, ScopeKind::Arm);
            if let Some(pattern) = node.child_by_field_name("pattern") {
                walk_match_pattern(pattern, st);
            }
            walk_children_except(node, st, &["pattern"]);
            st.stack.pop();
        }
        "block" => walk_container(node, ScopeKind::Block, st),
        "declaration_list" => walk_children(node, st),
        "source_file" => walk_container(node, ScopeKind::Module, st),
        "struct_expression" => {
            if let Some(name) = node.child_by_field_name("name") {
                walk(name, st);
            }
            if let Some(body) = node.child_by_field_name("body") {
                walk_field_inits(body, st);
            }
        }
        "field_expression" => {
            if let Some(value) = node.child_by_field_name("value") {
                walk(value, st);
            }
            if is_method_call(node)
                && let Some(field) = node.child_by_field_name("field")
            {
                record_method_ref(
                    st,
                    text(st.contents, field),
                    field.start_byte()..field.end_byte(),
                );
            }
        }
        "scoped_identifier" | "scoped_type_identifier" => {
            walk_path(node, st, None, None);
        }
        "generic_function" => {
            if let Some(function) = node.child_by_field_name("function") {
                walk(function, st);
            }
            if let Some(t) = node.child_by_field_name("type_arguments") {
                walk(t, st);
            }
        }
        "macro_invocation" => {
            if let Some(mac) = node.child_by_field_name("macro") {
                walk(mac, st);
            }
            for child in node.children(&mut node.walk()) {
                if child.kind() == "token_tree" {
                    record_macro_tokens(child, st);
                }
            }
        }
        "identifier" | "type_identifier" => {
            let name = text(st.contents, node).to_string();
            record_ref(st, &name, node.start_byte()..node.end_byte());
        }
        "shorthand_field_identifier" => {
            let name = text(st.contents, node).to_string();
            let range = node.start_byte()..node.end_byte();
            let target = resolve(st, &name, cur_scope(st));
            push_ref(st, range, target, Some(Expansion::ShorthandColon), false);
        }
        "field_identifier" | "self" | "crate" | "super" | "metavariable"
        | "primitive_type" | "lifetime" | "integer_literal" | "float_literal"
        | "string_literal" | "char_literal" | "boolean_literal" | "line_comment"
        | "block_comment" => {}
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

fn walk_children_except(node: Node, st: &mut State, fields: &[&str]) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        let skip = fields.iter().any(|f| node.child_by_field_name(f) == Some(child));
        if !skip {
            walk(child, st);
        }
    }
}

fn is_method_call(node: Node) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    match parent.kind() {
        "call_expression" | "generic_function" => {
            parent.child_by_field_name("function") == Some(node)
        }
        _ => false,
    }
}

/// Container that may directly hold items: prescan binds item names
/// (hoisting) then children are walked normally.
fn walk_container(node: Node, kind: ScopeKind, st: &mut State) {
    push_scope(st, kind);
    prescan_items(node, st);
    walk_children(node, st);
    st.stack.pop();
}

/// Bind names of item declarations that are direct children of `node`.
fn prescan_items(node: Node, st: &mut State) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        match child.kind() {
            "function_item" | "struct_item" | "enum_item" | "union_item"
            | "trait_item" | "mod_item" | "type_item" | "const_item"
            | "static_item" | "macro_definition" => {
                if let Some(name) = item_name(child) {
                    bind_here(st, text(st.contents, name), name.start_byte()..name.end_byte());
                }
            }
            "use_declaration" => prescan_use(child, st),
            "enum_variant" => {
                if let Some(name) = child.child_by_field_name("name") {
                    bind_here(st, text(st.contents, name), name.start_byte()..name.end_byte());
                }
            }
            _ => {}
        }
    }
}

fn item_name(node: Node) -> Option<Node> {
    node.child_by_field_name("name")
}

/// Record the inner item-namespace scope for a path-able item binding
/// (`mod m`, `trait T`, `enum E`) so `m::x` / `T::x` / `E::V` resolve.
fn link_inner_scope(st: &mut State, item: Node, inner: usize) {
    let Some(name) = item_name(item) else {
        return;
    };
    let name_text = text(st.contents, name).to_string();
    // The binding lives in the parent scope (stack top before push is
    // now second-to-top).
    if st.stack.len() < 2 {
        return;
    }
    let parent = st.stack[st.stack.len() - 2];
    if let Some(&bidx) = st.arena[parent].bindings.get(&name_text) {
        st.binding_inner[bidx] = Some(inner);
    }
}

// ---------- patterns ----------

fn walk_pattern(node: Node, st: &mut State) {
    match node.kind() {
        "identifier" => {
            bind_here(st, text(st.contents, node), node.start_byte()..node.end_byte());
        }
        "mut_pattern" | "ref_pattern" | "captured_pattern" | "or_pattern"
        | "slice_pattern" | "tuple_pattern" | "reference_pattern" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named() {
                    walk_pattern(child, st);
                }
            }
        }
        "struct_pattern" => {
            if let Some(t) = node.child_by_field_name("type") {
                walk(t, st);
            }
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if !child.is_named() {
                    continue;
                }
                if Some(child) == node.child_by_field_name("type") {
                    continue;
                }
                walk_pattern(child, st);
            }
        }
        "field_pattern" => {
            if let Some(name) = node.child_by_field_name("name")
                && name.kind() == "shorthand_field_identifier"
            {
                bind_expansion(
                    st,
                    text(st.contents, name),
                    name.start_byte()..name.end_byte(),
                    Some(Expansion::ShorthandColon),
                );
            }
            if let Some(pattern) = node.child_by_field_name("pattern") {
                walk_pattern(pattern, st);
            }
        }
        "tuple_struct_pattern" => {
            if let Some(t) = node.child_by_field_name("type") {
                walk(t, st);
            }
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named() && Some(child) != node.child_by_field_name("type") {
                    walk_pattern(child, st);
                }
            }
        }
        "range_pattern" => {
            for field in ["left", "right"] {
                if let Some(f) = node.child_by_field_name(field) {
                    walk(f, st);
                }
            }
        }
        "_" | "remaining_field_pattern" | "literal" | "integer_literal"
        | "float_literal" | "string_literal" | "char_literal" | "boolean_literal" => {}
        _ => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named() {
                    walk_pattern(child, st);
                }
            }
        }
    }
}

fn walk_match_pattern(node: Node, st: &mut State) {
    if node.kind() == "match_pattern" {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.is_named() {
                walk_match_pattern(child, st);
            }
        }
        return;
    }
    walk_pattern(node, st);
}

// ---------- struct expression field inits ----------

fn walk_field_inits(node: Node, st: &mut State) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        match child.kind() {
            "field_initializer" => {
                if let Some(value) = child.child_by_field_name("value") {
                    walk(value, st);
                }
            }
            "shorthand_field_initializer" => {
                let mut c2 = child.walk();
                for gc in child.children(&mut c2) {
                    if !gc.is_named() {
                        continue;
                    }
                    if gc.kind() == "identifier" {
                        let name = text(st.contents, gc).to_string();
                        let range = gc.start_byte()..gc.end_byte();
                        let target = resolve(st, &name, cur_scope(st));
                        push_ref(st, range, target, Some(Expansion::ShorthandColon), false);
                    } else {
                        walk(gc, st);
                    }
                }
            }
            _ => walk(child, st),
        }
    }
}

// ---------- paths ----------

/// Collect path segments outermost-first, including `self`/`crate`/
/// `super` base markers.
fn path_segments(node: Node) -> Vec<Node> {
    let mut segments = Vec::new();
    let mut cur = Some(node);
    while let Some(n) = cur {
        if let Some(nm) = n.child_by_field_name("name") {
            segments.push(nm);
        }
        let path = n.child_by_field_name("path");
        match path {
            Some(p) if matches!(p.kind(), "scoped_identifier" | "scoped_type_identifier") => {
                cur = Some(p);
            }
            Some(p) => {
                segments.push(p);
                break;
            }
            None => break,
        }
    }
    segments.reverse();
    segments
}

/// Record refs for every segment of a `a::b::C` path, resolving stepwise:
/// the first segment resolves lexically (or against the module named by
/// `self`/`crate`/`super`, or inside `base` for `use` list items), each
/// later segment resolves inside the previous binding's item namespace
/// when one exists. Unresolved tails of anchored paths may be assoc items
/// and must block renames; tails of unanchored paths are `remote`.
/// `tail_expansion` is applied to the last identifier segment — used for
/// un-aliased `use` bindings so renaming the item preserves the local
/// name (`use m::target` → `use m::renamed as target`).
/// Returns the item-namespace scope of the last resolved binding.
fn walk_path(
    node: Node,
    st: &mut State,
    base: Option<usize>,
    tail_expansion: Option<Expansion>,
) -> Option<usize> {
    let segments = path_segments(node);
    let ident_count = segments
        .iter()
        .filter(|s| is_ident_kind(s.kind()))
        .count();
    let mut seen_idents = 0;
    let mut scope: Option<usize> = base;
    let mut prev_inner: Option<usize> = None;
    let mut first_real = true;
    let mut anchored = base.is_some();
    let mut last_inner = None;
    for seg in segments.iter() {
        if !seg.is_named() {
            continue;
        }
        match seg.kind() {
            "self" => {
                scope = nearest_scope_of_kind(st, ScopeKind::Module);
                prev_inner = None;
                first_real = true;
                anchored = scope.is_some();
                continue;
            }
            "crate" => {
                scope = crate_root_scope(st);
                prev_inner = None;
                first_real = true;
                anchored = scope.is_some();
                continue;
            }
            "super" => {
                scope = parent_module_scope(st);
                prev_inner = None;
                first_real = true;
                anchored = scope.is_some();
                continue;
            }
            _ => {}
        }
        if !is_ident_kind(seg.kind()) {
            continue;
        }
        seen_idents += 1;
        let is_tail = seen_idents == ident_count;
        let name = text(st.contents, *seg).to_string();
        let range = seg.start_byte()..seg.end_byte();
        let (target, remote) = if first_real {
            first_real = false;
            let from = scope.unwrap_or_else(|| cur_scope(st));
            let t = resolve(st, &name, from);
            anchored = anchored || t.is_some();
            (t, false)
        } else {
            let t = prev_inner.and_then(|inner| st.arena[inner].bindings.get(&name).copied());
            // Unresolved tails of anchored paths may be assoc items —
            // they must block. Tails of unanchored paths are remote.
            (t, t.is_none() && !anchored)
        };
        let inner = target.and_then(|t| st.binding_inner[t]);
        prev_inner = inner;
        if target.is_some() {
            last_inner = inner;
        }
        let exp = if is_tail { tail_expansion } else { None };
        push_ref(st, range, target, exp, remote);
    }
    last_inner
}

fn nearest_scope_of_kind(st: &State, kind: ScopeKind) -> Option<usize> {
    st.stack
        .iter()
        .rev()
        .copied()
        .find(|&id| st.arena[id].kind == kind)
}

fn crate_root_scope(st: &State) -> Option<usize> {
    st.arena
        .iter()
        .position(|s| s.parent.is_none())
        .or_else(|| st.stack.first().copied())
}

fn parent_module_scope(st: &State) -> Option<usize> {
    let mut it = st
        .stack
        .iter()
        .rev()
        .copied()
        .filter(|&id| st.arena[id].kind == ScopeKind::Module);
    it.next()?;
    it.next()
}

// ---------- use trees ----------

/// `use` bindings are prescanned; the main walk records the path as
/// refs — including the bound tail segment, which refers to the item in
/// the source module (so renaming that item is caught).
fn walk_use(node: Node, st: &mut State) {
    if let Some(arg) = node.child_by_field_name("argument") {
        walk_use_arg_refs(arg, st, None);
    }
}

/// `base` is the item-namespace scope inherited from an enclosing
/// `use a::{...}` prefix. Un-aliased bound names get `ExportAlias`
/// expansion so renaming the source item preserves the local binding.
fn walk_use_arg_refs(node: Node, st: &mut State, base: Option<usize>) {
    match node.kind() {
        "use_as_clause" => {
            if let Some(path) = node.child_by_field_name("path") {
                if matches!(path.kind(), "scoped_identifier" | "scoped_type_identifier") {
                    walk_path(path, st, None, None);
                } else {
                    walk_use_arg_refs(path, st, base);
                }
            }
        }
        "use_wildcard" => {}
        "use_list" | "scoped_use_list" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named() {
                    walk_use_arg_refs(child, st, base);
                }
            }
        }
        "scoped_identifier" | "scoped_type_identifier" => {
            let name = node.child_by_field_name("name");
            let name_is_ident = name.is_some_and(|n| is_ident_kind(n.kind()));
            let tail = if name_is_ident {
                Some(Expansion::ExportAlias)
            } else {
                None
            };
            let inner = walk_path(node, st, base, tail);
            // `use a::{b, c}` — the name field may itself be a list whose
            // items resolve inside this path's last binding.
            if let Some(nm) = name
                && matches!(nm.kind(), "use_list" | "scoped_use_list")
            {
                walk_use_arg_refs(nm, st, inner);
            }
        }
        "identifier" | "type_identifier" => {
            if let Some(inner) = base {
                // leaf inside `use a::{x}` — resolves in a's namespace
                let name = text(st.contents, node).to_string();
                let range = node.start_byte()..node.end_byte();
                let target = st.arena[inner].bindings.get(&name).copied();
                let anchored = target.is_some();
                push_ref(
                    st,
                    range,
                    target,
                    Some(Expansion::ExportAlias),
                    target.is_none() && !anchored,
                );
            } else {
                // `use foo` — external crate root; never a lexical binding.
                push_ref(st, node.start_byte()..node.end_byte(), None, None, true);
            }
        }
        _ => {}
    }
}

/// Prescan half of use_declaration: bind names without recording refs.
fn prescan_use(node: Node, st: &mut State) {
    if let Some(arg) = node.child_by_field_name("argument") {
        prescan_use_arg(arg, st);
    }
}

fn prescan_use_arg(node: Node, st: &mut State) {
    match node.kind() {
        "use_as_clause" => {
            if let Some(alias) = node.child_by_field_name("alias") {
                bind_here(st, text(st.contents, alias), alias.start_byte()..alias.end_byte());
            }
        }
        "use_list" | "scoped_use_list" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.is_named() {
                    prescan_use_arg(child, st);
                }
            }
        }
        "scoped_identifier" | "scoped_type_identifier" => {
            if let Some(name) = node.child_by_field_name("name") {
                if matches!(name.kind(), "use_list" | "scoped_use_list") {
                    prescan_use_arg(name, st);
                } else if is_ident_kind(name.kind()) {
                    bind_imported(st, text(st.contents, name), name.start_byte()..name.end_byte());
                }
            }
        }
        "identifier" | "type_identifier" => {
            bind_imported(st, text(st.contents, node), node.start_byte()..node.end_byte());
        }
        _ => {}
    }
}

// ---------- bindings / refs / macros ----------

fn bind_here(st: &mut State, name: &str, range: Range<usize>) {
    bind_expansion(st, name, range, None);
}

fn bind_expansion(st: &mut State, name: &str, range: Range<usize>, expansion: Option<Expansion>) {
    let binding_idx = st.analysis.bindings.len();
    st.analysis.bindings.push(BindingSite {
        name: range,
        text: name.to_string(),
        expansion,
    });
    st.binding_scope.push(cur_scope(st));
    st.binding_inner.push(None);
    let scope = cur_scope(st);
    st.arena[scope].bindings.insert(name.to_string(), binding_idx);
}

fn bind_imported(st: &mut State, name: &str, range: Range<usize>) {
    bind_expansion(st, name, range, Some(Expansion::ImportAlias));
}

fn push_ref(
    st: &mut State,
    range: Range<usize>,
    target: Option<usize>,
    expansion: Option<Expansion>,
    remote: bool,
) {
    st.analysis.refs.push(RefSite {
        name: range,
        target,
        expansion,
        remote,
    });
}

fn record_ref(st: &mut State, name: &str, range: Range<usize>) {
    let target = resolve(st, name, cur_scope(st));
    push_ref(st, range, target, None, false);
}

fn record_method_ref(st: &mut State, name: &str, range: Range<usize>) {
    let mut target = None;
    for &id in st.stack.iter().rev() {
        if matches!(st.arena[id].kind, ScopeKind::Impl | ScopeKind::Trait)
            && let Some(&b) = st.arena[id].bindings.get(name)
        {
            target = Some(b);
            break;
        }
    }
    push_ref(st, range, target, None, false);
}

fn resolve(st: &State, name: &str, from_scope: usize) -> Option<usize> {
    let mut cur = Some(from_scope);
    while let Some(id) = cur {
        if let Some(&b) = st.arena[id].bindings.get(name) {
            return Some(b);
        }
        cur = st.arena[id].parent;
    }
    None
}

/// Every identifier token inside a macro token_tree is recorded as an
/// unresolved reference: macro expansion semantics are unknowable
/// statically, so any rename touching that spelling must block.
fn record_macro_tokens(node: Node, st: &mut State) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        match child.kind() {
            "identifier" | "type_identifier" => {
                push_ref(st, child.start_byte()..child.end_byte(), None, None, false);
            }
            "token_tree" => record_macro_tokens(child, st),
            "token_repetition" => {
                let mut c2 = child.walk();
                for gc in child.children(&mut c2) {
                    if gc.is_named() {
                        record_macro_tokens(gc, st);
                    }
                }
            }
            _ => {}
        }
    }
}
