use std::cmp::min;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;

use grep::regex::RegexMatcher;
use ignore::WalkBuilder;
use ignore::WalkState;
use ignore::overrides::OverrideBuilder;

use crate::source::{SourceFile, file_contents_if_matches, make_searcher, remember_first_error};
use crate::{Result, error_with_context};

pub(crate) fn available_worker_threads() -> usize {
    thread::available_parallelism().map_or(1, |parallelism| parallelism.get())
}

#[derive(Clone, Debug)]
pub(crate) enum FileSet {
    Extensions(Vec<String>),
    Glob {
        matches: Vec<String>,
        case_insensitive: bool,
    },
}


pub(crate) fn looks_like_code(path: &Path) -> bool {
    match path.file_name() {
        Some(name) => {
            let name = name.to_string_lossy();
            name != "tags" && name != "TAGS" && !name.ends_with('~')
        }
        None => true,
    }
}

pub(crate) fn walk_builder_with_file_set(dirs: Vec<&str>, file_set: Option<FileSet>) -> Result<WalkBuilder> {
    ensure!(!dirs.is_empty(), "must provide at least one path to walk!");
    let mut builder = WalkBuilder::new(dirs[0]);
    for dir in &dirs[1..] {
        builder.add(dir);
    }
    if let Some(file_set) = file_set {
        use self::FileSet::*;
        match file_set {
            Extensions(e) => {
                let mut override_builder = OverrideBuilder::new(".");
                for ext in e {
                    override_builder.add(&format!("*.{}", ext)).map_err(|e| {
                        error_with_context("Unable to register extension with directory walker", e)
                    })?;
                }
                builder.overrides(override_builder.build().map_err(|e| {
                    error_with_context("Unable to register extensions with directory walker", e)
                })?);
            }
            Glob {
                matches,
                case_insensitive,
            } => {
                let mut override_builder = OverrideBuilder::new(".");
                // Case sensitivity needs to be added before the patterns are.
                if case_insensitive {
                    override_builder
                        .case_insensitive(true)
                        .map_err(|e| error_with_context("Unable to toggle case sensitivity", e))?;
                }
                for file in matches {
                    override_builder.add(&file).map_err(|e| {
                        error_with_context("Unable to register glob with directory walker", e)
                    })?;
                }
                builder.overrides(override_builder.build().map_err(|e| {
                    error_with_context("Unable to register glob with directory walker", e)
                })?);
            }
        }
    }
    Ok(builder)
}

pub(crate) fn matching_files(
    dirs: Vec<&str>,
    file_set: Option<FileSet>,
    hidden: bool,
    no_ignore: bool,
    matcher: &RegexMatcher,
) -> Result<Vec<SourceFile>> {
    let walk = walk_builder_with_file_set(dirs, file_set)?
        .hidden(!hidden)
        .parents(!no_ignore)
        .ignore(!no_ignore)
        .git_ignore(!no_ignore)
        .git_global(!no_ignore)
        .git_exclude(!no_ignore)
        .threads(min(12, available_worker_threads()))
        .build_parallel();
    let matcher = matcher.clone();
    let files = Arc::new(Mutex::new(Vec::new()));
    let file_error = Arc::new(Mutex::new(None));
    let files_inner = files.clone();
    let file_error_inner = file_error.clone();
    walk.run(move || {
        let mut searcher = make_searcher();
        let matcher = matcher.clone();
        let files = files_inner.clone();
        let file_error = file_error_inner.clone();
        Box::new(move |result| {
            let dirent = match result {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("Warning: {}", e);
                    remember_first_error(&file_error, e.into());
                    return WalkState::Continue;
                }
            };
            if let Some(file_type) = dirent.file_type()
                && file_type.is_file()
            {
                let path = dirent.path();
                if looks_like_code(path) {
                    match file_contents_if_matches(&mut searcher, &matcher, path) {
                        Ok(Some(contents)) => files.lock().unwrap().push(SourceFile {
                            path: path.to_path_buf(),
                            contents,
                        }),
                        Ok(None) => {}
                        Err(error) => remember_first_error(&file_error, error),
                    }
                }
            }
            WalkState::Continue
        })
    });
    if let Some(error) = file_error.lock().unwrap().take() {
        return Err(error);
    }
    let mut files = files.lock().unwrap().clone();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}
/// Every regular file the walker visits under `dirs` before content
/// matching — the discovery read-set fingerprinted into edit plans so a
/// later apply can detect added, removed, or ignore-rule-changed files.
pub(crate) fn seen_files(
    dirs: Vec<&str>,
    file_set: Option<FileSet>,
    hidden: bool,
    no_ignore: bool,
) -> Result<Vec<std::path::PathBuf>> {
    let mut paths = Vec::new();
    for result in walk_builder_with_file_set(dirs, file_set)?
        .hidden(!hidden)
        .parents(!no_ignore)
        .ignore(!no_ignore)
        .git_ignore(!no_ignore)
        .git_global(!no_ignore)
        .git_exclude(!no_ignore)
        .build()
    {
        let dirent = result?;
        if dirent.file_type().is_some_and(|t| t.is_file())
            && looks_like_code(dirent.path())
        {
            paths.push(dirent.path().to_path_buf());
        }
    }
    paths.sort();
    Ok(paths)
}

/// Fingerprint a sorted seen-file listing: order-independent content hash
/// over path bytes so apply-time re-walks compare deterministically.
pub(crate) fn seen_fingerprint(paths: &[std::path::PathBuf]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    let mut mix = |bytes: &[u8]| {
        for b in bytes {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x100000001b3);
    };
    for p in paths {
        mix(p.as_os_str().as_encoded_bytes());
    }
    hash
}

/// Build the plan read-set: every analyzed file's content hash plus the
/// discovery fingerprint (walk params + seen-path hash) needed to detect
/// added/removed files at apply time.
pub(crate) fn fingerprint_read_set(
    files: &[SourceFile],
    dirs: Vec<&str>,
    file_set: Option<FileSet>,
    hidden: bool,
    no_ignore: bool,
) -> Result<crate::plan::ReadContext> {
    let seen = seen_files(dirs, file_set.clone(), hidden, no_ignore)?;
    Ok(crate::plan::ReadContext {
        entries: files
            .iter()
            .map(|f| crate::plan::ReadSetEntry {
                path: f.path.clone(),
                hash: crate::plan::content_hash(&f.contents),
            })
            .collect(),
        discovery: crate::plan::DiscoverySpec {
            hidden,
            no_ignore,
            file_set,
            seen_hash: seen_fingerprint(&seen),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_looks_like_code_uses_file_name() {
        for name in ["keep.rs", "ctags", "mytags", "file.tags", "TAGS.bak"] {
            assert!(
                looks_like_code(Path::new(name)),
                "{name} should be treated as code"
            );
        }
        for name in ["tags", "TAGS", "backup~"] {
            assert!(
                !looks_like_code(Path::new(name)),
                "{name} should be skipped"
            );
        }
        assert!(looks_like_code(Path::new("src/mytags")));
        assert!(!looks_like_code(Path::new("src/tags")));
    }
}
