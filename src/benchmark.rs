//! Benchmark support surface used by `benches/bench.rs`.
//! `#[doc(hidden)]` public API; not part of the CLI contract.

use std::path::Path;

use grep::regex::RegexMatcher;
use grep::regex::RegexMatcherBuilder;
use grep::searcher::Searcher;
use regex_lite::Regex;
use regex_lite::RegexBuilder;

use crate::discovery::FileSet;
use crate::review::Fastermod;
use crate::source::{display_warning, file_contents_if_matches, make_searcher};
use crate::text::Substitution;

pub type BenchmarkResult<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;



pub fn new_regex(pattern: &str) -> BenchmarkResult<Regex> {
    RegexBuilder::new(pattern)
        .multi_line(true)
        .build()
        .map_err(|error| error.into())
}

pub fn new_matcher(pattern: &str) -> BenchmarkResult<RegexMatcher> {
    RegexMatcherBuilder::new()
        .multi_line(true)
        .build(pattern)
        .map_err(|error| error.into())
}

pub fn new_searcher() -> Searcher {
    make_searcher()
}

pub fn search_file(
    searcher: &mut Searcher,
    matcher: &RegexMatcher,
    path: &Path,
) -> Option<usize> {
    match file_contents_if_matches(searcher, matcher, path) {
        Ok(contents) => contents.map(|contents| contents.len()),
        Err(error) => {
            eprintln!("{}", display_warning(&error));
            None
        }
    }
}

pub fn diff_line_count(original: &str, edited: &str) -> usize {
    Fastermod::new(false, false, false, false)
        .diffs_to_print(original, edited)
        .len()
}

pub fn replace_all_len(regex: &Regex, contents: &str, substitution: &str) -> usize {
    regex.replace_all(contents, substitution).len()
}

pub fn patch_file(
    regex: &Regex,
    substitution: &str,
    path: &Path,
    contents: &str,
) -> BenchmarkResult<bool> {
    Fastermod::new(true, false, false, false).fast_patch(
        regex,
        &Substitution::new(substitution.to_owned(), false),
        path,
        contents,
    )
}

pub fn run_fast(
    directory: &Path,
    regex: &Regex,
    matcher: &RegexMatcher,
    substitution: &str,
    extension: Option<&str>,
) -> BenchmarkResult<()> {
    let directory = directory
        .to_str()
        .ok_or_else(|| "benchmark directory path is not UTF-8".to_owned())?;
    let file_set = extension.map(|extension| FileSet::Extensions(vec![extension.to_owned()]));
    Fastermod::run_fast(
        regex,
        matcher,
        &Substitution::new(substitution.to_owned(), false),
        vec![directory],
        file_set,
        false,
        false,
        false,
    )
}
