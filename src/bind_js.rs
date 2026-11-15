//! JavaScript/TypeScript binding analysis via `oxc_semantic`: scopes,
//! symbols and resolved references are computed in-process — no LSP, no
//! runtime, no persistent index.

use std::collections::HashMap;
use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast::AstKind;
use oxc_parser::Parser;
use oxc_semantic::{AstNodes, NodeId, SemanticBuilder, SymbolFlags};
use oxc_span::{GetSpan, SourceType};

use crate::bind::{Analysis, BindingSite, Expansion, RefSite};
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
            name: span.start as usize..span.end as usize,
            text: scoping.symbol_name(*symbol_id).to_string(),
            expansion,
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
                name: span.start as usize..span.end as usize,
                target: Some(target),
                expansion: ref_expansion(nodes, reference.node_id()),
                remote: false,
            });
        }
    }
    for (_name, reference_ids) in scoping.root_unresolved_references().iter() {
        for &reference_id in reference_ids.iter() {
            let reference = scoping.get_reference(reference_id);
            let node = nodes.get_node(reference.node_id());
            let span = node.kind().span();
            analysis.refs.push(RefSite {
                name: span.start as usize..span.end as usize,
                target: None,
                expansion: ref_expansion(nodes, reference.node_id()),
                remote: false,
            });
        }
    }

    // Destructuring declarations: `const { helper } = o` makes `helper` a
    // binding whose shorthand couples key and local name.
    let decl_spans: HashMap<(usize, usize), usize> = analysis
        .bindings
        .iter()
        .enumerate()
        .map(|(i, b)| ((b.name.start, b.name.end), i))
        .collect();
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
