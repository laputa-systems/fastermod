//! Syntax engines for --ident and --rewrite. Both parse each candidate
//! file once with the language's tree-sitter grammar (via ast-grep) and
//! emit typed span edits; they never fall back to text matching and never
//! claim binding awareness.

use ast_grep_core::matcher::Pattern;
use ast_grep_core::tree_sitter::{LanguageExt, StrDoc};
use ast_grep_core::AstGrep;

use crate::embed;
use crate::lang::{self, LangSpec};
use crate::plan::{self, Anchor, EditKind, PlannedFile, SpanEdit, content_hash};
use crate::source::SourceFile;
use crate::text::index_to_row_col;
use crate::Failure;

/// Outcome of running a syntax engine over the discovered file set.
pub(crate) struct SyntaxRun {
    /// Files with planned edits.
    pub files: Vec<PlannedFile>,
    /// Files skipped because no language applied or parsing failed.
    pub skipped: Vec<plan::SkippedFile>,
    /// Non-fatal diagnostics (e.g. unsupported operation for a language).
    pub diagnostics: Vec<plan::Diagnostic>,
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

/// Parse `contents` with `spec`'s grammar. Returns the ast-grep root, or
/// `None` when the tree has syntax errors (the caller records a skip).
fn parse(spec: &LangSpec, contents: &str) -> Option<AstGrep<StrDoc<crate::lang::Grammar>>> {
    let root = spec.grammar.ast_grep(contents);
    if root.root().get_inner_node().has_error() {
        return None;
    }
    Some(root)
}

pub(crate) fn finish_file(file: &SourceFile, edits: Vec<SpanEdit>) -> Option<PlannedFile> {
    if edits.is_empty() {
        return None;
    }
    // Apply edits back-to-front so byte offsets stay valid.
    let mut after = file.contents.clone();
    for edit in edits.iter().rev() {
        after.replace_range(edit.byte_start..edit.byte_end, &edit.replacement);
    }
    Some(PlannedFile {
        path: file.path.clone(),
        before_hash: content_hash(&file.contents),
        before: file.contents.clone(),
        after,
        applied: false,
        edits,
    })
}

fn empty_run() -> SyntaxRun {
    SyntaxRun {
        files: Vec::new(),
        skipped: Vec::new(),
        diagnostics: Vec::new(),
    }
}

/// --ident OLD NEW: replace every identifier-token node whose text is
/// exactly OLD. Syntactic only — strings, comments and non-identifier
/// nodes can never match because they are not identifier kinds.
pub(crate) fn run_ident(
    old: &str,
    new: &str,
    files: &[SourceFile],
    lang_overrides: &[String],
) -> std::result::Result<SyntaxRun, Failure> {
    if old.is_empty() {
        return Err(Failure::usage("--ident requires a non-empty identifier"));
    }
    if new.is_empty() || new.chars().any(char::is_whitespace) {
        return Err(Failure::usage(
            "--ident replacement must be a single identifier token",
        ));
    }
    let mut run = empty_run();
    for file in files {
        let mut edits = Vec::new();
        let mut host_done = false;
        if let Some(spec) = lang::detect(&file.path, lang_overrides) {
            host_done = true;
            if !spec.ident {
                run.diagnostics.push(plan::Diagnostic {
                    severity: plan::Severity::Info,
                    code: "unsupported-operation".to_string(),
                    message: format!("--ident is not supported for {}", spec.id),
                    path: Some(file.path.clone()),
                });
            } else if let Some(root) = parse(spec, &file.contents) {
                for node in root.root().dfs() {
                    let kind = node.kind();
                    if (lang::is_identifier_kind(kind.as_ref())
                        || spec.ident_kinds.contains(&kind.as_ref()))
                        && node.text().as_ref() == old
                    {
                        let range = node.range();
                        edits.push(SpanEdit {
                            byte_start: range.start,
                            byte_end: range.end,
                            replacement: new.to_string(),
                            kind: EditKind::Ident,
                            anchor: anchor_for(&file.contents, range.start),
                        });
                    }
                }
            } else {
                run.skipped.push(plan::SkippedFile {
                    path: file.path.clone(),
                    reason: format!("{} parse contains syntax errors", spec.id),
                });
            }
        }
        // Embedded regions (HTML/Vue/Svelte script+style, Markdown fences)
        // are edited with the inner language's grammar; offsets map back
        // into host bytes so delimiters are preserved.
        for region in embed::regions(&file.path, &file.contents) {
            if !region.spec.ident {
                continue;
            }
            let sub = &file.contents[region.range.clone()];
            let Some(root) = parse(region.spec, sub) else {
                run.diagnostics.push(plan::Diagnostic {
                    severity: plan::Severity::Info,
                    code: "embedded-parse".to_string(),
                    message: format!(
                        "embedded {} region at byte {} failed to parse",
                        region.spec.id, region.range.start
                    ),
                    path: Some(file.path.clone()),
                });
                continue;
            };
            for node in root.root().dfs() {
                let kind = node.kind();
                if (lang::is_identifier_kind(kind.as_ref())
                    || region.spec.ident_kinds.contains(&kind.as_ref()))
                    && node.text().as_ref() == old
                {
                    let r = node.range();
                    let abs = region.range.start + r.start;
                    edits.push(SpanEdit {
                        byte_start: abs,
                        byte_end: region.range.start + r.end,
                        replacement: new.to_string(),
                        kind: EditKind::Ident,
                        anchor: anchor_for(&file.contents, abs),
                    });
                }
            }
        }
        if !host_done && edits.is_empty() {
            run.skipped.push(plan::SkippedFile {
                path: file.path.clone(),
                reason: "no registered language matches this file".to_string(),
            });
        }
        if let Some(planned) = finish_file(file, edits) {
            run.files.push(planned);
        }
    }
    Ok(run)
}

/// --rewrite PATTERN REPLACEMENT: ast-grep structural pattern + template
/// rewrite in the detected language. PATTERN uses ast-grep syntax
/// (`$$$ARGS` multi-metavar, `$VAR` single); REPLACEMENT is an ast-grep
/// template. This is a syntax rewrite, not a binding-aware rename.
pub(crate) fn run_rewrite(
    pattern_src: &str,
    replacement: &str,
    files: &[SourceFile],
    lang_overrides: &[String],
) -> std::result::Result<SyntaxRun, Failure> {
    // Compile the pattern once per registered language up front: an
    // invalid pattern is a usage error even when no file would match, and
    // a pattern that parses in one grammar may not parse in another.
    let compiled: Vec<(&'static LangSpec, std::result::Result<Pattern, String>)> = lang::LANGUAGES
        .iter()
        .map(|spec| {
            let result = Pattern::try_new(pattern_src, spec.grammar).map_err(|e| e.to_string());
            (spec, result)
        })
        .collect();
    if compiled.iter().all(|(_, r)| r.is_err()) {
        let first = compiled[0].1.as_ref().unwrap_err();
        return Err(Failure::usage(format!(
            "invalid --rewrite pattern: {first}"
        )));
    }
    // Collect rewrite edits for one parsed root; `offset` maps positions
    // from a substring (embedded region) into host bytes.
    fn rewrite_edits(
        root: &AstGrep<StrDoc<crate::lang::Grammar>>,
        pattern: &Pattern,
        replacement: &str,
        file: &SourceFile,
        offset: usize,
        edits: &mut Vec<SpanEdit>,
    ) -> std::result::Result<(), Failure> {
        for edit in root.root().replace_all(pattern, replacement) {
            let text = String::from_utf8(edit.inserted_text).map_err(|_| {
                Failure::analysis(format!(
                    "rewrite template produced non-UTF-8 output in {}",
                    file.path.display()
                ))
            })?;
            let pos = offset + edit.position;
            edits.push(SpanEdit {
                byte_start: pos,
                byte_end: offset + edit.position + edit.deleted_length,
                replacement: text,
                kind: EditKind::Rewrite,
                anchor: anchor_for(&file.contents, pos),
            });
        }
        Ok(())
    }

    let mut run = empty_run();
    for file in files {
        let mut edits = Vec::new();
        let mut host_done = false;
        if let Some(spec) = lang::detect(&file.path, lang_overrides) {
            host_done = true;
            if !spec.rewrite {
                run.diagnostics.push(plan::Diagnostic {
                    severity: plan::Severity::Info,
                    code: "unsupported-operation".to_string(),
                    message: format!("--rewrite is not supported for {}", spec.id),
                    path: Some(file.path.clone()),
                });
            } else {
                match compiled.iter().find(|(s, _)| s.id == spec.id).map(|(_, r)| r) {
                    Some(Ok(pattern)) => {
                        if let Some(root) = parse(spec, &file.contents) {
                            rewrite_edits(&root, pattern, replacement, file, 0, &mut edits)?;
                        } else {
                            run.skipped.push(plan::SkippedFile {
                                path: file.path.clone(),
                                reason: format!("{} parse contains syntax errors", spec.id),
                            });
                        }
                    }
                    Some(Err(e)) => {
                        run.skipped.push(plan::SkippedFile {
                            path: file.path.clone(),
                            reason: format!(
                                "--rewrite pattern does not parse as {}: {e}",
                                spec.id
                            ),
                        });
                    }
                    None => {}
                }
            }
        }
        // Embedded regions use the inner language's compiled pattern.
        for region in embed::regions(&file.path, &file.contents) {
            if !region.spec.rewrite {
                continue;
            }
            let Some(pattern) = compiled
                .iter()
                .find(|(s, _)| s.id == region.spec.id)
                .and_then(|(_, r)| r.as_ref().ok())
            else {
                continue;
            };
            let sub = &file.contents[region.range.clone()];
            match parse(region.spec, sub) {
                Some(root) => {
                    rewrite_edits(
                        &root,
                        pattern,
                        replacement,
                        file,
                        region.range.start,
                        &mut edits,
                    )?;
                }
                None => {
                    run.diagnostics.push(plan::Diagnostic {
                        severity: plan::Severity::Info,
                        code: "embedded-parse".to_string(),
                        message: format!(
                            "embedded {} region at byte {} failed to parse",
                            region.spec.id, region.range.start
                        ),
                        path: Some(file.path.clone()),
                    });
                }
            }
        }
        if !host_done && edits.is_empty() {
            run.skipped.push(plan::SkippedFile {
                path: file.path.clone(),
                reason: "no registered language matches this file".to_string(),
            });
        }
        if let Some(planned) = finish_file(file, edits) {
            run.files.push(planned);
        }
    }
    Ok(run)
}
