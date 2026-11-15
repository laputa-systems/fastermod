use std::fmt;
use std::fs::read_to_string;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;

use grep::regex::RegexMatcher;
use grep::searcher::BinaryDetection;
use grep::searcher::Searcher;
use grep::searcher::SearcherBuilder;
use grep::searcher::Sink;
use grep::searcher::SinkMatch;

use crate::Error;

#[derive(Clone)]
pub(crate) struct SourceFile {
    pub path: PathBuf,
    pub contents: String,
}

pub(crate) fn display_warning(error: &Error) -> DisplayWarning<'_> {
    DisplayWarning { inner: error }
}

pub(crate) fn make_searcher() -> Searcher {
    SearcherBuilder::new()
        .line_number(false)
        .multi_line(true)
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .bom_sniffing(false)
        .build()
}

pub(crate) fn file_contents_if_matches(
    searcher: &mut Searcher,
    matcher: &RegexMatcher,
    path: &Path,
) -> Option<String> {
    let mut sink = FastermodSink::new();
    if let Err(e) = searcher.search_path(matcher, path, &mut sink) {
        eprintln!("{}", display_warning(&e.into()));
    };
    if sink.did_match {
        match read_to_string(path) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("{}", display_warning(&e.into()));
                None
            }
        }
    } else {
        None
    }
}

#[derive(Debug)]
pub(crate) struct DisplayWarning<'a> {
    pub(crate) inner: &'a Error,
}

impl<'a> fmt::Display for DisplayWarning<'a> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        writeln!(fmt, "Warning: {:?}", self.inner)?;
        Ok(())
    }
}

struct FastermodSink {
    did_match: bool,
}

impl FastermodSink {
    fn new() -> Self {
        Self { did_match: false }
    }
}


impl Sink for FastermodSink {
    type Error = std::io::Error;

    fn matched(
        &mut self,
        _searcher: &Searcher,
        _mat: &SinkMatch,
    ) -> std::result::Result<bool, std::io::Error> {
        self.did_match = true;
        Ok(false)
    }
}

pub(crate) fn remember_first_error(slot: &Mutex<Option<Error>>, error: Error) {
    let mut slot = slot.lock().unwrap();
    if slot.is_none() {
        *slot = Some(error);
    }
}

