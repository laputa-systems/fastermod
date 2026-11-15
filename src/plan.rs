//! Typed edit plans: the shared representation produced by every engine and
//! consumed by review, JSON output, and application.
//!
//! A plan is invocation-local data, not a persisted index. `--plan` file
//! serialization and stale-plan revalidation live here alongside the
//! types and validation that are the contract they build on.

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
    /// A collapsed whole-file diff produced by a multi-op `--rules` run;
    /// per-op kinds are not recoverable from the merged span.
    Mixed,
}

impl EditKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            EditKind::Text => "text",
            EditKind::Ident => "ident",
            EditKind::Rewrite => "rewrite",
            EditKind::RenameReference => "rename-reference",
            EditKind::Mixed => "mixed",
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

/// One file the engines actually read, with its content fingerprint.
/// Unedited read-set members still invalidate a saved plan when they
/// change — imports, manifests, and configs the analysis depended on.
#[derive(Clone, Debug)]
pub(crate) struct ReadSetEntry {
    pub path: PathBuf,
    pub hash: u64,
}

/// Serializable mirror of the discovery parameters used for a run, so
/// `--apply-plan` can re-walk the same roots and detect added/removed
/// files even when none of the edited files changed.
#[derive(Clone, Debug)]
pub(crate) struct DiscoverySpec {
    pub hidden: bool,
    pub no_ignore: bool,
    pub file_set: Option<crate::discovery::FileSet>,
    /// Fingerprint of every file path the walk visited, sorted.
    pub seen_hash: u64,
}

/// Everything needed to revalidate the analysis read-set at apply time.
#[derive(Clone, Debug)]
pub(crate) struct ReadContext {
    pub entries: Vec<ReadSetEntry>,
    pub discovery: DiscoverySpec,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Coverage {
    pub roots: Vec<String>,
    pub files_scanned: usize,
    pub files_edited: usize,
    pub files_skipped: Vec<SkippedFile>,
    pub unresolved: Vec<UnresolvedRef>,
    pub read_set: Vec<ReadSetEntry>,
    pub discovery: Option<DiscoverySpec>,
}

#[derive(Clone, Debug)]
pub(crate) struct EditPlan {
    pub schema: u32,
    pub mode: String,
    pub applied: bool,
    pub operation_id: String,
    /// Data-only fingerprint of the operation configuration (rules file
    /// hash or mode/pattern/languages digest) recorded as evidence.
    pub config: Option<String>,
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

pub(crate) fn line_excerpt(contents: &str, byte_index: usize) -> String {
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
#[cfg(test)]
fn build_text_plan(
    mode: &str,
    regex: &Regex,
    substitution: &Substitution,
    files: &[SourceFile],
    roots: Vec<String>,
) -> EditPlan {
    build_text_plan_read(mode, regex, substitution, files, roots, None)
}

pub(crate) fn build_text_plan_read(
    mode: &str,
    regex: &Regex,
    substitution: &Substitution,
    files: &[SourceFile],
    roots: Vec<String>,
    read: Option<ReadContext>,
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
    assemble_plan(
        mode,
        planned,
        files.len(),
        roots,
        Vec::new(),
        Vec::new(),
        read,
    )
}

/// Assemble a complete plan from engine-produced files plus coverage
/// evidence. Shared by every engine so operation ids, coverage and
/// diagnostics have one shape.
pub(crate) fn assemble_plan(
    mode: &str,
    files: Vec<PlannedFile>,
    files_scanned: usize,
    roots: Vec<String>,
    files_skipped: Vec<SkippedFile>,
    diagnostics: Vec<Diagnostic>,
    read: Option<ReadContext>,
) -> EditPlan {
    let files_edited = files.len();
    // Content-derived so the id is stable across checkouts and machines;
    // paths are provenance in the plan itself, not part of the identity.
    let mut seed = mode.to_string();
    for file in &files {
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
        config: None,
        coverage: Coverage {
            roots,
            files_scanned,
            files_edited,
            files_skipped,
            unresolved: Vec::new(),
            read_set: read.as_ref().map(|r| r.entries.clone()).unwrap_or_default(),
            discovery: read.map(|r| r.discovery),
        },
        files,
        diagnostics,
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
    apply_excluding(plan, None)
}

/// Apply with an optional path excluded from the discovery re-walk — the
/// saved plan file itself may live inside a scanned root and must not
/// count as an "added file".
pub(crate) fn apply_excluding(
    plan: &mut EditPlan,
    exclude: Option<&std::path::Path>,
) -> std::result::Result<usize, Failure> {
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

    // Read-set revalidation: plans record every file the analysis read
    // plus a fingerprint of the whole discovered listing. Re-walking the
    // declared roots catches added/removed files, .gitignore drift, and
    // changes to unedited dependencies (imports, manifests, configs).
    if let Some(disc) = &plan.coverage.discovery {
        let spec_roots: Vec<&str> = plan.coverage.roots.iter().map(String::as_str).collect();
        let seen = crate::operational(crate::discovery::seen_files(
            spec_roots,
            disc.file_set.clone(),
            disc.hidden,
            disc.no_ignore,
        ))?;
        let seen: Vec<std::path::PathBuf> = match exclude {
            Some(ex) => {
                let ex_canon = std::fs::canonicalize(ex).ok();
                let ex_name = ex.file_name().map(|n| n.to_os_string());
                seen.into_iter()
                    .filter(|p| {
                        if ex_name.as_ref() != p.file_name().map(|n| n.to_os_string()).as_ref() {
                            return true;
                        }
                        match (&ex_canon, std::fs::canonicalize(p).ok()) {
                            (Some(a), Some(b)) => a != &b,
                            _ => true,
                        }
                    })
                    .collect()
            }
            None => seen,
        };
        if crate::discovery::seen_fingerprint(&seen) != disc.seen_hash {
            return Err(Failure::io(
                "plan is stale; files added or removed since analysis".to_string(),
            ));
        }
        let mut read_stale = Vec::new();
        for entry in &plan.coverage.read_set {
            match std::fs::read_to_string(&entry.path) {
                Ok(current) if content_hash(&current) == entry.hash => {}
                _ => read_stale.push(entry.path.display().to_string()),
            }
        }
        if !read_stale.is_empty() {
            return Err(Failure::io(format!(
                "plan is stale; analyzed files changed since analysis: {}",
                read_stale.join(", ")
            )));
        }
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
            config: None,
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
    #[test]
    fn save_load_round_trip_preserves_read_set() {
        let dir = TempDir::new();
        write(&dir, "a.txt", "foo\n");
        write(&dir, "dep.txt", "dep\n");
        let regex = Regex::new("foo").unwrap();
        let substitution = Substitution::new("bar".to_string(), false);
        let files = vec![
            SourceFile { path: dir.0.join("a.txt"), contents: "foo\n".into() },
            SourceFile { path: dir.0.join("dep.txt"), contents: "dep\n".into() },
        ];
        let read = crate::discovery::fingerprint_read_set(
            &files,
            vec![dir.0.to_str().unwrap()],
            None,
            false,
            false,
        )
        .unwrap();
        let mut plan = build_text_plan_read(
            "text",
            &regex,
            &substitution,
            &files,
            vec![dir.0.to_string_lossy().into_owned()],
            Some(read),
        );
        plan.config = Some("test-config".to_string());
        let text = serde_json::to_string_pretty(&save(&plan)).unwrap();
        let loaded = load(&text).unwrap();
        assert_eq!(loaded.schema, PLAN_SCHEMA);
        assert_eq!(loaded.config.as_deref(), Some("test-config"));
        assert_eq!(loaded.coverage.read_set.len(), 2);
        assert!(loaded.coverage.discovery.is_some());
        assert_eq!(loaded.files.len(), plan.files.len());
    }

    #[test]
    fn apply_rejects_changed_unedited_read_set_file() {
        let dir = TempDir::new();
        write(&dir, "a.txt", "foo\n");
        write(&dir, "dep.txt", "dep\n");
        let regex = Regex::new("foo").unwrap();
        let substitution = Substitution::new("bar".to_string(), false);
        let dep_path = dir.0.join("dep.txt");
        let files = vec![
            SourceFile { path: dir.0.join("a.txt"), contents: "foo\n".into() },
            SourceFile { contents: "dep\n".into(), path: dep_path.clone() },
        ];
        let read = crate::discovery::fingerprint_read_set(
            &files,
            vec![dir.0.to_str().unwrap()],
            None,
            false,
            false,
        )
        .unwrap();
        // dep.txt is not edited by the plan but was analyzed: changing it
        // must invalidate application.
        fs::write(&dep_path, "changed\n").unwrap();
        let mut plan = build_text_plan_read(
            "text",
            &regex,
            &substitution,
            &files,
            vec![dir.0.to_string_lossy().into_owned()],
            Some(read),
        );
        let err = apply(&mut plan).unwrap_err();
        assert!(err.to_string().contains("stale"), "{err}");
        assert_eq!(fs::read_to_string(dir.0.join("a.txt")).unwrap(), "foo\n");
    }

    #[test]
    fn apply_rejects_added_file_via_seen_fingerprint() {
        let dir = TempDir::new();
        write(&dir, "a.txt", "foo\n");
        let regex = Regex::new("foo").unwrap();
        let substitution = Substitution::new("bar".to_string(), false);
        let files = vec![SourceFile {
            path: dir.0.join("a.txt"),
            contents: "foo\n".into(),
        }];
        let read = crate::discovery::fingerprint_read_set(
            &files,
            vec![dir.0.to_str().unwrap()],
            None,
            false,
            false,
        )
        .unwrap();
        fs::write(dir.0.join("added.txt"), "new\n").unwrap();
        let mut plan = build_text_plan_read(
            "text",
            &regex,
            &substitution,
            &files,
            vec![dir.0.to_string_lossy().into_owned()],
            Some(read),
        );
        let err = apply(&mut plan).unwrap_err();
        assert!(err.to_string().contains("added or removed"), "{err}");
    }

}

fn edit_json(edit: &SpanEdit) -> serde_json::Value {
    serde_json::json!({
        "byte_start": edit.byte_start,
        "byte_end": edit.byte_end,
        "replacement": edit.replacement,
        "kind": edit.kind.as_str(),
        "anchor": {
            "line": edit.anchor.line,
            "column": edit.anchor.column,
            "excerpt": edit.anchor.excerpt,
        },
    })
}

fn file_json(file: &PlannedFile) -> serde_json::Value {
    serde_json::json!({
        "path": file.path.to_string_lossy(),
        "before_hash": file.before_hash,
        "before": file.before,
        "after": file.after,
        "edits": file.edits.iter().map(edit_json).collect::<Vec<_>>(),
    })
}

/// Serialize a plan to its versioned JSON document. Plans are data,
/// not scripts: every field needed to revalidate and apply is recorded.
pub(crate) fn save(plan: &EditPlan) -> serde_json::Value {
    serde_json::json!({
        "schema": plan.schema,
        "mode": plan.mode,
        "operation_id": plan.operation_id,
        "applied": plan.applied,
        "config": plan.config,
        "coverage": {
            "roots": plan.coverage.roots,
            "files_scanned": plan.coverage.files_scanned,
            "files_edited": plan.coverage.files_edited,
            "files_skipped": plan.coverage.files_skipped.iter().map(|s| serde_json::json!({
                "path": s.path.to_string_lossy(),
                "reason": s.reason,
            })).collect::<Vec<_>>(),
            "unresolved": plan.coverage.unresolved.iter().map(|u| serde_json::json!({
                "path": u.path.to_string_lossy(),
                "detail": u.detail,
            })).collect::<Vec<_>>(),
            "read_set": plan.coverage.read_set.iter().map(|e| serde_json::json!({
                "path": e.path.to_string_lossy(),
                "hash": e.hash,
            })).collect::<Vec<_>>(),
            "discovery": plan.coverage.discovery.as_ref().map(|d| {
                let file_set = d.file_set.as_ref().map(|fs| match fs {
                    crate::discovery::FileSet::Extensions(exts) => serde_json::json!({
                        "kind": "extensions",
                        "values": exts,
                    }),
                    crate::discovery::FileSet::Glob { matches, case_insensitive } => serde_json::json!({
                        "kind": "glob",
                        "matches": matches,
                        "case_insensitive": case_insensitive,
                    }),
                });
                serde_json::json!({
                    "hidden": d.hidden,
                    "no_ignore": d.no_ignore,
                    "file_set": file_set,
                    "seen_hash": d.seen_hash,
                })
            }),
        },
        "files": plan.files.iter().map(file_json).collect::<Vec<_>>(),
        "diagnostics": plan.diagnostics.iter().map(|d| serde_json::json!({
            "severity": d.severity.as_str(),
            "code": d.code,
            "message": d.message,
            "path": d.path.as_ref().map(|p| p.to_string_lossy().to_string()),
        })).collect::<Vec<_>>(),
    })
}

fn get_str<'a>(v: &'a serde_json::Value, key: &str) -> std::result::Result<&'a str, String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .ok_or_else(|| format!("plan field {key:?} missing or not a string"))
}

fn get_u64(v: &serde_json::Value, key: &str) -> std::result::Result<u64, String> {
    v.get(key)
        .and_then(|x| x.as_u64())
        .ok_or_else(|| format!("plan field {key:?} missing or not an integer"))
}

fn path_from(v: &serde_json::Value, key: &str) -> std::result::Result<PathBuf, String> {
    Ok(PathBuf::from(get_str(v, key)?))
}

fn edit_from(v: &serde_json::Value) -> std::result::Result<SpanEdit, String> {
    let kind = match get_str(v, "kind")? {
        "text" => EditKind::Text,
        "ident" => EditKind::Ident,
        "rewrite" => EditKind::Rewrite,
        "rename-reference" => EditKind::RenameReference,
        "mixed" => EditKind::Mixed,
        other => return Err(format!("unknown edit kind {other:?}")),
    };
    let anchor = v
        .get("anchor")
        .ok_or_else(|| "edit missing anchor".to_string())?;
    Ok(SpanEdit {
        byte_start: get_u64(v, "byte_start")? as usize,
        byte_end: get_u64(v, "byte_end")? as usize,
        replacement: get_str(v, "replacement")?.to_string(),
        kind,
        anchor: Anchor {
            line: get_u64(anchor, "line")? as usize,
            column: get_u64(anchor, "column")? as usize,
            excerpt: get_str(anchor, "excerpt")?.to_string(),
        },
    })
}

fn file_from(v: &serde_json::Value) -> std::result::Result<PlannedFile, String> {
    let edits = v
        .get("edits")
        .and_then(|e| e.as_array())
        .ok_or_else(|| "file missing edits array".to_string())?
        .iter()
        .map(edit_from)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(PlannedFile {
        path: path_from(v, "path")?,
        before: get_str(v, "before")?.to_string(),
        after: get_str(v, "after")?.to_string(),
        before_hash: get_u64(v, "before_hash")?,
        applied: false,
        edits,
    })
}

/// Load a versioned plan document. Schema mismatches and malformed
/// fields are hard errors — a plan is never silently repaired.
pub(crate) fn load(text: &str) -> std::result::Result<EditPlan, String> {
    let v: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("invalid plan JSON: {e}"))?;
    let schema = get_u64(&v, "schema")? as u32;
    if schema != PLAN_SCHEMA {
        return Err(format!("unsupported plan schema {schema}"));
    }
    let coverage = v
        .get("coverage")
        .ok_or_else(|| "plan missing coverage".to_string())?;
    let roots = coverage
        .get("roots")
        .and_then(|r| r.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let files_skipped = coverage
        .get("files_skipped")
        .and_then(|r| r.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    Some(SkippedFile {
                        path: PathBuf::from(x.get("path")?.as_str()?),
                        reason: x.get("reason")?.as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let unresolved = coverage
        .get("unresolved")
        .and_then(|r| r.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    Some(UnresolvedRef {
                        path: PathBuf::from(x.get("path")?.as_str()?),
                        detail: x.get("detail")?.as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let files = v
        .get("files")
        .and_then(|f| f.as_array())
        .ok_or_else(|| "plan missing files array".to_string())?
        .iter()
        .map(file_from)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let diagnostics = v
        .get("diagnostics")
        .and_then(|d| d.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    let severity = match x.get("severity")?.as_str()? {
                        "error" => Severity::Error,
                        "warning" => Severity::Warning,
                        _ => Severity::Info,
                    };
                    Some(Diagnostic {
                        severity,
                        code: x.get("code")?.as_str()?.to_string(),
                        message: x.get("message")?.as_str()?.to_string(),
                        path: x
                            .get("path")
                            .and_then(|p| p.as_str())
                            .map(PathBuf::from),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let read_set = coverage
        .get("read_set")
        .and_then(|r| r.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    Some(ReadSetEntry {
                        path: PathBuf::from(x.get("path")?.as_str()?),
                        hash: x.get("hash")?.as_u64()?,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let discovery = coverage.get("discovery").and_then(|d| {
        if d.is_null() {
            return None;
        }
        let file_set = d.get("file_set").and_then(|f| {
            if f.is_null() {
                return Some(None);
            }
            match f.get("kind")?.as_str()? {
                "extensions" => Some(Some(crate::discovery::FileSet::Extensions(
                    f.get("values")?
                        .as_array()?
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect(),
                ))),
                "glob" => Some(Some(crate::discovery::FileSet::Glob {
                    matches: f
                        .get("matches")?
                        .as_array()?
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect(),
                    case_insensitive: f
                        .get("case_insensitive")
                        .and_then(|c| c.as_bool())
                        .unwrap_or(false),
                })),
                _ => None,
            }
        })?;
        Some(DiscoverySpec {
            hidden: d.get("hidden").and_then(|h| h.as_bool()).unwrap_or(false),
            no_ignore: d
                .get("no_ignore")
                .and_then(|n| n.as_bool())
                .unwrap_or(false),
            file_set,
            seen_hash: d.get("seen_hash").and_then(|h| h.as_u64()).unwrap_or(0),
        })
    });
    Ok(EditPlan {
        schema,
        mode: get_str(&v, "mode")?.to_string(),
        applied: v.get("applied").and_then(|a| a.as_bool()).unwrap_or(false),
        operation_id: get_str(&v, "operation_id")?.to_string(),
        config: v
            .get("config")
            .and_then(|c| c.as_str())
            .map(str::to_string),
        coverage: Coverage {
            roots,
            files_scanned: coverage
                .get("files_scanned")
                .and_then(|n| n.as_u64())
                .unwrap_or(0) as usize,
            files_edited: files.len(),
            files_skipped,
            unresolved,
            read_set,
            discovery,
        },
        files,
        diagnostics,
    })
}
