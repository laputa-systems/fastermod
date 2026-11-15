//! JavaScript/TypeScript binding analysis via `oxc_semantic`: scopes,
//! symbols and resolved references are computed in-process — no LSP, no
//! runtime, no persistent index. Also records import/export edges and
//! export aliases so the workspace layer can resolve cross-file symbols.

use std::collections::HashMap;
use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    ExportDefaultDeclarationKind, Expression, ImportDeclarationSpecifier, ModuleExportName,
    Statement,
};
use oxc_ast::AstKind;
use oxc_parser::Parser;
use oxc_semantic::{AstNodes, NodeId, SemanticBuilder, SymbolFlags};
use oxc_span::{GetSpan, SourceType};

use crate::bind::{Analysis, BindingSite, ExportName, Expansion, ImportEdge, RefSite};
use crate::lang::LangSpec;

fn source_type(spec: &LangSpec, path: &Path) -> SourceType {
    // Prefer the file's real extension; fall back to a permissive default
    // for --lang overrides on foreign extensions. JSX is enabled for the
    // javascript spec because tree-sitter-javascript accepts JSX in .js
    // and oxc's Standard variant would reject it.
    if let Ok(st) = SourceType::from_path(path) {
        return match spec.id {
            "javascript" => st.with_jsx(true),
            _ => st,
        };
    }
    match spec.id {
        "typescript" => SourceType::ts(),
        "tsx" => SourceType::tsx(),
        _ => SourceType::jsx(),
    }
}

fn span_range(span: oxc_span::Span) -> std::ops::Range<usize> {
    span.start as usize..span.end as usize
}

fn export_name_text(name: &ModuleExportName<'_>) -> String {
    match name {
        ModuleExportName::IdentifierName(id) => id.name.to_string(),
        ModuleExportName::IdentifierReference(id) => id.name.to_string(),
        ModuleExportName::StringLiteral(s) => s.value.as_str().unwrap_or_default().to_string(),
    }
}

/// Build the binding analysis for one JS-family file. Returns `Ok(None)`
/// when the source does not parse cleanly — the caller records a skip.
pub(crate) fn analyze(spec: &LangSpec, contents: &str, path: &Path) -> Option<Analysis> {
    let allocator = Allocator::default();
    let ret = Parser::new(&allocator, contents, source_type(spec, path)).parse();
    if ret.fatal_error || !ret.diagnostics.is_empty() {
        return None;
    }
    let semantic = SemanticBuilder::new()
        .with_build_nodes(true)
        .build(&ret.program)
        .semantic;
    let scoping = semantic.scoping();
    let nodes = semantic.nodes();

    let mut analysis = Analysis::default();
    let symbol_ids: Vec<_> = scoping.symbol_ids().collect();
    let mut symbol_index = HashMap::new();
    for (i, id) in symbol_ids.iter().enumerate() {
        symbol_index.insert(*id, i);
    }
    let root_scope = scoping.root_scope_id();

    // Bindings: one per symbol. Symbol spans are the declaring identifier.
    for symbol_id in &symbol_ids {
        let span = scoping.symbol_span(*symbol_id);
        let flags = scoping.symbol_flags(*symbol_id);
        let mut expansion = None;
        if flags.contains(SymbolFlags::Import) {
            // `import { helper }` without `as` couples the local name to the
            // imported name; `import { helper as h }` does not.
            let decl_node = nodes.get_node(scoping.symbol_declaration(*symbol_id));
            for ancestor in
                std::iter::once(decl_node).chain(nodes.ancestors(decl_node.id()))
            {
                if let AstKind::ImportSpecifier(specifier) = ancestor.kind() {
                    if specifier.imported.span() == specifier.local.span() {
                        expansion = Some(Expansion::ImportAlias);
                    }
                    break;
                }
            }
        }
        analysis.bindings.push(BindingSite {
            name: span_range(span),
            text: scoping.symbol_name(*symbol_id).to_string(),
            expansion,
            module_level: scoping.symbol_scope_id(*symbol_id) == root_scope,
            is_mod: false,
        });
    }

    // span -> binding index, for resolving local names in export
    // specifiers and import specifier `local` tokens.
    let decl_spans: HashMap<(usize, usize), usize> = analysis
        .bindings
        .iter()
        .enumerate()
        .map(|(i, b)| ((b.name.start, b.name.end), i))
        .collect();
    let binding_at = |span: oxc_span::Span| -> Option<usize> {
        decl_spans.get(&(span.start as usize, span.end as usize)).copied()
    };

    // Import/export edges and local export aliases.
    let mut module_bindings: HashMap<usize, usize> = HashMap::new();
    for stmt in &ret.program.body {
        match stmt {
            Statement::ImportDeclaration(decl) => {
                let specifier_src = decl.source.value.as_str().unwrap_or_default().to_string();
                if let Some(specs) = &decl.specifiers {
                    for spec in specs.iter() {
                        match spec {
                            ImportDeclarationSpecifier::ImportSpecifier(s) => {
                                analysis.imports.push(ImportEdge {
                                    specifier: specifier_src.clone(),
                                    name: export_name_text(&s.imported),
                                    name_range: span_range(s.imported.span()),
                                    local: binding_at(s.local.span),
                                    reexport: false,
                                    star: false,
                                    exported: None,
                                });
                            }
                            ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                                let local = binding_at(s.local.span);
                                analysis.imports.push(ImportEdge {
                                    specifier: specifier_src.clone(),
                                    name: "default".to_string(),
                                    name_range: span_range(s.local.span),
                                    local,
                                    reexport: false,
                                    star: false,
                                    exported: None,
                                });
                            }
                            ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                                let local = binding_at(s.local.span);
                                let ei = analysis.imports.len();
                                if let Some(l) = local {
                                    module_bindings.insert(l, ei);
                                }
                                analysis.imports.push(ImportEdge {
                                    specifier: specifier_src.clone(),
                                    name: String::new(),
                                    name_range: span_range(s.local.span),
                                    local,
                                    reexport: false,
                                    star: false,
                                    exported: None,
                                });
                            }
                        }
                    }
                }
            }
            Statement::ExportNamedDeclaration(decl) => {
                // `export { a as b }` — local export aliases (no source).
                for spec in &decl.specifiers {
                    let local_binding = match &spec.local {
                        ModuleExportName::IdentifierReference(id) => id
                            .reference_id
                            .get()
                            .and_then(|rid| {
                                let r = scoping.get_reference(rid);
                                r.symbol_id().map(|sid| symbol_index[&sid])
                            })
                            .or_else(|| binding_at(spec.local.span())),
                        _ => binding_at(spec.local.span()),
                    };
                    // oxc does not always link export-specifier locals as
                    // references; fall back to the unique module-level
                    // binding with the same spelling (export specifiers
                    // resolve in module scope only).
                    let local_binding = local_binding.or_else(|| {
                        let lname = export_name_text(&spec.local);
                        let mut hits = analysis
                            .bindings
                            .iter()
                            .enumerate()
                            .filter(|(_, b)| b.module_level && b.text == lname);
                        match (hits.next(), hits.next()) {
                            (Some((i, _)), None) => Some(i),
                            _ => None,
                        }
                    });
                    let exported = export_name_text(&spec.exported);
                    analysis.exports.push(ExportName {
                        exported,
                        local: local_binding,
                    });
                }
            }
            Statement::ExportFromDeclaration(decl) => {
                let specifier_src = decl.source.value.as_str().unwrap_or_default().to_string();
                for spec in &decl.specifiers {
                    let exported = export_name_text(&spec.exported);
                    let imported = export_name_text(&spec.local);
                    let same = exported == imported;
                    analysis.imports.push(ImportEdge {
                        specifier: specifier_src.clone(),
                        name: imported,
                        name_range: span_range(spec.local.span()),
                        local: None,
                        reexport: true,
                        star: false,
                        exported: (!same).then_some(exported),
                    });
                }
            }
            Statement::ExportAllDeclaration(decl) => {
                let specifier_src = decl.source.value.as_str().unwrap_or_default().to_string();
                match &decl.exported {
                    Some(name) => {
                        // `export * as ns from 'm'` — namespace re-export.
                        analysis.imports.push(ImportEdge {
                            specifier: specifier_src,
                            name: String::new(),
                            name_range: span_range(name.span()),
                            local: None,
                            reexport: true,
                            star: false,
                            exported: Some(export_name_text(name)),
                        });
                    }
                    None => {
                        analysis.imports.push(ImportEdge {
                            specifier: specifier_src,
                            name: String::new(),
                            name_range: span_range(decl.span),
                            local: None,
                            reexport: true,
                            star: true,
                            exported: None,
                        });
                    }
                }
            }
            Statement::ExportDefaultDeclaration(decl) => {
                let local = match &decl.declaration {
                    ExportDefaultDeclarationKind::FunctionDeclaration(f) => f
                        .id
                        .as_ref()
                        .and_then(|id| binding_at(id.span)),
                    ExportDefaultDeclarationKind::ClassDeclaration(c) => c
                        .id
                        .as_ref()
                        .and_then(|id| binding_at(id.span)),
                    _ => decl
                        .declaration
                        .as_expression()
                        .and_then(|e| match e {
                            Expression::Identifier(id) => id
                                .reference_id
                                .get()
                                .and_then(|rid| {
                                    let r = scoping.get_reference(rid);
                                    r.symbol_id().map(|sid| symbol_index[&sid])
                                })
                                .or_else(|| binding_at(id.span)),
                            _ => None,
                        }),
                };
                analysis.exports.push(ExportName {
                    exported: "default".to_string(),
                    local,
                });
            }
            _ => {}
        }
    }

    // Dynamic module bindings: `const m = await import('x')` or
    // `const m = require('x')` make `m` a module object, so `m.foo` is a
            // module-attribute reference. Literal specifiers resolve like
            // static imports; non-literal ones get a sentinel specifier
            // that always resolves Unresolved (may name any analyzed file).
    for node in nodes.iter() {
        let AstKind::VariableDeclarator(decl) = node.kind() else {
            continue;
        };
        let Some(init) = &decl.init else {
            continue;
        };
        let mut init = init;
        loop {
            match init {
                Expression::AwaitExpression(a) => init = &a.argument,
                Expression::ParenthesizedExpression(p) => init = &p.expression,
                // Script-mode `await import('x')` parses `await` as an
                // identifier callee wrapping the awaited expression.
                Expression::CallExpression(call)
                    if matches!(
                        &call.callee,
                        Expression::Identifier(id) if id.name == "await"
                    ) =>
                {
                    match call.arguments.first().and_then(|a| a.as_expression()) {
                        Some(e) => init = e,
                        None => break,
                    }
                }
                _ => break,
            }
        }
        let specifier = match init {
            Expression::ImportExpression(imp) => match &imp.source {
                Expression::StringLiteral(s) => {
                    s.value.as_str().unwrap_or_default().to_string()
                }
                _ => "\u{0}dynamic".to_string(),
            },
            Expression::CallExpression(call) => {
                let is_require = matches!(
                    &call.callee,
                    Expression::Identifier(id) if id.name == "require"
                );
                if !is_require {
                    continue;
                }
                match call.arguments.first() {
                    Some(oxc_ast::ast::Argument::StringLiteral(s)) => {
                        s.value.as_str().unwrap_or_default().to_string()
                    }
                    _ => "\u{0}dynamic".to_string(),
                }
            }
            _ => continue,
        };
        let oxc_ast::ast::BindingPattern::BindingIdentifier(id) = &decl.id else {
            continue;
        };
        let Some(local) = binding_at(id.span) else {
            continue;
        };
        let ei = analysis.imports.len();
        module_bindings.insert(local, ei);
        analysis.imports.push(ImportEdge {
            specifier,
            name: String::new(),
            name_range: span_range(id.span),
            local: Some(local),
            reexport: false,
            star: false,
            exported: None,
        });
    }

    // References: resolved ones point at their binding; unresolved ones are
    // collected from the root unresolved map (oxc bubbles them there).
    for symbol_id in &symbol_ids {
        let target = symbol_index[symbol_id];
        for reference_id in scoping.get_resolved_reference_ids(*symbol_id) {
            let reference = scoping.get_reference(*reference_id);
            let node = nodes.get_node(reference.node_id());
            let span = node.kind().span();
            analysis.refs.push(RefSite {
                name: span_range(span),
                target: Some(target),
                expansion: ref_expansion(nodes, reference.node_id()),
                remote: false,
                module_attr: None,
            });
        }
    }
    for (_name, reference_ids) in scoping.root_unresolved_references().iter() {
        for &reference_id in reference_ids.iter() {
            let reference = scoping.get_reference(reference_id);
            let node = nodes.get_node(reference.node_id());
            let span = node.kind().span();
            analysis.refs.push(RefSite {
                name: span_range(span),
                target: None,
                expansion: ref_expansion(nodes, reference.node_id()),
                remote: false,
                module_attr: None,
            });
        }
    }

    // Namespace member references: `ns.foo` where `ns` is an
    // `import * as ns` / `export * as ns` binding. The property token is
    // not an oxc reference; tag it with the module edge so the workspace
    // layer can resolve `foo` inside the target module.
    for node in nodes.iter() {
        let AstKind::StaticMemberExpression(member) = node.kind() else {
            continue;
        };
        let Expression::Identifier(obj) = &member.object else {
            continue;
        };
        let Some(rid) = obj.reference_id.get() else {
            continue;
        };
        let reference = scoping.get_reference(rid);
        let Some(sid) = reference.symbol_id() else {
            continue;
        };
        let Some(&binding_idx) = symbol_index.get(&sid) else {
            continue;
        };
        let Some(&ei) = module_bindings.get(&binding_idx) else {
            continue;
        };
        analysis.refs.push(RefSite {
            name: span_range(member.property.span),
            target: None,
            expansion: None,
            remote: false,
            module_attr: Some(ei),
        });
    }

    // Destructuring declarations: `const { helper } = o` makes `helper` a
    // binding whose shorthand couples key and local name.
    for symbol_id in &symbol_ids {
        let span = scoping.symbol_span(*symbol_id);
        let Some(&index) = decl_spans.get(&(span.start as usize, span.end as usize)) else {
            continue;
        };
        if analysis.bindings[index].expansion.is_some() {
            continue;
        }
        let decl_node = nodes.get_node(scoping.symbol_declaration(*symbol_id));
        for ancestor in
            std::iter::once(decl_node).chain(nodes.ancestors(decl_node.id()))
        {
            if let AstKind::BindingProperty(prop) = ancestor.kind()
                && prop.shorthand
            {
                analysis.bindings[index].expansion = Some(Expansion::ShorthandColon);
                break;
            }
        }
    }

    analysis.unresolved_is_exact = true;
    Some(analysis)
}

/// Classify a reference's coupled position: object shorthand values expand
/// to `OLD: NEW`; shorthand export specifiers expand to `NEW as OLD`.
fn ref_expansion(nodes: &AstNodes<'_>, node_id: NodeId) -> Option<Expansion> {
    for ancestor in nodes.ancestors(node_id) {
        match ancestor.kind() {
            AstKind::ObjectProperty(prop) if prop.shorthand => {
                return Some(Expansion::ShorthandColon);
            }
            AstKind::ExportSpecifier(specifier) => {
                // `export { helper }` — local and exported share one span.
                if specifier.local.span() == specifier.exported.span() {
                    return Some(Expansion::ExportAlias);
                }
                return None;
            }
            _ => {}
        }
    }
    None
}
