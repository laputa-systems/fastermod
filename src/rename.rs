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
    target: usize,
    edits: &[SpanEdit],
    new_decl: Range<usize>,
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
    // Every other binding must keep its declaration site and reference
    // count — a changed count means capture or accidental unbinding.
    for (j, old_binding) in old_analysis.bindings.iter().enumerate() {
        if j == target {
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

    // Select the target binding.
    let mut candidates: Vec<(usize, usize)> = Vec::new(); // (analyzed idx, binding idx)
    for (ai, a) in analyzed.iter().enumerate() {
        for (bi, b) in a.analysis.bindings.iter().enumerate() {
            if b.text == old {
                candidates.push((ai, bi));
            }
        }
    }
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
        (ai, bi)
    } else {
        match candidates.len() {
            0 => {
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
            1 => candidates[0],
            _ => {
                let mut msg = format!(
                    "{old:?} is bound {} times; use --at to select one:\n",
                    candidates.len()
                );
                for (ai, bi) in candidates.iter().take(20) {
                    let a = &analyzed[*ai];
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
        binding_idx,
        &edits,
        new_decl,
        &ref_new_starts,
    )?;

    if let Some(planned) = syntax::finish_file(file, edits) {
        run.files.push(planned);
    }
    Ok(run)
}
