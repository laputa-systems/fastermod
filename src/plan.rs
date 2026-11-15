//! Typed edit plans: the shared representation produced by every engine and
//! consumed by review, JSON output, and application.
//!
//! A plan is invocation-local data, not a persisted index. `--plan` file
//! serialization and stale-plan revalidation land with REFAC-12; the types
//! and validation here are the contract they build on.

use std::path::Component;
use std::path::PathBuf;

use regex_lite::Regex;

use crate::review::Fastermod;
use crate::source::SourceFile;
use crate::text::{Substitution, index_to_row_col};
use crate::{Failure, operational};

pub const PLAN_SCHEMA: u32 = 1;

/// What kind of operation produced an edit. Serialized in JSON output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditKind {
    Text,
    Ident,
    Rewrite,
    RenameReference,
}

impl EditKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            EditKind::Text => "text",
            EditKind::Ident => "ident",
            EditKind::Rewrite => "rewrite",
            EditKind::RenameReference => "rename-reference",
        }
    }
}

/// 1-based line and 1-based UTF-8 byte column plus a source excerpt, so
/// agents need not count columns.
#[derive(Clone, Debug)]
pub(crate) struct Anchor {
    pub line: usize,
    pub column: usize,
    pub excerpt: String,
}

/// One byte-range replacement inside a file.
#[derive(Clone, Debug)]
pub(crate) struct SpanEdit {
    pub byte_start: usize,
    pub byte_end: usize,
    pub replacement: String,
    pub kind: EditKind,
    pub anchor: Anchor,
}

/// A file's complete before/after snapshot plus its edit records. `before`
/// and `after` are kept so preview, validation and application all describe
/// the same operation.
#[derive(Clone, Debug)]
pub(crate) struct PlannedFile {
    pub path: PathBuf,
    pub before: String,
    pub after: String,
    pub before_hash: u64,
    /// Set only after this file's staged write has been renamed into place.
    pub applied: bool,
    pub edits: Vec<SpanEdit>,
}

#[derive(Clone, Debug)]
pub(crate) struct SkippedFile {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub(crate) struct UnresolvedRef {
    pub path: PathBuf,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    pub message: String,
    pub path: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Coverage {
    pub roots: Vec<String>,
    pub files_scanned: usize,
    pub files_edited: usize,
    pub files_skipped: Vec<SkippedFile>,
    pub unresolved: Vec<UnresolvedRef>,
}

#[derive(Clone, Debug)]
pub(crate) struct EditPlan {
    pub schema: u32,
    pub mode: String,
    pub applied: bool,
    pub operation_id: String,
    pub coverage: Coverage,
    pub files: Vec<PlannedFile>,
    pub diagnostics: Vec<Diagnostic>,
}

/// FNV-1a 64-bit content fingerprint used for staleness checks.
pub(crate) fn content_hash(contents: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in contents.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn line_excerpt(contents: &str, byte_index: usize) -> String {
    let start = contents[..byte_index].rfind('\n').map_or(0, |i| i + 1);
    let end = contents[byte_index..]
        .find('\n')
        .map_or(contents.len(), |i| byte_index + i);
    contents[start..end].to_string()
}

/// Per-match edit records for a regex text substitution.
pub(crate) fn text_span_edits(
    regex: &Regex,
    substitution: &Substitution,
    file: &SourceFile,
) -> Vec<SpanEdit> {
    regex
        .captures_iter(&file.contents)
        .map(|captures| {
            let mat = captures.get(0).unwrap();
            let (line0, col0) = index_to_row_col(&file.contents, mat.start());
            SpanEdit {
                byte_start: mat.start(),
                byte_end: mat.end(),
                replacement: substitution.replacement(&captures),
                kind: EditKind::Text,
                anchor: Anchor {
                    line: line0 + 1,
                    column: col0 + 1,
                    excerpt: line_excerpt(&file.contents, mat.start()),
                },
            }
        })
        .collect()
}

/// Build a complete edit plan for text-mode substitution over `files`.
pub(crate) fn build_text_plan(
    mode: &str,
    regex: &Regex,
    substitution: &Substitution,
    files: &[SourceFile],
    roots: Vec<String>,
) -> EditPlan {
    let mut planned = Vec::new();
    for file in files {
        if let std::borrow::Cow::Owned(after) = substitution.replace_all(regex, &file.contents) {
            let edits = text_span_edits(regex, substitution, file);
            planned.push(PlannedFile {
                path: file.path.clone(),
                before_hash: content_hash(&file.contents),
                before: file.contents.clone(),
                after,
                applied: false,
                edits,
            });
        }
    }
    let coverage = Coverage {
        roots,
        files_scanned: files.len(),
        files_edited: planned.len(),
        files_skipped: Vec::new(),
        unresolved: Vec::new(),
    };
    // Content-derived so the id is stable across checkouts and machines;
    // paths are provenance in the plan itself, not part of the identity.
    let mut seed = mode.to_string();
    for file in &planned {
        seed.push_str(&format!(
            "{:016x}{:016x}",
            file.before_hash,
            content_hash(&file.after)
        ));
        for edit in &file.edits {
            seed.push_str(&format!("{}:{};", edit.byte_start, edit.byte_end));
        }
        seed.push(',');
    }
    EditPlan {
        schema: PLAN_SCHEMA,
        mode: mode.to_string(),
        applied: false,
        operation_id: format!("fm1-{:016x}", content_hash(&seed)),
        coverage,
        files: planned,
        diagnostics: Vec::new(),
    }
}

/// Structural validation of a plan before it may be reviewed or applied.
/// Returns human-readable problems; an empty list means the plan is well
/// formed. This checks plan-internal consistency only — staleness against
/// the filesystem is checked by [`apply`].
pub(crate) fn validate(plan: &EditPlan) -> Vec<String> {
    let mut problems = Vec::new();
    if plan.schema != PLAN_SCHEMA {
        problems.push(format!("unsupported plan schema {}", plan.schema));
    }
    for file in &plan.files {
        let path_label = file.path.display().to_string();
        if file.path.as_os_str().is_empty()
            || file
                .path
                .components()
                .any(|c| matches!(c, Component::ParentDir))
        {
            problems.push(format!("unsafe path in plan: {path_label}"));
        }
        let mut prev_end = 0usize;
        for edit in &file.edits {
            if edit.byte_start > edit.byte_end || edit.byte_end > file.before.len() {
                problems.push(format!(
                    "{path_label}: edit range {}..{} out of bounds (len {})",
                    edit.byte_start,
                    edit.byte_end,
                    file.before.len()
                ));
            }
            if !file.before.is_char_boundary(edit.byte_start)
                || !file.before.is_char_boundary(edit.byte_end)
            {
                problems.push(format!(
                    "{path_label}: edit range {}..{} splits a UTF-8 codepoint",
                    edit.byte_start, edit.byte_end
                ));
            }
            if edit.byte_start < prev_end {
                problems.push(format!(
                    "{path_label}: overlapping edits at byte {}",
                    edit.byte_start
                ));
            }
            prev_end = edit.byte_end;
        }
    }
    problems
}

/// Apply a validated plan against the current filesystem.
///
/// Every file's current bytes must still match the snapshot fingerprint and
/// every path must resolve inside a declared coverage root; any stale,
/// escaped or unreadable file refuses the whole operation before any write.
///
/// Writes are per-file atomic (same-directory stage + rename) but there is
/// no cross-file transaction: a mid-apply I/O failure leaves earlier files
/// applied. Each [`PlannedFile::applied`] records what happened so callers
/// can report exactly which files were written, and the returned error
/// lists applied and failed paths for recovery. The residual TOCTOU window
/// between the staleness read and the rename is documented in
/// docs/refactor/apply-safety.md.
pub(crate) fn apply(plan: &mut EditPlan) -> std::result::Result<usize, Failure> {
    let problems = validate(plan);
    if !problems.is_empty() {
        return Err(Failure::io(format!(
            "refusing to apply invalid plan: {}",
            problems.join("; ")
        )));
    }

    // Root containment: canonicalize declared roots once, then require every
    // planned file to resolve inside one of them. This is the gate that keeps
    // a hand-built or stale plan from writing outside the analyzed tree.
    let mut roots = Vec::new();
    for root in &plan.coverage.roots {
        match std::fs::canonicalize(root) {
            Ok(canonical) => roots.push(canonical),
            Err(e) => {
                return Err(Failure::io(format!(
                    "cannot canonicalize plan root {}: {e}",
                    root
                )));
            }
        }
    }

    let mut stale = Vec::new();
    let mut escaped = Vec::new();
    let mut unsafe_files = Vec::new();
    for file in &plan.files {
        // Preflight file type without following links: a symlink or
        // non-regular file in a plan is refused for the whole operation,
        // matching the save() guard.
        match std::fs::symlink_metadata(&file.path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                unsafe_files.push(format!("{} (symbolic link)", file.path.display()));
                continue;
            }
            Ok(metadata) if !metadata.file_type().is_file() => {
                unsafe_files.push(format!("{} (non-regular file)", file.path.display()));
                continue;
            }
            Ok(_) => {}
            Err(e) => {
                return Err(Failure::io(format!(
                    "unable to stat {}: {e}",
                    file.path.display()
                )));
            }
        }
        let current = match std::fs::read_to_string(&file.path) {
            Ok(current) => current,
            Err(e) => {
                return Err(Failure::io(format!(
                    "unable to read {} for staleness check: {e}",
                    file.path.display()
                )));
            }
        };
        match std::fs::canonicalize(&file.path) {
            Ok(canonical)
                if roots.iter().any(|root| canonical.starts_with(root)) => {}
            Ok(canonical) => escaped.push(format!(
                "{} (resolved to {})",
                file.path.display(),
                canonical.display()
            )),
            Err(e) => {
                return Err(Failure::io(format!(
                    "cannot canonicalize {}: {e}",
                    file.path.display()
                )));
            }
        }
        if content_hash(&current) != file.before_hash || current.len() != file.before.len() {
            stale.push(file.path.display().to_string());
        }
    }
    if !unsafe_files.is_empty() {
        return Err(Failure::io(format!(
            "plan contains unsafe file types: {}",
            unsafe_files.join(", ")
        )));
    }
    if !escaped.is_empty() {
        return Err(Failure::io(format!(
            "plan contains paths outside the declared roots: {}",
            escaped.join(", ")
        )));
    }
    if !stale.is_empty() {
        return Err(Failure::io(format!(
            "plan is stale; files changed since analysis: {}",
            stale.join(", ")
        )));
    }

    let mut fm = Fastermod::new(true, false, false, false);
    let mut failed = Vec::new();
    for file in &mut plan.files {
        match operational(fm.save(&file.path, &file.after)) {
            Ok(()) => file.applied = true,
            Err(e) => failed.push(format!("{} ({e})", file.path.display())),
        }
    }
    plan.applied = failed.is_empty();
    if !failed.is_empty() {
        let applied: Vec<String> = plan
            .files
            .iter()
            .filter(|f| f.applied)
            .map(|f| f.path.display().to_string())
            .collect();
        return Err(Failure::io(format!(
            "apply incomplete: {} of {} files written; applied: [{}]; failed: [{}]",
            applied.len(),
            plan.files.len(),
            applied.join(", "),
            failed.join(", ")
        )));
    }
    Ok(plan.files.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(start: usize, end: usize, replacement: &str) -> SpanEdit {
        SpanEdit {
            byte_start: start,
            byte_end: end,
            replacement: replacement.to_string(),
            kind: EditKind::Text,
            anchor: Anchor {
                line: 1,
                column: 1,
                excerpt: String::new(),
            },
        }
    }

    fn plan_with(path: &str, before: &str, edits: Vec<SpanEdit>) -> EditPlan {
        EditPlan {
            schema: PLAN_SCHEMA,
            mode: "text".to_string(),
            applied: false,
            operation_id: String::new(),
            coverage: Coverage::default(),
            files: vec![PlannedFile {
                path: PathBuf::from(path),
                before_hash: content_hash(before),
                before: before.to_string(),
                after: String::new(),
                applied: false,
                edits,
            }],
            diagnostics: Vec::new(),
        }
    }

    #[test]
    fn validate_accepts_ordered_non_overlapping_edits() {
        let plan = plan_with("a.txt", "foo bar", vec![edit(0, 3, "x"), edit(4, 7, "y")]);
        assert!(validate(&plan).is_empty());
    }

    #[test]
    fn validate_rejects_overlapping_edits() {
        let plan = plan_with("a.txt", "foobar", vec![edit(0, 4, "x"), edit(2, 6, "y")]);
        assert!(validate(&plan).iter().any(|p| p.contains("overlapping")));
    }

    #[test]
    fn validate_rejects_out_of_bounds_range() {
        let plan = plan_with("a.txt", "ab", vec![edit(0, 10, "x")]);
        assert!(validate(&plan).iter().any(|p| p.contains("out of bounds")));
    }

    #[test]
    fn validate_rejects_non_char_boundary_range() {
        // 'é' is two bytes; splitting it must fail.
        let plan = plan_with("a.txt", "aéb", vec![edit(1, 2, "x")]);
        assert!(validate(&plan)
            .iter()
            .any(|p| p.contains("UTF-8 codepoint")));
    }

    #[test]
    fn validate_rejects_parent_dir_path() {
        let plan = plan_with("../escape.txt", "ab", vec![edit(0, 1, "x")]);
        assert!(validate(&plan).iter().any(|p| p.contains("unsafe path")));
    }

    #[test]
    fn validate_rejects_wrong_schema() {
        let mut plan = plan_with("a.txt", "ab", vec![]);
        plan.schema = 99;
        assert!(validate(&plan).iter().any(|p| p.contains("schema")));
    }

    #[test]
    fn build_text_plan_is_deterministic() {
        let regex = Regex::new("o+").unwrap();
        let substitution = Substitution::new("0".to_string(), false);
        let file = SourceFile {
            path: PathBuf::from("a.txt"),
            contents: "foo boo".to_string(),
        };
        let a = build_text_plan(
            "text",
            &regex,
            &substitution,
            std::slice::from_ref(&file),
            vec![".".into()],
        );
        let b = build_text_plan("text", &regex, &substitution, &[file], vec![".".into()]);
        assert_eq!(a.operation_id, b.operation_id);
        assert_eq!(a.files[0].after, "f0 b0");
        assert_eq!(a.files[0].edits.len(), 2);
        assert!(validate(&a).is_empty());
    }
}

#[cfg(test)]
mod apply_tests {
    use std::fs;
    use std::time::SystemTime;

    use super::*;
    use crate::source::SourceFile;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> TempDir {
            let base = std::env::temp_dir();
            for _ in 0..1000 {
                let unique = format!(
                    "fastermod-plan-test-{}-{}",
                    std::process::id(),
                    SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos(),
                );
                let path = base.join(unique);
                if fs::create_dir(&path).is_ok() {
                    return TempDir(path);
                }
            }
            panic!("unable to create temp dir");
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write(dir: &TempDir, name: &str, contents: &str) -> PathBuf {
        let path = dir.0.join(name);
        fs::write(&path, contents).unwrap();
        path
    }

    fn plan_for(dir: &TempDir, names: &[&str]) -> EditPlan {
        let regex = Regex::new("foo").unwrap();
        let substitution = Substitution::new("bar".to_string(), false);
        let files: Vec<SourceFile> = names
            .iter()
            .map(|name| {
                let path = dir.0.join(name);
                SourceFile {
                    contents: fs::read_to_string(&path).unwrap(),
                    path,
                }
            })
            .collect();
        build_text_plan(
            "text",
            &regex,
            &substitution,
            &files,
            vec![dir.0.to_string_lossy().into_owned()],
        )
    }

    #[test]
    fn apply_writes_and_marks_files() {
        let dir = TempDir::new();
        write(&dir, "a.txt", "foo\n");
        write(&dir, "b.txt", "foo foo\n");
        let mut plan = plan_for(&dir, &["a.txt", "b.txt"]);
        assert_eq!(apply(&mut plan).unwrap(), 2);
        assert!(plan.applied);
        assert!(plan.files.iter().all(|f| f.applied));
        assert_eq!(fs::read_to_string(dir.0.join("a.txt")).unwrap(), "bar\n");
        assert_eq!(
            fs::read_to_string(dir.0.join("b.txt")).unwrap(),
            "bar bar\n"
        );
    }

    #[test]
    fn apply_refuses_stale_file_before_any_write() {
        let dir = TempDir::new();
        write(&dir, "a.txt", "foo\n");
        write(&dir, "b.txt", "foo\n");
        let mut plan = plan_for(&dir, &["a.txt", "b.txt"]);
        // Concurrent modification after analysis.
        fs::write(dir.0.join("b.txt"), "changed\n").unwrap();
        let err = apply(&mut plan).unwrap_err();
        assert!(err.to_string().contains("stale"), "{err}");
        // All-or-refuse: a.txt must not have been written.
        assert_eq!(fs::read_to_string(dir.0.join("a.txt")).unwrap(), "foo\n");
        assert!(plan.files.iter().all(|f| !f.applied));
        assert!(!plan.applied);
    }

    #[test]
    fn apply_refuses_file_outside_roots() {
        let dir = TempDir::new();
        let other = TempDir::new();
        write(&dir, "a.txt", "foo\n");
        let outside = write(&other, "out.txt", "foo\n");
        let regex = Regex::new("foo").unwrap();
        let substitution = Substitution::new("bar".to_string(), false);
        let file = SourceFile {
            path: outside.clone(),
            contents: "foo\n".to_string(),
        };
        // Roots claim only `dir`, but the plan targets a file in `other`.
        let mut plan = build_text_plan(
            "text",
            &regex,
            &substitution,
            std::slice::from_ref(&file),
            vec![dir.0.to_string_lossy().into_owned()],
        );
        // validate() rejects .. components but not this absolute path; the
        // containment gate in apply() is what must stop it.
        let err = apply(&mut plan).unwrap_err();
        assert!(err.to_string().contains("outside the declared roots"), "{err}");
        assert_eq!(fs::read_to_string(&outside).unwrap(), "foo\n");
        let _ = dir;
    }

    #[cfg(unix)]
    #[test]
    fn apply_refuses_symlink_target() {
        let dir = TempDir::new();
        let real = write(&dir, "real.txt", "foo\n");
        let link = dir.0.join("link.txt");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let regex = Regex::new("foo").unwrap();
        let substitution = Substitution::new("bar".to_string(), false);
        let file = SourceFile {
            path: link.clone(),
            contents: "foo\n".to_string(),
        };
        let mut plan = build_text_plan(
            "text",
            &regex,
            &substitution,
            std::slice::from_ref(&file),
            vec![dir.0.to_string_lossy().into_owned()],
        );
        let err = apply(&mut plan).unwrap_err();
        assert!(err.to_string().contains("unsafe file types"), "{err}");
        assert_eq!(fs::read_to_string(&real).unwrap(), "foo\n");
    }

    #[cfg(unix)]
    #[test]
    fn apply_reports_partial_failure_per_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new();
        write(&dir, "a.txt", "foo\n");
        let locked = write(&dir, "b.txt", "foo\n");
        let mut permissions = fs::metadata(&locked).unwrap().permissions();
        permissions.set_mode(0o444);
        fs::set_permissions(&locked, permissions).unwrap();

        let mut plan = plan_for(&dir, &["a.txt", "b.txt"]);
        let err = apply(&mut plan).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("apply incomplete"), "{message}");
        assert!(message.contains("a.txt"), "{message}");
        assert!(message.contains("b.txt"), "{message}");
        // a.txt applied, b.txt did not; per-file flags tell the truth.
        assert_eq!(fs::read_to_string(dir.0.join("a.txt")).unwrap(), "bar\n");
        assert_eq!(fs::read_to_string(&locked).unwrap(), "foo\n");
        let a = plan.files.iter().find(|f| f.path.ends_with("a.txt")).unwrap();
        let b = plan.files.iter().find(|f| f.path.ends_with("b.txt")).unwrap();
        assert!(a.applied);
        assert!(!b.applied);
        assert!(!plan.applied);
        // Restore so TempDir cleanup is not affected on strict filesystems.
        let mut permissions = fs::metadata(&locked).unwrap().permissions();
        permissions.set_mode(0o644);
        fs::set_permissions(&locked, permissions).unwrap();
    }
}
