//! Declarative `--rules` migrations: ordered, language-scoped operations
//! executed sequentially over in-memory file contents. Configuration is
//! data-only TOML — no code, no includes, no eval.

use std::collections::BTreeMap;
use std::path::PathBuf;

use grep::regex::RegexMatcherBuilder;

use crate::cli::Mode;
use crate::discovery::{FileSet, matching_files};
use crate::plan::{self, EditKind, EditPlan, PlannedFile, SpanEdit};
use crate::source::SourceFile;
use crate::{operational, rename, syntax, Failure};

enum OpKind {
    Ident { old: String, new: String },
    Rewrite { pattern: String, replacement: String },
    Rename { old: String, new: String, at: Option<String> },
}

struct Op {
    kind: OpKind,
    lang: Option<String>,
}

fn req_str<'a>(t: &'a toml_edit::Table, key: &str) -> Result<&'a str, Failure> {
    t.get(key)
        .and_then(|i| i.as_str())
        .ok_or_else(|| Failure::usage(format!("rules op is missing string field `{key}`")))
}

fn parse_rules(text: &str) -> Result<Vec<Op>, Failure> {
    let doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| Failure::usage(format!("invalid rules file: {e}")))?;
    let ops = doc
        .get("op")
        .and_then(|i| i.as_array_of_tables())
        .ok_or_else(|| Failure::usage("rules file must contain at least one [[op]] table"))?;
    let mut out = Vec::new();
    for (i, t) in ops.iter().enumerate() {
        let kind = match req_str(t, "type")? {
            "ident" => OpKind::Ident {
                old: req_str(t, "old")?.to_string(),
                new: req_str(t, "new")?.to_string(),
            },
            "rewrite" => OpKind::Rewrite {
                pattern: req_str(t, "pattern")?.to_string(),
                replacement: req_str(t, "replacement")?.to_string(),
            },
            "rename" => OpKind::Rename {
                old: req_str(t, "old")?.to_string(),
                new: req_str(t, "new")?.to_string(),
                at: t.get("at").and_then(|v| v.as_str()).map(str::to_string),
            },
            other => {
                return Err(Failure::usage(format!(
                    "rules op {i}: unknown type `{other}` (expected ident, rewrite, or rename)"
                )));
            }
        };
        out.push(Op {
            kind,
            lang: t.get("lang").and_then(|v| v.as_str()).map(str::to_string),
        });
    }
    Ok(out)
}

/// Collapse a before/after pair into one minimal byte-range edit using the
/// longest common prefix/suffix. Edits from sequential ops cannot be merged
/// byte-wise, so the plan records one honest contiguous replacement per file.
fn collapse_edit(before: &str, after: &str, kind: EditKind) -> Option<SpanEdit> {
    if before == after {
        return None;
    }
    let b = before.as_bytes();
    let a = after.as_bytes();
    let mut start = 0;
    while start < b.len().min(a.len()) && b[start] == a[start] {
        start += 1;
    }
    let mut end_b = b.len();
    let mut end_a = a.len();
    while end_b > start && end_a > start && b[end_b - 1] == a[end_a - 1] {
        end_b -= 1;
        end_a -= 1;
    }
    while !before.is_char_boundary(start) {
        start += 1;
    }
    while !before.is_char_boundary(end_b) {
        end_b -= 1;
    }
    while !after.is_char_boundary(start) {
        start += 1;
    }
    while !after.is_char_boundary(end_a) {
        end_a -= 1;
    }
    let (line0, col0) = crate::text::index_to_row_col(before, start);
    Some(SpanEdit {
        byte_start: start,
        byte_end: end_b,
        replacement: after[start..end_a].to_string(),
        kind,
        anchor: plan::Anchor {
            line: line0 + 1,
            column: col0 + 1,
            excerpt: plan::line_excerpt(before, start),
        },
    })
}

/// Run an ordered rules file over the discovered file set.
pub(crate) fn run_rules(
    rules_path: &str,
    dirs: Vec<&str>,
    file_set: Option<FileSet>,
    hidden: bool,
    no_ignore: bool,
    cli_langs: &[String],
    roots: &[String],
) -> std::result::Result<EditPlan, Failure> {
    let text = std::fs::read_to_string(rules_path)
        .map_err(|e| Failure::io(format!("cannot read rules file {rules_path}: {e}")))?;
    let ops = parse_rules(&text)?;
    if ops.is_empty() {
        return Err(Failure::usage("rules file contains no [[op]] tables"));
    }

    // Discover every candidate file once; ops thread contents in memory.
    let matcher = RegexMatcherBuilder::new()
        .build("")
        .map_err(|e| Failure::usage(format!("Unable to make matcher: {e}")))?;
    let discovered = operational(matching_files(
        dirs.clone(),
        file_set.clone(),
        hidden,
        no_ignore,
        &matcher,
    ))?;
    let scanned = discovered.len();

    let mut originals: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut current: BTreeMap<PathBuf, String> = BTreeMap::new();
    for f in &discovered {
        originals.insert(f.path.clone(), f.contents.clone());
        current.insert(f.path.clone(), f.contents.clone());
    }

    let mut skipped: Vec<plan::SkippedFile> = Vec::new();
    let mut diagnostics: Vec<plan::Diagnostic> = Vec::new();
    let mut residual_names: Vec<String> = Vec::new();
    // Files no op could parse (no registered language) cannot contain
    // resolvable references; residual review only scans analyzable files.
    let mut non_language: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();

    for (i, op) in ops.iter().enumerate() {
        let files: Vec<SourceFile> = current
            .iter()
            .map(|(p, c)| SourceFile {
                path: p.clone(),
                contents: c.clone(),
            })
            .collect();
        let langs: &[String] = match &op.lang {
            Some(l) => std::slice::from_ref(l),
            None => cli_langs,
        };
        let run = match &op.kind {
            OpKind::Ident { old, new } => syntax::run_ident(old, new, &files, langs)?,
            OpKind::Rewrite { pattern, replacement } => {
                syntax::run_rewrite(pattern, replacement, &files, langs)?
            }
            OpKind::Rename { old, new, at } => {
                rename::run_rename(old, new, &files, at.as_deref(), langs)?
            }
        };
        for s in run.skipped {
            if s.reason.contains("no registered language") {
                non_language.insert(s.path.clone());
            }
            skipped.push(plan::SkippedFile {
                path: s.path,
                reason: format!("op {i}: {}", s.reason),
            });
        }
        for d in run.diagnostics {
            diagnostics.push(plan::Diagnostic {
                path: d.path,
                ..d
            });
        }
        // Thread in-memory contents forward: untouched files keep their text.
        let mut touched = std::collections::BTreeSet::new();
        for pf in run.files {
            if let Some(slot) = current.get_mut(&pf.path) {
                *slot = pf.after.clone();
                touched.insert(pf.path);
            }
        }
        match &op.kind {
            OpKind::Ident { old, .. } | OpKind::Rename { old, .. } => {
                residual_names.push(old.clone());
            }
            _ => {}
        }
        let _ = touched;
    }

    // Residual review aid: spellings that ops targeted but that still appear
    // in final contents are reported, never silently accepted.
    for name in &residual_names {
        let mut hits = 0usize;
        for (p, c) in &current {
            if !non_language.contains(p) {
                hits += c.matches(name.as_str()).count();
            }
        }
        if hits > 0 {
            diagnostics.push(plan::Diagnostic {
                severity: plan::Severity::Warning,
                code: "residual-reference".to_string(),
                message: format!(
                    "{hits} occurrence(s) of `{name}` remain after rules execution; review before applying"
                ),
                path: None,
            });
        }
    }

    // Build the final plan: one collapsed edit per changed file.
    let mut files_out: Vec<PlannedFile> = Vec::new();
    for (path, before) in &originals {
        let after = &current[path];
        let edits: Vec<SpanEdit> = collapse_edit(before, after, EditKind::Rewrite)
            .into_iter()
            .collect();
        if edits.is_empty() {
            continue;
        }
        files_out.push(PlannedFile {
            path: path.clone(),
            before: before.clone(),
            after: after.clone(),
            before_hash: plan::content_hash(before),
            applied: false,
            edits,
        });
    }

    let read = operational(crate::discovery::fingerprint_read_set(
        &discovered,
        dirs,
        file_set,
        hidden,
        no_ignore,
    ))?;
    let mut plan = plan::assemble_plan(
        Mode::Rules.name(),
        files_out,
        scanned,
        roots.to_vec(),
        skipped,
        diagnostics,
        Some(read),
    );
    plan.config = Some(format!(
        "rules-file:{:016x}",
        plan::content_hash(&text)
    ));
    Ok(plan)
}
