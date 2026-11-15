//! Ruby binding analysis via tree-sitter CST traversal.
//!
//! Ruby scopes: program → type (class/module bodies) → method →
//! block/lambda. Only blocks and lambdas nest lexically; `def` bodies
//! are isolated from outer locals (like PHP functions), while blocks
//! and lambdas are closures that see enclosing locals.
//!
//! Namespaces kept distinct by node kind and context:
//! - `identifier` locals: assignment targets, parameters, block params.
//! - method names: `method`/`singleton_method` declarations; reached by
//!   bare calls, `self.m()`, and unresolved-blocked `obj.m()`.
//! - `constant` names: class/module/constant declarations and refs.
//! - `@ivar`, `@@cvar`, `$gvar`: bound at the nearest type/program
//!   scope (ivars/cvars are per-object/class, shared across methods).
//!
//! Boundaries: `define_method`, `attr_accessor`-generated methods,
//! `method_missing`, `eval`, and `obj.m()` on unknown receivers are not
//! resolvable — `unresolved_is_exact` is false so unresolved same-name
//! occurrences block renames. `require`/`include` file resolution is
//! not implemented; module-level symbols block on sibling occurrences.

use std::collections::HashMap;
use std::path::Path;

use ast_grep_core::tree_sitter::LanguageExt;
use tree_sitter::{Node, Parser};

use crate::bind::{Analysis, BindingSite, RefSite};
use crate::lang::LangSpec;

#[derive(Clone, Copy, PartialEq)]
enum ScopeKind {
    Program,
    Type,
    /// `def` body: barrier for local-variable resolution.
    Method,
    /// do/{}/lambda bodies: closures, see enclosing scopes.
    Block,
    Lambda,
}

struct Scope {
    kind: ScopeKind,
    vars: HashMap<String, usize>,
    methods: HashMap<String, usize>,
    constants: HashMap<String, usize>,
    ivars: HashMap<String, usize>,
    cvars: HashMap<String, usize>,
    gvars: HashMap<String, usize>,
}

impl Scope {
    fn new(kind: ScopeKind) -> Scope {
        Scope {
            kind,
            vars: HashMap::new(),
            methods: HashMap::new(),
            constants: HashMap::new(),
            ivars: HashMap::new(),
            cvars: HashMap::new(),
            gvars: HashMap::new(),
        }
    }
}

struct State<'a> {
    analysis: Analysis,
    scopes: Vec<Scope>,
    contents: &'a [u8],
}

fn text<'a>(contents: &'a [u8], node: Node) -> &'a str {
    node.utf8_text(contents).unwrap_or("")
}

#[derive(Clone, Copy, PartialEq)]
enum Ns {
    Var,
    Method,
    Constant,
    Ivar,
    Cvar,
    Gvar,
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
        scopes: vec![Scope::new(ScopeKind::Program)],
        contents: contents.as_bytes(),
    };
    walk(root, &mut st);
    st.analysis.unresolved_is_exact = false;
    Some(st.analysis)
}

fn cur(st: &State) -> usize {
    st.scopes.len() - 1
}

fn bind(st: &mut State, name: &str, range: std::ops::Range<usize>, ns: Ns) {
    let kind = st.scopes[cur(st)].kind;
    let module_level = matches!(kind, ScopeKind::Program | ScopeKind::Type);
    let idx = st.analysis.bindings.len();
    st.analysis.bindings.push(BindingSite {
        name: range,
        text: name.to_string(),
        expansion: None,
        module_level,
        is_mod: false,
    });
    let si = cur(st);
    let map = match ns {
        Ns::Var => &mut st.scopes[si].vars,
        Ns::Method => &mut st.scopes[si].methods,
        Ns::Constant => &mut st.scopes[si].constants,
        Ns::Ivar => &mut st.scopes[si].ivars,
        Ns::Cvar => &mut st.scopes[si].cvars,
        Ns::Gvar => &mut st.scopes[si].gvars,
    };
    map.insert(name.to_string(), idx);
}

/// Bind into a specific scope index (used when the target scope is not
/// the innermost, e.g. ivars belong to the enclosing type).
fn bind_at(st: &mut State, si: usize, name: &str, range: std::ops::Range<usize>, ns: Ns) {
    let kind = st.scopes[si].kind;
    let module_level = matches!(kind, ScopeKind::Program | ScopeKind::Type);
    let idx = st.analysis.bindings.len();
    st.analysis.bindings.push(BindingSite {
        name: range,
        text: name.to_string(),
        expansion: None,
        module_level,
        is_mod: false,
    });
    let map = match ns {
        Ns::Var => &mut st.scopes[si].vars,
        Ns::Method => &mut st.scopes[si].methods,
        Ns::Constant => &mut st.scopes[si].constants,
        Ns::Ivar => &mut st.scopes[si].ivars,
        Ns::Cvar => &mut st.scopes[si].cvars,
        Ns::Gvar => &mut st.scopes[si].gvars,
    };
    map.insert(name.to_string(), idx);
}

/// Local variables: visible through blocks/lambdas but a `def` boundary
/// stops the search (methods cannot see outer locals).
fn lookup_var(st: &State, name: &str) -> Option<usize> {
    for s in st.scopes.iter().rev() {
        if let Some(&b) = s.vars.get(name) {
            return Some(b);
        }
        if s.kind == ScopeKind::Method {
            break;
        }
    }
    None
}

fn lookup(st: &State, name: &str, ns: Ns) -> Option<usize> {
    let pick = |s: &Scope| -> Option<usize> {
        match ns {
            Ns::Var => s.vars.get(name).copied(),
            Ns::Method => s.methods.get(name).copied(),
            Ns::Constant => s.constants.get(name).copied(),
            Ns::Ivar => s.ivars.get(name).copied(),
            Ns::Cvar => s.cvars.get(name).copied(),
            Ns::Gvar => s.gvars.get(name).copied(),
        }
    };
    match ns {
        Ns::Gvar => st.scopes[0].gvars.get(name).copied(),
        // ivars/cvars live at the nearest enclosing type/program scope.
        Ns::Ivar | Ns::Cvar => {
            for s in st.scopes.iter().rev() {
                if matches!(s.kind, ScopeKind::Type | ScopeKind::Program) {
                    if let Some(b) = pick(s) {
                        return Some(b);
                    }
                }
            }
            None
        }
        _ => st.scopes.iter().rev().find_map(pick),
    }
}

/// Nearest scope index that can hold ivars/cvars/methods/constants:
/// Type or Program (skips Method/Block/Lambda).
fn type_scope(st: &State) -> usize {
    st.scopes
        .iter()
        .rposition(|s| matches!(s.kind, ScopeKind::Type | ScopeKind::Program))
        .unwrap_or(0)
}

fn record_ref(st: &mut State, name: &str, range: std::ops::Range<usize>, ns: Ns) {
    let target = lookup(st, name, ns);
    st.analysis.refs.push(RefSite {
        name: range,
        target,
        expansion: None,
        remote: false,
        module_attr: None,
    });
}

fn record_ref_target(st: &mut State, range: std::ops::Range<usize>, target: Option<usize>) {
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
    st.scopes.push(Scope::new(kind));
}

fn bind_name_node(st: &mut State, node: Node, ns: Ns) {
    bind(
        st,
        text(st.contents, node),
        node.start_byte()..node.end_byte(),
        ns,
    );
}

/// Identifier local: bind-or-reference like Ruby assignment semantics —
/// an unbound name declares a local in the current scope; a bound one is
/// a reference (possibly to an outer scope through blocks).
fn assign_ident(st: &mut State, node: Node) {
    let name = text(st.contents, node);
    let range = node.start_byte()..node.end_byte();
    if let Some(b) = lookup_var(st, name) {
        record_ref_target(st, range, Some(b));
    } else {
        bind(st, name, range, Ns::Var);
    }
}

fn walk(node: Node, st: &mut State) {
    let kind = node.kind();
    match kind {
        // Type declarations: name binds in enclosing scope's constant
        // namespace; body is a type scope.
        "class" | "module" | "singleton_class" => {
            if let Some(name) = node.child_by_field_name("name") {
                bind_name_node(st, name, Ns::Constant);
            }
            for i in 0..node.child_count() {
                let child = node.child(i).unwrap();
                if !child.is_named() || node.field_name_for_child(i) == Some("name") {
                    continue;
                }
                if matches!(child.kind(), "body_statement" | "body") {
                    push(st, ScopeKind::Type);
                    walk(child, st);
                    st.scopes.pop();
                } else {
                    walk(child, st);
                }
            }
            return;
        }
        // `def name(...)`: method name binds at the nearest type/program
        // scope; body is an isolated method scope.
        "method" | "singleton_method" => {
            if let Some(name) = node.child_by_field_name("name") {
                let si = type_scope(st);
                bind_at(st, si, text(st.contents, name), name.start_byte()..name.end_byte(), Ns::Method);
            }
            push(st, ScopeKind::Method);
            walk_children_except_fields(st, node, &["name"]);
            st.scopes.pop();
            return;
        }
        "do_block" | "block" => {
            push(st, ScopeKind::Block);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "lambda" => {
            push(st, ScopeKind::Lambda);
            walk_children(node, st);
            st.scopes.pop();
            return;
        }
        "method_parameters" | "block_parameters" | "lambda_parameters" => {
            bind_param_names(st, node);
            walk_children_except(st, node, &[]);
            return;
        }
        "optional_parameter" | "keyword_parameter" | "rest_parameter"
        | "block_parameter" | "splat_parameter" | "forward_parameter" => {
            if let Some(name) = node.child_by_field_name("name") {
                bind_name_node(st, name, Ns::Var);
            } else if node.child_by_field_name("name").is_none() {
                // some parameter kinds carry a bare identifier child
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.is_named() && child.kind() == "identifier" {
                        bind_name_node(st, child, Ns::Var);
                    }
                }
            }
            walk_children_except(st, node, &["name"]);
            return;
        }
        "multiple_assignment" | "mlhs" | "left_assignment_list" => {
            // `a, b = ...` / `a, (b, c) = ...`: each target binds or
            // references like a plain assignment left side.
            for i in 0..node.child_count() {
                let child = node.child(i).unwrap();
                if !child.is_named() {
                    continue;
                }
                match child.kind() {
                    "identifier" => assign_ident(st, child),
                    "instance_variable" => assign_special(st, child, Ns::Ivar),
                    "class_variable" => assign_special(st, child, Ns::Cvar),
                    "global_variable" => assign_special(st, child, Ns::Gvar),
                    "constant" => {
                        let si = type_scope(st);
                        let name = text(st.contents, child);
                        let range = child.start_byte()..child.end_byte();
                        if let Some(b) = lookup(st, name, Ns::Constant) {
                            record_ref_target(st, range, Some(b));
                        } else {
                            bind_at(st, si, name, range, Ns::Constant);
                        }
                    }
                    _ => walk(child, st),
                }
            }
            return;
        }
        "assignment" => {
            // RHS first: `x = x` reads the pre-assignment binding.
            if let Some(right) = node.child_by_field_name("right") {
                walk(right, st);
            }
            if let Some(left) = node.child_by_field_name("left") {
                match left.kind() {
                    "identifier" => assign_ident(st, left),
                    "instance_variable" => assign_special(st, left, Ns::Ivar),
                    "class_variable" => assign_special(st, left, Ns::Cvar),
                    "global_variable" => assign_special(st, left, Ns::Gvar),
                    "constant" => {
                        // `X = 1` at any level: constants bind at the
                        // nearest type/program scope.
                        let si = type_scope(st);
                        let name = text(st.contents, left);
                        let range = left.start_byte()..left.end_byte();
                        if let Some(b) = lookup(st, name, Ns::Constant) {
                            record_ref_target(st, range, Some(b));
                        } else {
                            bind_at(st, si, name, range, Ns::Constant);
                        }
                    }
                    _ => walk(left, st),
                }
            }
            walk_children_except_fields(st, node, &["left", "right"]);
            return;
        }
        "call" => {
            walk_call(st, node);
            return;
        }
        "identifier" => {
            // Bare identifier: local variable if bound, else a zero-arg
            // method call (or an unresolvable name -> blocker).
            let name = text(st.contents, node);
            let range = node.start_byte()..node.end_byte();
            if let Some(b) = lookup_var(st, name) {
                record_ref_target(st, range, Some(b));
            } else {
                let t = lookup(st, name, Ns::Method);
                record_ref_target(st, range, t);
            }
            return;
        }
        "constant" => {
            record_ref(st, text(st.contents, node), node.start_byte()..node.end_byte(), Ns::Constant);
            return;
        }
        "instance_variable" | "class_variable" | "global_variable" => {
            let ns = match kind {
                "instance_variable" => Ns::Ivar,
                "class_variable" => Ns::Cvar,
                _ => Ns::Gvar,
            };
            record_ref(st, text(st.contents, node), node.start_byte()..node.end_byte(), ns);
            return;
        }
        _ => {}
    }
    walk_children(node, st);
}

/// `@x = v` style targets for non-identifier namespaces: bind at the
/// appropriate scope if unbound, else reference.
fn assign_special(st: &mut State, node: Node, ns: Ns) {
    let name = text(st.contents, node);
    let range = node.start_byte()..node.end_byte();
    if let Some(b) = lookup(st, name, ns) {
        record_ref_target(st, range, Some(b));
    } else {
        match ns {
            Ns::Gvar => bind_at(st, 0, name, range, ns),
            _ => {
                let si = type_scope(st);
                bind_at(st, si, name, range, ns);
            }
        }
    }
}

/// `recv.m(args)` / `m(args)`: the method name resolves only for bare
/// calls and `self.` receivers; other receivers are unknown.
fn walk_call(st: &mut State, node: Node) {
    let method = node.child_by_field_name("method");
    let receiver = node.child_by_field_name("receiver");
    let resolvable = receiver.is_none_or(|r| text(st.contents, r) == "self");
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() {
            continue;
        }
        if Some(child.id()) == method.map(|n| n.id()) {
            if let Some(m) = method {
                let range = m.start_byte()..m.end_byte();
                let name = text(st.contents, m);
                if resolvable {
                    let t = lookup(st, name, Ns::Method);
                    record_ref_target(st, range, t);
                } else {
                    record_unresolved(st, range);
                }
            }
        } else {
            walk(child, st);
        }
    }
}

/// Parameter lists: bind every `name`-field identifier plus bare
/// identifier children (required params have no `name` field wrapper
/// in some grammar versions).
fn bind_param_names(st: &mut State, node: Node) {
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        if !child.is_named() {
            continue;
        }
        match child.kind() {
            "identifier" => bind_name_node(st, child, Ns::Var),
            // parameter wrapper kinds handle their own name fields when
            // walked; but their `name` children are identifiers — bind
            // here too only if the wrapper won't be walked. We walk
            // children normally afterwards, so skip wrappers.
            _ => {}
        }
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

fn walk_children_except(st: &mut State, node: Node, _skip: &[&str]) {
    // Parameter lists: wrappers bind their own names when walked; bare
    // identifier children were bound by bind_param_names.
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        if !child.is_named() {
            continue;
        }
        if child.kind() == "identifier" {
            continue; // already bound
        }
        walk(child, st);
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
