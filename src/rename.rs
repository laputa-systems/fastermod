//! --rename: binding-aware local rename. Selects one binding (bare name
//! must be unique across the analyzed files, or --at picks it), rewrites
//! its declaration and resolved references, then re-parses and re-resolves
//! the edited file to prove the rename preserved binding identity. Local
//! rename covers the binding's own file; cross-file references are REFAC-8.

use std::ops::Range;

use crate::bind::{Analysis, Expansion};
use crate::lang::{self, BindingSupport, LangSpec};
use crate::plan::{Anchor, Diagnostic, EditKind, Severity, SkippedFile, SpanEdit};
use crate::source::SourceFile;
use crate::syntax::{self, SyntaxRun};
use crate::text::index_to_row_col;
use crate::Failure;

/// Per-language analysis dispatch. `Ok(None)` means the file did not parse.
fn analyze(spec: &LangSpec, contents: &str, path: &std::path::Path) -> Option<Analysis> {
    match spec.id {
        "javascript" | "typescript" | "tsx" => crate::bind_js::analyze(spec, contents, path),
        "python" => crate::bind_py::analyze(spec, contents, path),
        "rust" => crate::bind_rs::analyze(spec, contents, path),
        "go" => crate::bind_go::analyze(spec, contents, path),
        "lua" => crate::bind_lua::analyze(spec, contents, path),
        "java" => crate::bind_java::analyze(spec, contents, path),
        "csharp" => crate::bind_csharp::analyze(spec, contents, path),
        "kotlin" => crate::bind_kotlin::analyze(spec, contents, path),
        "swift" => crate::bind_swift::analyze(spec, contents, path),
        "dart" => crate::bind_dart::analyze(spec, contents, path),
        "c" => crate::bind_c::analyze(spec, contents, path),
        "cpp" => crate::bind_cpp::analyze(spec, contents, path),
        "bash" => crate::bind_bash::analyze(spec, contents, path),
        "zig" => crate::bind_zig::analyze(spec, contents, path),
        "scala" => crate::bind_scala::analyze(spec, contents, path),
        "elixir" => crate::bind_elixir::analyze(spec, contents, path),
        "haskell" => crate::bind_haskell::analyze(spec, contents, path),
        "erlang" => crate::bind_erlang::analyze(spec, contents, path),
        "julia" => crate::bind_julia::analyze(spec, contents, path),
        "r" => crate::bind_r::analyze(spec, contents, path),
        "groovy" => crate::bind_groovy::analyze(spec, contents, path),
        "vb-dotnet" => crate::bind_vb::analyze(spec, contents, path),
        "ocaml" => crate::bind_ocaml::analyze(spec, contents, path),
        "fsharp" => crate::bind_fsharp::analyze(spec, contents, path),
        "php" => crate::bind_php::analyze(spec, contents, path),
        "ruby" => crate::bind_ruby::analyze(spec, contents, path),
        _ => None,
    }
}

fn line_excerpt(contents: &str, byte_index: usize) -> String {
    let start = contents[..byte_index].rfind('\n').map_or(0, |i| i + 1);
    let end = contents[byte_index..]
        .find('\n')
        .map_or(contents.len(), |i| byte_index + i);
    contents[start..end].to_string()
}

fn anchor_for(contents: &str, byte_start: usize) -> Anchor {
    let (line0, col0) = index_to_row_col(contents, byte_start);
    Anchor {
        line: line0 + 1,
        column: col0 + 1,
        excerpt: line_excerpt(contents, byte_start),
    }
}

/// Parse `--at FILE:LINE:COL` (1-based line, 1-based byte column).
fn parse_anchor(spec: &str) -> std::result::Result<(String, usize, usize), String> {
    let mut parts = spec.rsplitn(3, ':');
    let col = parts.next().and_then(|s| s.parse::<usize>().ok());
    let line = parts.next().and_then(|s| s.parse::<usize>().ok());
    let file = parts.next();
    match (file, line, col) {
        (Some(f), Some(l), Some(c)) if !f.is_empty() && l > 0 && c > 0 => {
            Ok((f.to_string(), l, c))
        }
        _ => Err(format!(
            "invalid --at anchor {spec:?}: expected FILE:LINE:COL (1-based)"
        )),
    }
}

/// Byte offset of a 1-based line / 1-based byte column anchor.
fn anchor_offset(contents: &str, line: usize, col: usize) -> Option<usize> {
    let mut offset = 0usize;
    for (i, l) in contents.split_inclusive('\n').enumerate() {
        if i + 1 == line {
            let line_start = offset;
            let target = line_start + col - 1;
            if target >= offset + l.len() {
                return None;
            }
            // Column must land on a char boundary.
            return contents.is_char_boundary(target).then_some(target);
        }
        offset += l.len();
    }
    None
}

fn expand_replacement(
    expansion: Option<Expansion>,
    old: &str,
    new: &str,
    is_decl: bool,
) -> String {
    match (expansion, is_decl) {
        (Some(Expansion::ShorthandColon), _) => format!("{old}: {new}"),
        (Some(Expansion::ImportAlias), true) => format!("{old} as {new}"),
        (Some(Expansion::ExportAlias), false) => format!("{new} as {old}"),
        // ImportAlias on a reference or ExportAlias on a declaration should
        // not occur; treat as plain replacement rather than guessing.
        _ => new.to_string(),
    }
}

/// Map a byte position through a sorted set of disjoint edits.
fn map_pos(edits: &[SpanEdit], pos: usize) -> usize {
    let mut out = pos as i64;
    for edit in edits {
        if edit.byte_end <= pos {
            out += edit.replacement.len() as i64 - (edit.byte_end - edit.byte_start) as i64;
        } else {
            break;
        }
    }
    out as usize
}

/// Post-edit proof: re-analyze the rewritten file and confirm the renamed
/// binding's references are exactly the edited sites and every other
/// binding kept its reference count (no capture, no unbinding).
/// `target: None` verifies file integrity only (no renamed binding —
/// used for files touched solely by import-specifier or attribute edits).
/// Offset of the NEW identifier inside an expansion's replacement text.
/// `NEW as OLD` puts it first; every other template ends with NEW.
fn new_offset(expansion: Option<Expansion>, replacement: &str, new: &str) -> usize {
    if expansion == Some(Expansion::ExportAlias) {
        0
    } else {
        replacement.len() - new.len()
    }
}

#[allow(clippy::too_many_arguments)]
fn verify_rename(
    spec: &LangSpec,
    contents: &str,
    path: &std::path::Path,
    old_analysis: &Analysis,
    target: Option<usize>,
    edits: &[SpanEdit],
    new_decl: Option<Range<usize>>,
    ref_new_starts: &std::collections::HashSet<usize>,
) -> std::result::Result<(), Failure> {
    let mut after = contents.to_string();
    for edit in edits.iter().rev() {
        after.replace_range(edit.byte_start..edit.byte_end, &edit.replacement);
    }
    let Some(analysis) = analyze(spec, &after, path) else {
        return Err(Failure::analysis(
            "post-edit parse failed; refusing rename",
        ));
    };
    if let (Some(target), Some(new_decl)) = (target, new_decl) {
        // Locate the renamed binding at its mapped declaration site.
        let renamed = analysis
            .bindings
            .iter()
            .position(|b| b.name.start == new_decl.start && b.name.end == new_decl.end)
            .ok_or_else(|| {
                Failure::analysis("post-edit resolution: renamed declaration not found")
            })?;
        let expected_refs = old_analysis.refs_of(target).len();
        let renamed_refs = analysis.refs_of(renamed);
        if renamed_refs.len() != expected_refs {
            return Err(Failure::analysis(format!(
                "post-edit resolution: renamed binding has {} reference(s), expected {expected_refs} (capture or unbinding)",
                renamed_refs.len()
            )));
        }
        // Every reference of the renamed binding must sit on an edited span.
        for r in &renamed_refs {
            if !ref_new_starts.contains(&r.name.start) {
                return Err(Failure::analysis(format!(
                    "post-edit resolution: renamed binding gained an unedited reference at bytes {}..{} (capture)",
                    r.name.start, r.name.end
                )));
            }
        }
    }
    // Every other binding must keep its declaration site and reference
    // count — a changed count means capture or accidental unbinding.
    for (j, old_binding) in old_analysis.bindings.iter().enumerate() {
        if Some(j) == target {
            continue;
        }
        // A dual-role token (e.g. `use m::target`'s bound name, which is
        // also the path-tail ref) may be covered by an expansion that
        // keeps the old spelling inside the replacement (`renamed as
        // target`). Locate the name inside the replacement then.
        let (mapped_start, mapped_end) = edits
            .iter()
            .find(|e| e.byte_start == old_binding.name.start && e.byte_end == old_binding.name.end)
            .and_then(|e| {
                e.replacement
                    .find(&old_binding.text)
                    .map(|off| {
                        let s = map_pos(edits, e.byte_start) + off;
                        (s, s + old_binding.text.len())
                    })
            })
            .unwrap_or_else(|| {
                (
                    map_pos(edits, old_binding.name.start),
                    map_pos(edits, old_binding.name.end),
                )
            });
        let Some(new_j) = analysis
            .bindings
            .iter()
            .position(|b| b.name.start == mapped_start && b.name.end == mapped_end)
        else {
            return Err(Failure::analysis(
                "post-edit resolution: an unrelated declaration moved or vanished",
            ));
        };
        let before = old_analysis.refs_of(j).len();
        let after_count = analysis.refs_of(new_j).len();
        if before != after_count {
            return Err(Failure::analysis(format!(
                "post-edit resolution: binding at {} changed reference count {before} -> {after_count} (capture)",
                old_binding.name.start
            )));
        }
    }
    Ok(())
}

/// Expansion lookup for a token range: bindings first, then refs.
fn expansion_at(
    ctxs: &[crate::workspace::FileCtx<'_>],
    file: usize,
    range: &Range<usize>,
) -> Option<Expansion> {
    let a = ctxs[file].analysis;
    for b in &a.bindings {
        if b.name == *range {
            return b.expansion;
        }
    }
    for r in &a.refs {
        if r.name == *range {
            return r.expansion;
        }
    }
    None
}

/// The defining binding for a same-spelling cluster: canonical roots
/// preferred, module-level non-`mod` roots win.
fn symbol_def(
    ctxs: &[crate::workspace::FileCtx<'_>],
    cluster: &[(usize, usize)],
) -> (usize, usize) {
    let mut defs: Vec<(usize, usize)> = Vec::new();
    for &m in cluster {
        let d = crate::workspace::canonical(ctxs, m.0, m.1, 0).unwrap_or(m);
        if !defs.contains(&d) {
            defs.push(d);
        }
    }
    let roots: Vec<_> = defs
        .iter()
        .copied()
        .filter(|&d| crate::workspace::canonical(ctxs, d.0, d.1, 0).is_none())
        .collect();
    let pool: Vec<_> = if roots.is_empty() { defs.clone() } else { roots };
    pool.iter()
        .copied()
        .find(|&(f, b)| {
            let bb = &ctxs[f].analysis.bindings[b];
            bb.module_level && !bb.is_mod
        })
        .unwrap_or(pool[0])
}

/// Cross-file rename: expand the defining binding's mentions across the
/// module graph, verify each touched file, then prove no `old`-spelled
/// edge or attribute was left unresolved.
fn run_workspace(
    ctxs: &[crate::workspace::FileCtx<'_>],
    def: (usize, usize),
    old: &str,
    new: &str,
    run: &mut SyntaxRun,
) -> std::result::Result<(), Failure> {
    let (df, db) = def;
    let def_binding = &ctxs[df].analysis.bindings[db];
    if def_binding.is_mod {
        return Err(Failure::analysis(format!(
            "cannot rename module {:?}: renaming a `mod` requires renaming its file, which is not supported",
            old
        )));
    }
    let (mentions, blockers) = crate::workspace::collect_mentions(ctxs, df, db);
    if !blockers.is_empty() {
        let mut msg = format!(
            "cannot rename {old:?}: {} unresolved module reference(s) may refer to it",
            blockers.len()
        );
        for (fi, range) in blockers.iter().take(10) {
            let c = &ctxs[*fi].file.contents;
            let anchor = anchor_for(c, range.start);
            msg.push_str(&format!(
                "\n  {}:{}:{}",
                ctxs[*fi].file.path.display(),
                anchor.line,
                anchor.column
            ));
        }
        return Err(Failure::analysis(msg));
    }
    // Any inexact analyzer may hide `old` occurrences that refer to the
    // symbol (star imports, macros, dynamic access) — block on all of
    // them across every analyzed file, not just the defining file.
    for c in ctxs.iter() {
        if c.analysis.unresolved_is_exact {
            continue;
        }
        // A token already covered by an import edge is resolved (or
        // blocked) by `collect_mentions` — don't double-count it here.
        let edge_ranges: std::collections::HashSet<&Range<usize>> = c
            .analysis
            .imports
            .iter()
            .map(|e| &e.name_range)
            .collect();
        let unresolved: Vec<_> = c
            .analysis
            .unresolved_named(&c.file.contents, old)
            .into_iter()
            .filter(|r| !edge_ranges.contains(r))
            .collect();
        if unresolved.is_empty() {
            continue;
        }
        let mut msg = format!(
            "cannot rename {old:?}: {} unresolved reference(s) in {} may refer to it",
            unresolved.len(),
            c.file.path.display()
        );
        for r in unresolved.iter().take(10) {
            let anchor = anchor_for(&c.file.contents, r.start);
            msg.push_str(&format!("\n  {}:{}", anchor.line, anchor.column));
        }
        return Err(Failure::analysis(msg));
    }

    // Group mentions per file; expand unaliased import locals to their
    // references (the local binding takes the new name too).
    let mut by_file: std::collections::BTreeMap<usize, Vec<crate::workspace::Mention>> =
        std::collections::BTreeMap::new();
    for m in mentions {
        by_file.entry(m.file).or_default().push(m);
    }
    for (fi, ms) in by_file.iter_mut() {
        let mut extra: Vec<crate::workspace::Mention> = Vec::new();
        for m in ms.iter() {
            if let Some(b) = m.local_binding {
                let bb = &ctxs[*fi].analysis.bindings[b];
                // Unaliased: the mention range IS the binding's name
                // token, so the local binding (and its refs) renames.
                if bb.name == m.range {
                    for r in ctxs[*fi].analysis.refs_of(b) {
                        extra.push(crate::workspace::Mention {
                            file: *fi,
                            range: r.name.clone(),
                            local_binding: None,
                        });
                    }
                }
            }
        }
        ms.extend(extra);
        // Dedupe by range — a `use` tail token can be both a path ref
        // and the edge's name token.
        let mut seen = std::collections::HashSet::new();
        ms.retain(|m| seen.insert(m.range.clone()));
    }

    // Build per-file edits with expansion lookup, then verify.
    let mut touched_files = 0usize;
    let mut file_edits: std::collections::BTreeMap<usize, Vec<SpanEdit>> =
        std::collections::BTreeMap::new();
    for (fi, ms) in &by_file {
        let ctx = &ctxs[*fi];
        let file = ctx.file;
        let mut sites: Vec<(SpanEdit, Option<Expansion>)> = Vec::new();
        for m in ms {
            // A token coinciding with an import edge's name range denotes
            // the *remote* spelling: both sides of an unaliased edge
            // rename together, so it becomes plain NEW. Every other
            // position keeps its coupled-syntax expansion (`export {
            // foo }` -> `qux as foo`, `{ foo }` -> `foo: qux`).
            let is_edge = ctx
                .analysis
                .imports
                .iter()
                .any(|e| e.name_range == m.range);
            let (rep, exp) = if is_edge {
                (new.to_string(), None)
            } else {
                let is_decl = ctx.analysis.bindings.iter().any(|b| b.name == m.range);
                let exp = expansion_at(ctxs, *fi, &m.range);
                (expand_replacement(exp, old, new, is_decl), exp)
            };
            sites.push((
                SpanEdit {
                    byte_start: m.range.start,
                    byte_end: m.range.end,
                    replacement: rep,
                    kind: EditKind::RenameReference,
                    anchor: anchor_for(&file.contents, m.range.start),
                },
                exp,
            ));
        }
        sites.sort_by_key(|(e, _)| e.byte_start);
        let edits: Vec<SpanEdit> = sites.iter().map(|(e, _)| e.clone()).collect();

        // Renamed bindings in this file: the def (when present) plus
        // every unaliased import local.
        let mut renamed: Vec<usize> = Vec::new();
        if *fi == df {
            renamed.push(db);
        }
        for m in ms {
            if let Some(b) = m.local_binding {
                let bb = &ctx.analysis.bindings[b];
                if bb.name == m.range && !renamed.contains(&b) {
                    renamed.push(b);
                }
            }
        }

        // Post-edit verification per renamed binding.
        for &b in &renamed {
            let bb = &ctx.analysis.bindings[b];
            let decl_edit = edits
                .iter()
                .position(|e| e.byte_start == bb.name.start && e.byte_end == bb.name.end)
                .ok_or_else(|| {
                    Failure::analysis("internal: renamed binding decl edit missing")
                })?;
            let (decl_exp, _) = {
                let e = &sites[decl_edit];
                (e.1, &e.0)
            };
            let decl_off = new_offset(decl_exp, &sites[decl_edit].0.replacement, new);
            let new_start = map_pos(&edits, bb.name.start) + decl_off;
            let new_decl = new_start..new_start + new.len();
            let mut ref_starts = std::collections::HashSet::new();
            for r in ctx.analysis.refs_of(b) {
                if let Some(pos) = edits.iter().position(|e| {
                    e.byte_start == r.name.start && e.byte_end == r.name.end
                }) {
                    let (rexp, _) = {
                        let e = &sites[pos];
                        (e.1, &e.0)
                    };
                    let off = new_offset(rexp, &sites[pos].0.replacement, new);
                    ref_starts.insert(map_pos(&edits, r.name.start) + off);
                }
            }
            verify_rename(
                ctx.spec,
                &file.contents,
                &file.path,
                ctx.analysis,
                Some(b),
                &edits,
                Some(new_decl),
                &ref_starts,
            )?;
        }
        if renamed.is_empty() {
            verify_rename(
                ctx.spec,
                &file.contents,
                &file.path,
                ctx.analysis,
                None,
                &edits,
                None,
                &std::collections::HashSet::new(),
            )?;
        }
        touched_files += 1;
        file_edits.insert(*fi, edits.clone());
        if let Some(planned) = syntax::finish_file(file, edits) {
            run.files.push(planned);
        }
    }

    // Cross-file proof: re-analyze touched files and confirm (a) the
    // defining module still resolves `new` at the renamed declaration
    // and (b) no `old`-spelled edge, attribute, or inexact unresolved
    // occurrence remains that could still refer to the old symbol.
    let mut new_analyses: Vec<Analysis> = ctxs.iter().map(|c| c.analysis.clone()).collect();
    let mut new_files: Vec<SourceFile> = ctxs.iter().map(|c| c.file.clone()).collect();
    for (fi, edits) in &file_edits {
        let ctx = &ctxs[*fi];
        let file = ctx.file;
        let mut after = file.contents.clone();
        for e in edits.iter().rev() {
            after.replace_range(e.byte_start..e.byte_end, &e.replacement);
        }
        let Some(analysis) = analyze(ctx.spec, &after, &file.path) else {
            return Err(Failure::analysis(
                "post-edit parse failed; refusing workspace rename",
            ));
        };
        new_analyses[*fi] = analysis;
        new_files[*fi].contents = after;
    }
    let ctxs2: Vec<crate::workspace::FileCtx<'_>> = ctxs
        .iter()
        .enumerate()
        .map(|(i, c)| crate::workspace::FileCtx {
            file: &new_files[i],
            spec: c.spec,
            analysis: &new_analyses[i],
        })
        .collect();
    // The renamed symbol must still resolve under its new spelling.
    match crate::workspace::resolve_name(&ctxs2, df, new, 0) {
        Some((f, b)) if f == df && new_analyses[df].bindings[b].text == new => {}
        _ => {
            return Err(Failure::analysis(
                "post-edit resolution: renamed symbol no longer resolves in its module",
            ));
        }
    }
    // Leftover check: any `old`-spelled internal edge or module
    // attribute that now fails to resolve is a missed reference.
    for (fi, c) in ctxs2.iter().enumerate() {
        for edge in &c.analysis.imports {
            if edge.star || edge.name != old {
                continue;
            }
            if crate::workspace::resolve_edge(&ctxs2, fi, edge, 0) == crate::workspace::MemberTarget::Unresolved {
                return Err(Failure::analysis(format!(
                    "post-edit resolution: leftover unresolved import of {old:?} in {}",
                    c.file.path.display()
                )));
            }
        }
        for r in &c.analysis.refs {
            let Some(ei) = r.module_attr else {
                continue;
            };
            if &c.file.contents[r.name.clone()] != old {
                continue;
            }
            let edge = &c.analysis.imports[ei];
            if crate::workspace::resolve_edge(&ctxs2, fi, edge, 0) == crate::workspace::MemberTarget::Unresolved {
                return Err(Failure::analysis(format!(
                    "post-edit resolution: leftover unresolved attribute {old:?} in {}",
                    c.file.path.display()
                )));
            }
        }
        if !c.analysis.unresolved_is_exact {
            let leftover = c.analysis.unresolved_named(&c.file.contents, old);
            if !leftover.is_empty() {
                return Err(Failure::analysis(format!(
                    "post-edit resolution: {} unresolved {old:?} occurrence(s) remain in {}",
                    leftover.len(),
                    c.file.path.display()
                )));
            }
        }
    }
    run.diagnostics.push(Diagnostic {
        severity: Severity::Info,
        code: "workspace-rename".to_string(),
        message: format!(
            "workspace rename of {old:?} -> {new:?} across {} analyzed file(s); files outside the discovery roots are not analyzed",
            touched_files
        ),
        path: None,
    });
    Ok(())
}

/// Run --rename OLD NEW over the discovered files.
pub(crate) fn run_rename(
    old: &str,
    new: &str,
    files: &[SourceFile],
    at_anchor: Option<&str>,
    lang_overrides: &[String],
) -> std::result::Result<SyntaxRun, Failure> {
    if old.is_empty() {
        return Err(Failure::usage("--rename requires a non-empty identifier"));
    }
    if new.is_empty() || new.chars().any(char::is_whitespace) {
        return Err(Failure::usage(
            "--rename replacement must be a single identifier token",
        ));
    }
    let mut run = SyntaxRun {
        files: Vec::new(),
        skipped: Vec::new(),
        diagnostics: Vec::new(),
    };

    // Analyze every candidate file once.
    struct Analyzed {
        file_index: usize,
        spec: &'static LangSpec,
        analysis: Analysis,
    }
    let mut analyzed: Vec<Analyzed> = Vec::new();
    for (i, file) in files.iter().enumerate() {
        let Some(spec) = lang::detect(&file.path, lang_overrides) else {
            run.skipped.push(SkippedFile {
                path: file.path.clone(),
                reason: "no registered language matches this file".to_string(),
            });
            continue;
        };
        if spec.binding == BindingSupport::None {
            run.diagnostics.push(Diagnostic {
                severity: Severity::Info,
                code: "unsupported-operation".to_string(),
                message: format!("--rename is not supported for {}", spec.id),
                path: Some(file.path.clone()),
            });
            continue;
        }
        match analyze(spec, &file.contents, &file.path) {
            Some(analysis) => analyzed.push(Analyzed {
                file_index: i,
                spec,
                analysis,
            }),
            None => run.skipped.push(SkippedFile {
                path: file.path.clone(),
                reason: format!("{} parse contains syntax errors", spec.id),
            }),
        }
    }

    // Cross-file view over the analyzed files (indices match `analyzed`).
    let ctxs: Vec<crate::workspace::FileCtx<'_>> = analyzed
        .iter()
        .map(|a| crate::workspace::FileCtx {
            file: &files[a.file_index],
            spec: a.spec,
            analysis: &a.analysis,
        })
        .collect();

    // Select the target symbol. Same-spelling bindings are grouped into
    // clusters by import-edge canonicalization: an importer's `foo` and
    // the module's `foo` are one symbol, unrelated `foo`s are not.
    let selected = if let Some(anchor) = at_anchor {
        let (anchor_path, line, col) = parse_anchor(anchor).map_err(Failure::usage)?;
        let norm = |p: &std::path::Path| {
            p.to_string_lossy().trim_start_matches("./").to_string()
        };
        let want = anchor_path.trim_start_matches("./");
        let file_matches: Vec<usize> = analyzed
            .iter()
            .enumerate()
            .filter(|(_, a)| {
                let p = norm(&files[a.file_index].path);
                p == want || p.ends_with(&format!("/{want}"))
            })
            .map(|(i, _)| i)
            .collect();
        if file_matches.len() != 1 {
            return Err(Failure::usage(format!(
                "--at path {anchor_path:?} matches {} analyzed file(s)",
                file_matches.len()
            )));
        }
        let ai = file_matches[0];
        let contents = &files[analyzed[ai].file_index].contents;
        let Some(offset) = anchor_offset(contents, line, col) else {
            return Err(Failure::usage(format!(
                "--at {anchor:?} is outside the file or not a char boundary"
            )));
        };
        // The anchor may point at the declaration or any resolved reference
        // of the binding; both identify the same symbol.
        let a = &analyzed[ai];
        let mut found = None;
        for (bi, b) in a.analysis.bindings.iter().enumerate() {
            if b.name.contains(&offset) {
                found = Some(bi);
                break;
            }
        }
        if found.is_none() {
            for r in &a.analysis.refs {
                if r.name.contains(&offset)
                    && let Some(t) = r.target
                {
                    found = Some(t);
                    break;
                }
            }
        }
        let Some(bi) = found else {
            return Err(Failure::usage(format!(
                "--at {anchor:?} does not select an identifier"
            )));
        };
        let binding = &a.analysis.bindings[bi];
        if binding.text != old {
            return Err(Failure::usage(format!(
                "--at selects binding {:?}, not {old:?}",
                binding.text
            )));
        }
        crate::workspace::canonical(&ctxs, ai, bi, 0).unwrap_or((ai, bi))
    } else {
        let clusters = crate::workspace::symbol_clusters(&ctxs, old);
        if clusters.is_empty() {
            // Distinguish "no such binding" from "unresolved occurrences".
            let mut unresolved = Vec::new();
            for a in &analyzed {
                unresolved.extend(
                    a.analysis
                        .unresolved_named(&files[a.file_index].contents, old)
                        .into_iter()
                        .map(|r| (a.file_index, r)),
                );
            }
            if unresolved.is_empty() {
                return Err(Failure::analysis(format!(
                    "no binding named {old:?} in the analyzed files"
                )));
            }
            let mut msg = format!(
                "cannot rename {old:?}: {} unresolved reference(s) may refer to it",
                unresolved.len()
            );
            for (fi, r) in unresolved.iter().take(10) {
                let c = &files[*fi].contents;
                let anchor = anchor_for(c, r.start);
                msg.push_str(&format!(
                    "\n  {}:{}:{}", files[*fi].path.display(), anchor.line, anchor.column
                ));
            }
            return Err(Failure::analysis(msg));
        }
        let mut unique: Vec<(usize, usize)> = Vec::new();
        for c in &clusters {
            let d = symbol_def(&ctxs, c);
            if !unique.contains(&d) {
                unique.push(d);
            }
        }
        match unique.len() {
            1 => unique[0],
            _ => {
                let mut msg = format!(
                    "{old:?} is bound {} times; use --at to select one:\n",
                    unique.len()
                );
                for (fi, bi) in unique.iter().take(20) {
                    let a = &analyzed[*fi];
                    let b = &a.analysis.bindings[*bi];
                    let c = &files[a.file_index].contents;
                    let anchor = anchor_for(c, b.name.start);
                    msg.push_str(&format!(
                        "  {}:{}:{} ({})\n",
                        files[a.file_index].path.display(),
                        anchor.line,
                        anchor.column,
                        b.text
                    ));
                }
                return Err(Failure::analysis(msg));
            }
        }
    };

    // `mod` declarations name files, not in-file symbols: renaming one
    // requires a file rename, which is out of scope — refuse everywhere.
    if analyzed[selected.0].analysis.bindings[selected.1].is_mod {
        return Err(Failure::analysis(format!(
            "cannot rename module {:?}: renaming a `mod` requires renaming its file, which is not supported",
            old
        )));
    }

    // A module-level non-`mod` definition means the symbol is reachable
    // through the module graph: run the workspace path. Everything else
    // keeps the REFAC-7 single-file behavior.
    let def_binding = &analyzed[selected.0].analysis.bindings[selected.1];
    if def_binding.module_level && !def_binding.is_mod {
        run_workspace(&ctxs, selected, old, new, &mut run)?;
        return Ok(run);
    }

    // Build edits for the selected binding only, tracking where NEW lands
    // inside each replacement (expansions embed it at a nonzero offset).
    let mut sites: Vec<(SpanEdit, usize)> = Vec::new();
    let target = &analyzed[selected.0];
    let file = &files[target.file_index];
    let binding_idx = selected.1;
    let binding = &target.analysis.bindings[binding_idx];
    let decl_rep = expand_replacement(binding.expansion, old, new, true);
    let decl_off = new_offset(binding.expansion, &decl_rep, new);
    sites.push((
        SpanEdit {
            byte_start: binding.name.start,
            byte_end: binding.name.end,
            replacement: decl_rep,
            kind: EditKind::RenameReference,
            anchor: anchor_for(&file.contents, binding.name.start),
        },
        decl_off,
    ));
    for r in target.analysis.refs_of(binding_idx) {
        let rep = expand_replacement(r.expansion, old, new, false);
        let off = new_offset(r.expansion, &rep, new);
        sites.push((
            SpanEdit {
                byte_start: r.name.start,
                byte_end: r.name.end,
                replacement: rep,
                kind: EditKind::RenameReference,
                anchor: anchor_for(&file.contents, r.name.start),
            },
            off,
        ));
    }
    sites.sort_by_key(|(e, _)| e.byte_start);
    let edits: Vec<SpanEdit> = sites.iter().map(|(e, _)| e.clone()).collect();
    let mut ref_new_starts = std::collections::HashSet::new();
    let mut new_decl = None;
    for (edit, off) in &sites {
        let mapped = map_pos(&edits, edit.byte_start) + off;
        if edit.byte_start == binding.name.start && edit.byte_end == binding.name.end {
            new_decl = Some(mapped..mapped + new.len());
        } else {
            ref_new_starts.insert(mapped);
        }
    }
    let new_decl = new_decl.ok_or_else(|| Failure::analysis("internal: decl edit missing"))?;

    // Unresolved occurrences of OLD inside the target file may refer to the
    // selected binding — but only for analyzers whose unresolved set is not
    // provably complete (macros, dynamic constructs). For exact resolvers
    // an unresolved occurrence is a different symbol by definition.
    let unresolved = if target.analysis.unresolved_is_exact {
        Vec::new()
    } else {
        target.analysis.unresolved_named(&file.contents, old)
    };
    if !unresolved.is_empty() {
        let mut msg = format!(
            "cannot rename {old:?}: {} unresolved reference(s) in {} may refer to it",
            unresolved.len(),
            file.path.display()
        );
        for r in unresolved.iter().take(10) {
            let anchor = anchor_for(&file.contents, r.start);
            msg.push_str(&format!("\n  {}:{}", anchor.line, anchor.column));
        }
        return Err(Failure::analysis(msg));
    }

    // Post-edit verification: re-parse and re-resolve before writing.
    verify_rename(
        target.spec,
        &file.contents,
        &file.path,
        &target.analysis,
        Some(binding_idx),
        &edits,
        Some(new_decl),
        &ref_new_starts,
    )?;

    if let Some(planned) = syntax::finish_file(file, edits) {
        run.files.push(planned);
    }
    Ok(run)
}
