/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use std::borrow::Cow;
use std::cmp::max;
use std::cmp::min;
use std::collections::HashSet;
use std::env;
use std::fmt;
use std::fs;
use std::fs::read_to_string;
use std::io::IsTerminal;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::exit;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::mpsc::sync_channel;
use std::thread;

use grep::regex::RegexMatcher;
use grep::regex::RegexMatcherBuilder;
use grep::searcher::BinaryDetection;
use grep::searcher::Searcher;
use grep::searcher::SearcherBuilder;
use grep::searcher::Sink;
use grep::searcher::SinkMatch;
use ignore::WalkBuilder;
use ignore::WalkState;
use ignore::overrides::OverrideBuilder;
use imara_histogram_diff::Diff as HistogramDiff;
use imara_histogram_diff::diff as histogram_diff;
use lexopt::prelude::*;
use regex_lite::Regex;
use regex_lite::RegexBuilder;

mod terminal;

use crate::terminal::Color;

type Error = Box<dyn std::error::Error + Send + Sync>;
type Result<T> = ::std::result::Result<T, Error>;

/// Process exit carrying the frozen exit-code contract (docs/refactor/cli.md).
#[derive(Debug)]
struct Failure {
    code: i32,
    message: String,
}

impl Failure {
    fn usage(message: impl Into<String>) -> Failure {
        Failure {
            code: 2,
            message: message.into(),
        }
    }

    fn analysis(message: impl Into<String>) -> Failure {
        Failure {
            code: 3,
            message: message.into(),
        }
    }

    fn io(message: impl Into<String>) -> Failure {
        Failure {
            code: 4,
            message: message.into(),
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

fn operational<T>(result: Result<T>) -> std::result::Result<T, Failure> {
    result.map_err(|e| Failure::io(e.to_string()))
}

fn error_with_context(msg: impl std::fmt::Display, e: impl std::fmt::Display) -> Error {
    format!("{msg}: {e}").into()
}

fn available_worker_threads() -> usize {
    thread::available_parallelism().map_or(1, |parallelism| parallelism.get())
}

macro_rules! ensure {
    ($cond:expr, $msg:expr $(,)?) => {
        if !$cond {
            return Err($msg.into());
        }
    };
}

#[derive(Clone)]
enum FileSet {
    Extensions(Vec<String>),
    Glob {
        matches: Vec<String>,
        case_insensitive: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
enum DiffResult<'a> {
    Left(&'a str),
    Both(&'a str, &'a str),
    Right(&'a str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Text,
    Ident,
    Rename,
    Rewrite,
    Rules,
    ApplyPlan,
    Languages,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Text => "text",
            Mode::Ident => "ident",
            Mode::Rename => "rename",
            Mode::Rewrite => "rewrite",
            Mode::Rules => "rules",
            Mode::ApplyPlan => "apply-plan",
            Mode::Languages => "languages",
        }
    }

    fn is_semantic(self) -> bool {
        !matches!(self, Mode::Text)
    }
}

struct Args {
    mode: Mode,
    multiline: bool,
    dir: Vec<String>,
    file_or_dir: Vec<String>,
    ignore_case: bool,
    smart_case: bool,
    extensions: Option<Vec<String>>,
    glob: Option<Vec<String>>,
    hidden: bool,
    no_ignore: bool,
    iglob: Option<Vec<String>>,
    accept_all: bool,
    apply: bool,
    dry_run: bool,
    check: bool,
    json: bool,
    plan: Option<String>,
    apply_plan: Option<String>,
    rules_file: Option<String>,
    at_anchor: Option<String>,
    langs: Vec<String>,
    print_changed_files: bool,
    fixed_strings: bool,
    match_regex: String,
    subst: String,
}

#[derive(Clone)]
struct Substitution {
    template: String,
    smart_case: bool,
}

impl Substitution {
    fn new(template: String, smart_case: bool) -> Self {
        Self {
            template,
            smart_case,
        }
    }

    fn replacement(&self, captures: &regex_lite::Captures<'_>) -> String {
        let mut replacement = String::new();
        captures.expand(&self.template, &mut replacement);
        if self.smart_case {
            adapt_case(captures.get(0).unwrap().as_str(), &replacement)
        } else {
            replacement
        }
    }

    fn replace_all<'a>(&self, regex: &Regex, contents: &'a str) -> Cow<'a, str> {
        if self.smart_case {
            regex.replace_all(contents, |captures: &regex_lite::Captures<'_>| {
                self.replacement(captures)
            })
        } else {
            regex.replace_all(contents, self.template.as_str())
        }
    }
}

fn adapt_case(matched: &str, replacement: &str) -> String {
    let mut letters = matched
        .chars()
        .filter(|ch| ch.is_lowercase() || ch.is_uppercase());
    let Some(first_letter) = letters.next() else {
        return replacement.to_owned();
    };
    if first_letter.is_uppercase() && letters.clone().all(char::is_uppercase) {
        return replacement.chars().flat_map(char::to_uppercase).collect();
    }
    if first_letter.is_lowercase() && letters.clone().all(char::is_lowercase) {
        return replacement.to_lowercase();
    }
    if first_letter.is_uppercase() && letters.all(char::is_lowercase) {
        let mut first_letter = true;
        let mut title_case = String::new();
        for ch in replacement.chars() {
            if ch.is_uppercase() || ch.is_lowercase() {
                if first_letter {
                    title_case.extend(ch.to_uppercase());
                    first_letter = false;
                } else {
                    title_case.extend(ch.to_lowercase());
                }
            } else {
                title_case.push(ch);
            }
        }
        return title_case;
    }
    replacement.to_owned()
}

fn help_text() -> &'static str {
    "fastermod is a fast partial replacement for codemod.

A tool to assist you with large-scale codebase refactors that can be
partially automated but still require human oversight and occasional
intervention.

Usage: fastermod [OPTIONS] <OLD> <NEW> [PATH...]
       fastermod --rules <RULES_FILE> [PATH...]
       fastermod --apply-plan <PLAN_FILE>
       fastermod --languages [--json]

Modes (mutually exclusive):
      --ident               Edit every matching identifier token (syntactic,
                           not binding-aware)
      --rename              Rename one resolved binding and its references
      --rewrite             Structural pattern rewrite (see --lang)
      --rules <FILE>        Declarative migration rules (data only)
      --apply-plan <FILE>   Apply a saved, versioned plan
      --languages           Print the capability matrix

Arguments:
  <OLD>            Pattern to match: regex in text mode, identifier name in
                   --ident/--rename, AST pattern in --rewrite.
  <NEW>            Replacement: substitution template, new name, or AST
                   replacement.
  <PATH>...        Paths whose descendent files are to be explored.

Options:
  -m, --multiline            Have regex work over multiple lines (i.e., have dot match newlines)
  -d, --dir <DIR>            The path whose descendent files are to be explored
  -i, --ignore-case          Perform case-insensitive search
  -S, --smartcase            Match without regard to case when OLD has no uppercase letters. Adapt NEW casing.
  -e, --extensions <EXT>     A comma-delimited list of file extensions to process
  -g, --glob <GLOB>          A space-delimited list of globs to process
      --hidden              Search hidden files
  -u, --no-ignore           Also search ignored files
      --iglob <IGLOB>       A space-delimited list of case-insensitive globs to process
      --at <ANCHOR>         Select the rename target: FILE:LINE:COL (1-based line,
                           1-based UTF-8 byte column). --rename only.
      --lang <LANG>         Force a language/grammar (repeatable). --rewrite/--rules.
      --apply               Write accepted changes without review
      --accept-all          Same as --apply (legacy spelling)
      --dry-run             Preview changes; never write
      --check               Exit 1 when changes are pending; never write
      --json                Machine-readable result on stdout
      --plan <FILE>         Preview and save a versioned plan (no simultaneous apply)
      --print-changed-files Print the paths of changed files
  -F, --fixed-strings        Treat OLD as a literal string. Avoids the need to escape regex metacharacters
  -h, --help                Print help
  -V, --version             Print version

Without --apply/--accept-all on a non-terminal stdin, fastermod previews
changes and exits without writing. See docs/refactor/cli.md for the frozen
mode and exit-code contract."
}

fn print_help_and_exit() {
    print!("fastermod {}\n{}\n", env!("CARGO_PKG_VERSION"), help_text());
    exit(0);
}

fn parse_args() -> Args {
    match parse_args_inner() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("error: {}", e);
            exit(2);
        }
    }
}

fn set_mode(slot: &mut Option<Mode>, mode: Mode) -> Result<()> {
    if slot.replace(mode).is_some() {
        return Err("conflicting mode flags: only one of --ident, --rename, --rewrite, --rules, --apply-plan, --languages may be used".into());
    }
    Ok(())
}

fn parse_args_inner() -> Result<Args> {
    let mut mode_slot = None;
    let mut multiline = false;
    let mut dir = Vec::new();
    let mut ignore_case = false;
    let mut smart_case = false;
    let mut extensions = None;
    let mut has_glob = false;
    let mut glob = Vec::new();
    let mut hidden = false;
    let mut no_ignore = false;
    let mut has_iglob = false;
    let mut iglob = Vec::new();
    let mut accept_all = false;
    let mut apply = false;
    let mut dry_run = false;
    let mut check = false;
    let mut json = false;
    let mut plan = None;
    let mut apply_plan = None;
    let mut rules_file = None;
    let mut at_anchor = None;
    let mut langs = Vec::new();
    let mut print_changed_files = false;
    let mut fixed_strings = false;
    let mut positionals = Vec::new();

    let mut parser = lexopt::Parser::from_env();
    while let Some(arg) = parser.next()? {
        match arg {
            Short('h') | Long("help") => print_help_and_exit(),
            Short('V') | Long("version") => {
                println!("fastermod {}", env!("CARGO_PKG_VERSION"));
                exit(0);
            }
            Long("ident") => set_mode(&mut mode_slot, Mode::Ident)?,
            Long("rename") => set_mode(&mut mode_slot, Mode::Rename)?,
            Long("rewrite") => set_mode(&mut mode_slot, Mode::Rewrite)?,
            Long("rules") => {
                set_mode(&mut mode_slot, Mode::Rules)?;
                rules_file = Some(parser.value()?.to_string_lossy().into_owned());
            }
            Long("apply-plan") => {
                set_mode(&mut mode_slot, Mode::ApplyPlan)?;
                apply_plan = Some(parser.value()?.to_string_lossy().into_owned());
            }
            Long("languages") => set_mode(&mut mode_slot, Mode::Languages)?,
            Long("at") => at_anchor = Some(parser.value()?.to_string_lossy().into_owned()),
            Long("lang") => langs.push(parser.value()?.to_string_lossy().into_owned()),
            Long("apply") => apply = true,
            Long("dry-run") => dry_run = true,
            Long("check") => check = true,
            Long("json") => json = true,
            Long("plan") => plan = Some(parser.value()?.to_string_lossy().into_owned()),
            Short('m') | Long("multiline") => multiline = true,
            Short('d') | Long("dir") => dir.push(parser.value()?.to_string_lossy().into_owned()),
            Short('i') | Long("ignore-case") => ignore_case = true,
            Short('S') | Long("smartcase") => smart_case = true,
            Short('e') | Long("extensions") => {
                let value = parser.value()?.to_string_lossy().into_owned();
                let mut exts: Vec<String> = value.split(',').map(str::to_string).collect();
                extensions.get_or_insert_with(Vec::new).append(&mut exts);
            }
            Short('g') | Long("glob") => {
                for value in parser.values()? {
                    has_glob = true;
                    glob.push(value.to_string_lossy().into_owned());
                }
            }
            Long("hidden") => hidden = true,
            Short('u') | Long("no-ignore") => no_ignore = true,
            Long("iglob") => {
                for value in parser.values()? {
                    has_iglob = true;
                    iglob.push(value.to_string_lossy().into_owned());
                }
            }
            Long("accept-all") => accept_all = true,
            Long("print-changed-files") => print_changed_files = true,
            Short('F') | Long("fixed-strings") => fixed_strings = true,
            Value(value) => positionals.push(value.to_string_lossy().into_owned()),
            _ => return Err(format!("Unrecognized argument: {:?}", arg).into()),
        }
    }
    let mode = mode_slot.unwrap_or(Mode::Text);
    let mut positionals = positionals.into_iter();
    let (match_regex, subst) = match mode {
        Mode::Text | Mode::Ident | Mode::Rename | Mode::Rewrite => {
            let old = positionals.next().ok_or(
                "Missing required argument <OLD>.\nUsage: fastermod [OPTIONS] <OLD> <NEW> [PATH...]",
            )?;
            let new = positionals.next().ok_or(
                "Missing required argument <NEW>.\nUsage: fastermod [OPTIONS] <OLD> <NEW> [PATH...]",
            )?;
            (old, new)
        }
        Mode::Rules | Mode::ApplyPlan | Mode::Languages => (String::new(), String::new()),
    };
    let file_or_dir: Vec<String> = positionals.collect();
    ensure!(
        extensions.is_none() || (glob.is_empty() && iglob.is_empty()),
        "cannot use --extensions with --glob or --iglob"
    );
    ensure!(
        has_glob != has_iglob || glob.is_empty(),
        "cannot use --glob and --iglob together"
    );
    let glob = if has_glob { Some(glob) } else { None };
    let iglob = if has_iglob { Some(iglob) } else { None };

    ensure!(
        !(mode.is_semantic() && (multiline || ignore_case || smart_case || fixed_strings)),
        format!("regex options (-m, -i, -S, -F) are not meaningful in --{} mode", mode.name())
    );
    ensure!(
        at_anchor.is_none() || mode == Mode::Rename,
        "--at is only valid with --rename"
    );
    ensure!(
        langs.is_empty() || matches!(mode, Mode::Rewrite | Mode::Rules),
        "--lang is only valid with --rewrite or --rules"
    );
    ensure!(
        plan.is_none() || !matches!(mode, Mode::ApplyPlan | Mode::Languages),
        "--plan cannot be combined with --apply-plan or --languages"
    );
    ensure!(
        !(apply || accept_all) || plan.is_none(),
        "--plan cannot be combined with --apply or --accept-all"
    );
    ensure!(
        !check || !(apply || accept_all || plan.is_some()),
        "--check cannot be combined with --apply, --accept-all, or --plan"
    );
    ensure!(
        !(apply || accept_all) || !dry_run,
        "--apply/--accept-all cannot be combined with --dry-run"
    );
    ensure!(
        !(dry_run || check || plan.is_some())
            || matches!(mode, Mode::Text | Mode::Ident | Mode::Rename | Mode::Rewrite | Mode::Rules),
        "--dry-run, --check and --plan require a content mode"
    );
    ensure!(
        !json || !matches!(mode, Mode::ApplyPlan),
        "--json cannot be combined with --apply-plan"
    );
    ensure!(
        !matches!(mode, Mode::ApplyPlan | Mode::Languages) || file_or_dir.is_empty(),
        format!("--{} takes no path arguments", mode.name())
    );
    ensure!(
        rules_file.is_some() || mode != Mode::Rules,
        "--rules requires a rules file"
    );
    Ok(Args {
        mode,
        multiline,
        dir,
        file_or_dir,
        ignore_case,
        smart_case,
        extensions,
        glob,
        hidden,
        no_ignore,
        iglob,
        accept_all,
        apply,
        dry_run,
        check,
        json,
        plan,
        apply_plan,
        rules_file,
        at_anchor,
        langs,
        print_changed_files,
        fixed_strings,
        match_regex,
        subst,
    })
}

fn notify_fast_mode() {
    eprintln!("Fast mode activated. Sit back, relax, and enjoy the brief flight.");
}

fn run_editor(path: &Path, start_line: usize) -> Result<()> {
    let editor = env::var("EDITOR").unwrap_or_else(|_| String::from("vim"));
    let args: Vec<&str> = editor.split(' ').collect();
    let mut editor_cmd = Command::new(args[0])
        .args(&args[1..])
        .arg(format!("+{}", start_line))
        .arg(path)
        .spawn()
        .map_err(|e| {
            error_with_context(
                format!("Unable to launch editor {} on path {:?}", editor, path),
                e,
            )
        })?;
    editor_cmd
        .wait()
        .map_err(|e| error_with_context("Error waiting for editor to exit", e))?;
    Ok(())
}

fn looks_like_code(path: &Path) -> bool {
    match path.file_name() {
        Some(name) => {
            let name = name.to_string_lossy();
            name != "tags" && name != "TAGS" && !name.ends_with('~')
        }
        None => true,
    }
}

fn read_reply() -> std::io::Result<String> {
    let mut reply = String::new();
    std::io::stdin().read_line(&mut reply)?;
    if !reply.ends_with('\n') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "unexpected end of file",
        ));
    }
    reply.pop();
    if reply.ends_with('\r') {
        reply.pop();
    }
    Ok(reply)
}

fn prompt_reply_stdout(prompt: &str) -> std::io::Result<String> {
    let mut stdout = std::io::stdout();
    write!(stdout, "{}", prompt)?;
    stdout.flush()?;
    read_reply()
}

fn prompt_reply_stderr(prompt: &str) -> std::io::Result<String> {
    let mut stderr = std::io::stderr();
    write!(stderr, "{}", prompt)?;
    stderr.flush()?;
    read_reply()
}

fn prompt(prompt_text: &str, letters: &str, default: Option<char>) -> Result<char> {
    loop {
        let input = prompt_reply_stdout(prompt_text)
            .map_err(|e| error_with_context("Unable to read user input", e))?;
        if input.is_empty()
            && let Some(default) = default
        {
            return Ok(default);
        }
        if input.len() == 1 && letters.contains(&input) {
            return Ok(input.chars().next().unwrap());
        }
        println!("Come again?")
    }
}

fn walk_builder_with_file_set(dirs: Vec<&str>, file_set: Option<FileSet>) -> Result<WalkBuilder> {
    ensure!(!dirs.is_empty(), "must provide at least one path to walk!");
    let mut builder = WalkBuilder::new(dirs[0]);
    for dir in &dirs[1..] {
        builder.add(dir);
    }
    if let Some(file_set) = file_set {
        use crate::FileSet::*;
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

fn index_to_row_col(s: &str, index: usize) -> (usize, usize) {
    let chunk = &s[..index];
    let line_num = chunk.chars().filter(|x| x == &'\n').count();
    let last_newline = if let Some(result) = chunk.rfind('\n') {
        result as isize
    } else {
        -1
    };
    let col = index as isize - last_newline - 1;
    (line_num, col as usize)
}

fn display_warning(error: &Error) -> DisplayWarning<'_> {
    DisplayWarning { inner: error }
}

fn make_searcher() -> Searcher {
    SearcherBuilder::new()
        .line_number(false)
        .multi_line(true)
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .bom_sniffing(false)
        .build()
}

fn file_contents_if_matches(
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
struct DisplayWarning<'a> {
    inner: &'a Error,
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

struct Fastermod {
    yes_to_all: bool,
    hidden: bool,
    no_ignore: bool,
    changed_files: Option<Vec<PathBuf>>,
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

fn resume_after(s: &str, index: usize, zero_length: bool) -> usize {
    let index = index.min(s.len());
    if zero_length {
        one_char_past(s, index)
    } else {
        index
    }
}

fn one_char_past(s: &str, index: usize) -> usize {
    if index >= s.len() {
        s.len()
    } else {
        index + s[index..].chars().next().unwrap().len_utf8()
    }
}

fn backward_to_char_boundary(s: &str, mut index: usize) -> usize {
    while !s.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn without_line_ending(line: &str) -> &str {
    if let Some(line) = line.strip_suffix('\n') {
        line.strip_suffix('\r').unwrap_or(line)
    } else {
        line
    }
}

fn compute_diff<'a>(orig: &'a str, edit: &'a str) -> HistogramDiff<'a> {
    histogram_diff(orig, edit)
}

fn diff_output_shape(diff: &HistogramDiff<'_>) -> (usize, usize, usize) {
    let mut output_len = 0;
    let mut before_position = 0;
    let mut after_position = 0;
    let mut prefix_len = None;

    for hunk in diff.hunks() {
        let before_start = hunk.before.start;
        let before_end = hunk.before.end;
        let after_start = hunk.after.start;
        let after_end = hunk.after.end;

        prefix_len.get_or_insert(before_start);
        output_len += before_start - before_position;
        output_len += before_end - before_start;
        output_len += after_end - after_start;
        before_position = before_end;
        after_position = after_end;
    }

    let suffix_len = (diff.before_len() - before_position).min(diff.after_len() - after_position);
    output_len += suffix_len;
    let prefix_len = prefix_len.unwrap_or(output_len);
    (output_len, prefix_len, suffix_len)
}

fn visit_diff_lines<'a>(diff: &HistogramDiff<'a>, mut visit: impl FnMut(DiffResult<'a>) -> bool) {
    let mut before_position = 0;
    let mut after_position = 0;
    for hunk in diff.hunks() {
        let before_start = hunk.before.start;
        let before_end = hunk.before.end;
        let after_start = hunk.after.start;
        let after_end = hunk.after.end;

        debug_assert_eq!(before_start - before_position, after_start - after_position);
        for token in before_position..before_start {
            let before_line = without_line_ending(diff.before_line(token));
            let after_line =
                without_line_ending(diff.after_line(after_position + token - before_position));
            if !visit(DiffResult::Both(before_line, after_line)) {
                return;
            }
        }
        for token in before_start..before_end {
            if !visit(DiffResult::Left(without_line_ending(
                diff.before_line(token),
            ))) {
                return;
            }
        }
        for token in after_start..after_end {
            if !visit(DiffResult::Right(without_line_ending(
                diff.after_line(token),
            ))) {
                return;
            }
        }

        before_position = before_end;
        after_position = after_end;
    }
    for (before_token, after_token) in
        (before_position..diff.before_len()).zip(after_position..diff.after_len())
    {
        if !visit(DiffResult::Both(
            without_line_ending(diff.before_line(before_token)),
            without_line_ending(diff.after_line(after_token)),
        )) {
            return;
        }
    }
}

fn diff_lines_in_range<'a>(
    diff: &HistogramDiff<'a>,
    start: usize,
    end: usize,
) -> Vec<DiffResult<'a>> {
    let mut result = Vec::with_capacity(end - start);
    let mut output_position = 0;
    visit_diff_lines(diff, |line| {
        if output_position >= start {
            result.push(line);
        }
        output_position += 1;
        output_position < end
    });
    result
}

impl Fastermod {
    fn new(
        accept_all: bool,
        hidden: bool,
        no_ignore: bool,
        print_changed_files: bool,
    ) -> Fastermod {
        Fastermod {
            yes_to_all: accept_all,
            hidden,
            no_ignore,
            changed_files: if print_changed_files {
                Some(Vec::new())
            } else {
                None
            },
        }
    }

    fn save(&mut self, path: &Path, text: &str) -> Result<()> {
        if let Ok(metadata) = fs::metadata(path)
            && metadata.permissions().readonly()
        {
            return Err(error_with_context(
                format!("Unable to write to {:?}", path),
                std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "permission denied",
                ),
            ));
        }
        static SAVE_COUNTER: AtomicU64 = AtomicU64::new(0);
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| String::from("fastermod"));
        for _ in 0..100 {
            let temp = parent.join(format!(
                ".{}.tmp.{}-{}",
                file_name,
                std::process::id(),
                SAVE_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let staged = (|| -> Result<Option<()>> {
                let mut file = match fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&temp)
                {
                    Ok(file) => file,
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        return Ok(None);
                    }
                    Err(e) => {
                        return Err(error_with_context(
                            format!("Unable to write to {:?}", path),
                            e,
                        ));
                    }
                };
                if let Ok(metadata) = fs::metadata(path) {
                    file.set_permissions(metadata.permissions()).map_err(|e| {
                        error_with_context(format!("Unable to write to {:?}", path), e)
                    })?;
                }
                std::io::Write::write_all(&mut file, text.as_bytes()).map_err(|e| {
                    error_with_context(format!("Unable to write to {:?}", path), e)
                })?;
                file.sync_all().map_err(|e| {
                    error_with_context(format!("Unable to write to {:?}", path), e)
                })?;
                drop(file);
                fs::rename(&temp, path).map_err(|e| {
                    error_with_context(format!("Unable to write to {:?}", path), e)
                })?;
                #[cfg(unix)]
                if let Ok(dir) = fs::File::open(parent) {
                    let _ = dir.sync_all();
                }
                Ok(Some(()))
            })();
            match staged {
                Ok(Some(())) => {
                    self.record_change(path.to_owned());
                    return Ok(());
                }
                Ok(None) => continue,
                Err(error) => {
                    let _ = fs::remove_file(&temp);
                    return Err(error);
                }
            }
        }
        Err(error_with_context(
            format!("Unable to write to {:?}", path),
            "unable to stage a temporary file",
        ))
    }

    fn record_change(&mut self, path: PathBuf) {
        if let Some(ref mut changed_files) = self.changed_files {
            changed_files.push(path);
        }
    }

    fn print_changed_files_if_needed(&mut self) {
        if let Some(ref mut changed_files) = self.changed_files {
            changed_files.sort();
            for file in changed_files {
                println!("{}", file.display());
            }
        }
    }

    fn fast_patch(
        &mut self,
        regex: &Regex,
        substitution: &Substitution,
        path: &Path,
        contents: &str,
    ) -> Result<bool> {
        let new_contents = substitution.replace_all(regex, contents);
        match new_contents {
            Cow::Borrowed(_) => Ok(false),
            Cow::Owned(_) => {
                self.save(path, &new_contents)?;
                Ok(true)
            }
        }
    }

    fn present_and_apply_patches(
        &mut self,
        regex: &Regex,
        substitution: &Substitution,
        path: &Path,
        mut contents: String,
    ) -> Result<()> {
        let mut offset = 0;
        while offset < contents.len() {
            let (accepted, applied_end, match_end, is_zero_length_match, previous, proposed) = {
                let captures = match regex.captures(&contents[offset..]) {
                    None => break,
                    Some(captures) => captures,
                };
                let mat = captures.get(0).unwrap();
                let replacement = substitution.replacement(&captures);
                let mut new_contents = contents[..offset].to_string();
                new_contents.push_str(&contents[offset..offset + mat.start()]);
                new_contents.push_str(&replacement);
                new_contents.push_str(&contents[offset + mat.end()..]);
                let is_zero_length_match = mat.end() == mat.start();
                let (start_line, _) = index_to_row_col(&contents, mat.start() + offset);
                let (end_line, _) = index_to_row_col(
                    &contents,
                    backward_to_char_boundary(
                        &contents,
                        mat.end() + offset - if is_zero_length_match { 0 } else { 1 },
                    ),
                );
                let accepted = self.ask_about_patch(
                    path,
                    &contents,
                    start_line + 1,
                    end_line + 1,
                    &new_contents,
                )?;
                (
                    accepted,
                    offset + mat.start() + replacement.len(),
                    offset + mat.end(),
                    is_zero_length_match,
                    contents.clone(),
                    new_contents,
                )
            };
            let written = read_to_string(path)?;
            offset = if accepted && written == proposed {
                resume_after(&written, applied_end, is_zero_length_match)
            } else if accepted && written != previous {
                0
            } else {
                resume_after(&written, match_end, is_zero_length_match)
            };
            contents = written;
        }
        Ok(())
    }

    fn ask_about_patch<'a>(
        &mut self,
        path: &Path,
        old: &'a str,
        start_line: usize,
        end_line: usize,
        new: &'a str,
    ) -> Result<bool> {
        terminal::clear();

        let diffs = self.diffs_to_print(old, new);
        if diffs.is_empty() {
            return Ok(false);
        }

        if start_line == end_line {
            println!("{}:{}", path.to_string_lossy(), start_line);
        } else {
            println!("{}:{}-{}", path.to_string_lossy(), start_line, end_line);
        }

        self.print_diff(&diffs);
        let mut user_input = if self.yes_to_all {
            'y'
        } else {
            prompt(
                "Accept change (y = yes [default], \
                 n = no, e = edit, A = yes to all, E = yes+edit, q = quit)?\n",
                "yneAEq",
                Some('y'),
            )?
        };
        if user_input == 'A' {
            self.yes_to_all = true;
            user_input = 'y';
        }
        match user_input {
            'y' => {
                self.save(path, new)?;
                Ok(true)
            }
            'E' => {
                self.save(path, new)?;
                run_editor(path, start_line)?;
                Ok(true)
            }
            'e' => {
                self.record_change(path.to_owned());
                run_editor(path, start_line)?;
                Ok(true)
            }
            'q' => exit(0),
            'n' => Ok(false),
            _ => unreachable!(),
        }
    }

    fn diffs_to_print<'a>(&self, orig: &'a str, edit: &'a str) -> Vec<DiffResult<'a>> {
        let diff = compute_diff(orig, edit);
        let lines_to_print = match terminal::size() {
            Some((_w, h)) => h,
            None => 25,
        } - 20;

        let (diff_len, num_prefix_lines, num_suffix_lines) = diff_output_shape(&diff);

        if diff_len == num_prefix_lines {
            return vec![];
        }

        let size_of_diff = diff_len - num_prefix_lines - num_suffix_lines;
        let size_of_context = lines_to_print.saturating_sub(size_of_diff);
        let size_of_up_context = size_of_context / 2;
        let size_of_down_context = size_of_context / 2 + size_of_context % 2;

        let start_offset = num_prefix_lines.saturating_sub(size_of_up_context);
        let end_offset = min(
            diff_len,
            num_prefix_lines + size_of_diff + size_of_down_context,
        );
        let diffs = diff_lines_in_range(&diff, start_offset, end_offset);

        assert!(
            diffs.len() <= max(lines_to_print, size_of_diff),
            "changeset too long: {} > max({}, {})",
            diffs.len(),
            lines_to_print,
            size_of_diff
        );

        diffs
    }

    fn print_diff<'a>(&mut self, diffs: &[DiffResult<'a>]) {
        let color = std::io::stdout().is_terminal();
        for diff in diffs {
            match diff {
                DiffResult::Left(l) => {
                    if color {
                        terminal::fg(Color::Red);
                    }
                    println!("- {}", l);
                    if color {
                        terminal::reset();
                    }
                }
                DiffResult::Both(l, _) => println!("  {}", l),
                DiffResult::Right(r) => {
                    if color {
                        terminal::fg(Color::Green);
                    }
                    println!("+ {}", r);
                    if color {
                        terminal::reset();
                    }
                }
            }
        }
    }

    fn run_interactive(
        &mut self,
        regex: &Regex,
        matcher: &RegexMatcher,
        substitution: &Substitution,
        dirs: Vec<&str>,
        file_set: Option<FileSet>,
    ) -> Result<()> {
        let walk = walk_builder_with_file_set(dirs.clone(), file_set.clone())?
            .hidden(!self.hidden)
            .parents(!self.no_ignore)
            .ignore(!self.no_ignore)
            .git_ignore(!self.no_ignore)
            .git_global(!self.no_ignore)
            .git_exclude(!self.no_ignore)
            .threads(min(12, available_worker_threads()))
            .build_parallel();
        const QUEUE: usize = 16;
        let (tx, rx) = sync_channel(QUEUE);
        let thread_matcher = matcher.clone();
        thread::spawn(|| {
            walk.run(move || {
                let mut searcher = make_searcher();
                let tx = tx.clone();
                let matcher = thread_matcher.clone();
                Box::new(move |result| {
                    let dirent = match result {
                        Ok(d) => d,
                        Err(e) => {
                            eprintln!("Warning: {}", e);
                            return WalkState::Continue;
                        }
                    };
                    if let Some(file_type) = dirent.file_type() {
                        if !file_type.is_file() {
                            return WalkState::Continue;
                        }
                        let path = dirent.path();
                        if !looks_like_code(path) {
                            return WalkState::Continue;
                        }
                        if let Some(contents) =
                            file_contents_if_matches(&mut searcher, &matcher, path)
                            && tx.send((path.to_path_buf(), contents)).is_err()
                        {
                            return WalkState::Quit;
                        }
                    }
                    WalkState::Continue
                })
            })
        });

        let mut visited = HashSet::default();
        while let Ok((path, contents)) = rx.recv() {
            visited.insert(path.clone());
            self.present_and_apply_patches(regex, substitution, &path, contents)?;
            if self.yes_to_all {
                break;
            }
        }
        drop(rx);
        if self.yes_to_all {
            terminal::clear();
            notify_fast_mode();
            return Fastermod::run_fast_impl(
                regex,
                matcher,
                substitution,
                dirs,
                file_set,
                self.hidden,
                self.no_ignore,
                self.changed_files.clone(),
                Some(visited),
            );
        }
        self.print_changed_files_if_needed();
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn run_fast(
        regex: &Regex,
        matcher: &RegexMatcher,
        substitution: &Substitution,
        dirs: Vec<&str>,
        file_set: Option<FileSet>,
        hidden: bool,
        no_ignore: bool,
        print_changed_files: bool,
    ) -> Result<()> {
        Fastermod::run_fast_impl(
            regex,
            matcher,
            substitution,
            dirs,
            file_set,
            hidden,
            no_ignore,
            if print_changed_files {
                Some(Vec::new())
            } else {
                None
            },
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn run_fast_impl(
        regex: &Regex,
        matcher: &RegexMatcher,
        substitution: &Substitution,
        dirs: Vec<&str>,
        file_set: Option<FileSet>,
        hidden: bool,
        no_ignore: bool,
        changed_files: Option<Vec<PathBuf>>,
        visited: Option<HashSet<PathBuf>>,
    ) -> Result<()> {
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
        let visited = Arc::new(visited);
        let substitution = Arc::new(substitution.clone());
        let should_record_changed_files = changed_files.is_some();
        let changed_files = Arc::new(Mutex::new(changed_files.unwrap_or_default()));
        let changed_files_inner = changed_files.clone();
        let file_error = Arc::new(Mutex::new(None));
        let file_error_inner = file_error.clone();
        walk.run(move || {
            let mut fm = Fastermod::new(true, hidden, no_ignore, false);
            let regex = regex.clone();
            let matcher = matcher.clone();
            let substitution = substitution.clone();
            let visited = visited.clone();
            let changed_files = changed_files_inner.clone();
            let file_error = file_error_inner.clone();
            let mut searcher = make_searcher();
            Box::new(move |result| {
                let dirent = match result {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!("Warning: {}", e);
                        remember_first_error(&file_error, e.into());
                        return WalkState::Continue;
                    }
                };
                if let Some(file_type) = dirent.file_type() {
                    if !file_type.is_file() {
                        return WalkState::Continue;
                    }
                    let path = dirent.path();
                    if let Some(ref visited) = *visited
                        && visited.contains(path)
                    {
                        return WalkState::Continue;
                    }
                    if !looks_like_code(path) {
                        return WalkState::Continue;
                    }
                    if let Some(contents) = file_contents_if_matches(&mut searcher, &matcher, path)
                    {
                        let patching_result =
                            fm.fast_patch(&regex, substitution.as_ref(), path, &contents);
                        match patching_result {
                            Ok(changed_file) => {
                                if should_record_changed_files && changed_file {
                                    let mut changed_files = changed_files.lock().unwrap();
                                    changed_files.push(path.to_owned())
                                }
                            }
                            Err(error) => {
                                eprintln!("{}", display_warning(&error));
                                remember_first_error(&file_error, error);
                            }
                        }
                    }
                }
                WalkState::Continue
            })
        });
        if should_record_changed_files {
            let mut changed_files = changed_files.lock().unwrap();
            (*changed_files).sort();
            for file in &*changed_files {
                println!("{}", file.display());
            }
        }
        if let Some(error) = file_error.lock().unwrap().take() {
            return Err(error);
        }
        Ok(())
    }
}

fn remember_first_error(slot: &Mutex<Option<Error>>, error: Error) {
    let mut slot = slot.lock().unwrap();
    if slot.is_none() {
        *slot = Some(error);
    }
}

/// A file whose contents matched the prefilter, plus its full text.
#[derive(Clone)]
struct SourceFile {
    path: PathBuf,
    contents: String,
}

/// Walk `dirs` and return every regular file whose contents match `matcher`.
/// Mirrors the discovery rules used by the apply paths (hidden/ignore/globs).
fn matching_files(
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
                if looks_like_code(path)
                    && let Some(contents) = file_contents_if_matches(&mut searcher, &matcher, path)
                {
                    files.lock().unwrap().push(SourceFile {
                        path: path.to_path_buf(),
                        contents,
                    });
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

fn json_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn json_string(text: &str) -> String {
    format!("\"{}\"", json_escape(text))
}

/// One regex match inside a source file, for JSON edit records.
struct TextEdit {
    path: PathBuf,
    byte_start: usize,
    byte_end: usize,
    line: usize,
    column: usize,
    excerpt: String,
}

fn text_edits(
    regex: &Regex,
    file: &SourceFile,
) -> Vec<TextEdit> {
    regex
        .captures_iter(&file.contents)
        .map(|captures| {
            let mat = captures.get(0).unwrap();
            let (line0, col0) = index_to_row_col(&file.contents, mat.start());
            let line_start = file.contents[..mat.start()].rfind('\n').map_or(0, |i| i + 1);
            let line_end = file.contents[mat.start()..]
                .find('\n')
                .map_or(file.contents.len(), |i| mat.start() + i);
            TextEdit {
                path: file.path.clone(),
                byte_start: mat.start(),
                byte_end: mat.end(),
                line: line0 + 1,
                column: col0 + 1,
                excerpt: file.contents[line_start..line_end].to_string(),
            }
        })
        .collect()
}

fn text_json_report(
    mode: Mode,
    applied: bool,
    roots: &[String],
    files_scanned: usize,
    files_edited: usize,
    edits: &[TextEdit],
) -> String {
    let mut out = String::new();
    out.push_str("{\"schema\":1,\"mode\":");
    out.push_str(json_string(mode.name()).as_str());
    out.push_str(&format!(",\"applied\":{applied}"));
    out.push_str(",\"coverage\":{\"roots\":[");
    for (i, root) in roots.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&json_string(root));
    }
    out.push_str(&format!(
        "],\"files_scanned\":{files_scanned},\"files_edited\":{files_edited},\"files_skipped\":[],\"unresolved\":[]}}"
    ));
    out.push_str(",\"edits\":[");
    for (i, edit) in edits.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('{');
        out.push_str("\"path\":");
        out.push_str(&json_string(&edit.path.to_string_lossy()));
        out.push_str(&format!(
            ",\"byte_start\":{},\"byte_end\":{},\"anchor\":{{\"line\":{},\"column\":{},\"excerpt\":{}}},\"kind\":\"text\"}}",
            edit.byte_start,
            edit.byte_end,
            edit.line,
            edit.column,
            json_string(&edit.excerpt)
        ));
    }
    out.push_str("],\"diagnostics\":[]}\n");
    out
}

fn languages_report(json: bool) -> String {
    if json {
        return "{\"schema\":1,\"modes\":{\"text\":{\"status\":\"available\"},\"ident\":{\"status\":\"unavailable\",\"ticket\":\"REFAC-6\"},\"rename\":{\"status\":\"unavailable\",\"ticket\":\"REFAC-7\"},\"rewrite\":{\"status\":\"unavailable\",\"ticket\":\"REFAC-6\"},\"rules\":{\"status\":\"unavailable\",\"ticket\":\"REFAC-12\"},\"apply-plan\":{\"status\":\"unavailable\",\"ticket\":\"REFAC-12\"}},\"languages\":[]}\n".to_string();
    }
    let mut out = String::new();
    out.push_str(&format!("fastermod {} capability report\n", env!("CARGO_PKG_VERSION")));
    out.push_str("modes:\n");
    out.push_str("  text       available (regex/literal substitution)\n");
    out.push_str("  ident      unavailable (REFAC-6)\n");
    out.push_str("  rename     unavailable (REFAC-7/8)\n");
    out.push_str("  rewrite    unavailable (REFAC-6)\n");
    out.push_str("  rules      unavailable (REFAC-12)\n");
    out.push_str("  apply-plan unavailable (REFAC-12)\n");
    out.push_str("languages: no syntax-aware adapters yet; the typed capability\n");
    out.push_str("registry lands with REFAC-6. Parser availability is not rename\n");
    out.push_str("support.\n");
    out
}

/// Preview (or report on) text-mode edits without writing, unless `apply`.
/// Returns the process exit code.
#[allow(clippy::too_many_arguments)]
fn run_text_pipeline(
    regex: &Regex,
    matcher: &RegexMatcher,
    substitution: &Substitution,
    dirs: Vec<&str>,
    file_set: Option<FileSet>,
    hidden: bool,
    no_ignore: bool,
    print_changed_files: bool,
    apply: bool,
    check: bool,
    json: bool,
    roots: &[String],
) -> std::result::Result<i32, Failure> {
    let files = operational(matching_files(dirs, file_set, hidden, no_ignore, matcher))?;
    let files_scanned = files.len();

    // Compute the full edit set up front so review, check, JSON and apply all
    // describe the same operation.
    let mut changes: Vec<(PathBuf, String, String)> = Vec::new();
    let mut all_edits: Vec<TextEdit> = Vec::new();
    for file in &files {
        if let Cow::Owned(new_contents) = substitution.replace_all(regex, &file.contents) {
            if json || check {
                all_edits.extend(text_edits(regex, file));
            }
            changes.push((file.path.clone(), file.contents.clone(), new_contents));
        }
    }
    let files_edited = changes.len();

    if json {
        if apply {
            let mut fm = Fastermod::new(true, hidden, no_ignore, false);
            for (path, _, new_contents) in &changes {
                operational(fm.save(path, new_contents))?;
            }
            print!(
                "{}",
                text_json_report(Mode::Text, true, roots, files_scanned, files_edited, &all_edits)
            );
            return Ok(0);
        }
        print!(
            "{}",
            text_json_report(Mode::Text, false, roots, files_scanned, files_edited, &all_edits)
        );
        return Ok(if check && files_edited > 0 { 1 } else { 0 });
    }

    if check {
        for (path, _, _) in &changes {
            println!("{}", path.display());
        }
        return Ok(if files_edited > 0 { 1 } else { 0 });
    }

    // Preview: show diffs, never write.
    let mut fm = Fastermod::new(false, hidden, no_ignore, print_changed_files);
    for (path, old_contents, new_contents) in &changes {
        let diffs = fm.diffs_to_print(old_contents, new_contents);
        if diffs.is_empty() {
            continue;
        }
        let (start_line, end_line) = diff_line_span(&compute_diff(old_contents, new_contents));
        if start_line == end_line {
            println!("{}:{}", path.display(), start_line);
        } else {
            println!("{}:{}-{}", path.display(), start_line, end_line);
        }
        fm.print_diff(&diffs);
        if print_changed_files {
            fm.record_change(path.clone());
        }
    }
    fm.print_changed_files_if_needed();
    Ok(0)
}

/// First and last changed 1-based line numbers described by a line diff.
fn diff_line_span(diff: &HistogramDiff<'_>) -> (usize, usize) {
    let mut start = usize::MAX;
    let mut end = 0;
    for hunk in diff.hunks() {
        let hunk_start = min(hunk.before.start, hunk.after.start);
        let hunk_end = max(hunk.before.end, hunk.after.end);
        start = min(start, hunk_start);
        end = max(end, hunk_end);
    }
    if start == usize::MAX {
        return (1, 1);
    }
    (start + 1, max(end, start + 1))
}

fn fastermod() -> std::result::Result<i32, Failure> {
    let Args {
        mode,
        multiline,
        dir,
        file_or_dir,
        ignore_case,
        smart_case,
        extensions,
        glob,
        hidden,
        no_ignore,
        iglob,
        accept_all,
        apply,
        dry_run,
        check,
        json,
        plan,
        apply_plan,
        rules_file,
        at_anchor,
        langs,
        print_changed_files,
        fixed_strings,
        match_regex,
        subst,
    } = parse_args();

    if mode == Mode::Languages {
        print!("{}", languages_report(json));
        return Ok(0);
    }
    for unimplemented in [
        (mode == Mode::Ident, "--ident"),
        (mode == Mode::Rename, "--rename"),
        (mode == Mode::Rewrite, "--rewrite"),
        (mode == Mode::Rules, "--rules"),
        (mode == Mode::ApplyPlan, "--apply-plan"),
    ] {
        if unimplemented.0 {
            return Err(Failure::analysis(format!(
                "{} is not yet implemented in this build",
                unimplemented.1
            )));
        }
    }
    if plan.is_some() {
        return Err(Failure::analysis(
            "--plan is not yet implemented in this build",
        ));
    }
    let _ = (apply_plan, rules_file, at_anchor, langs);

    let dirs = {
        let mut dirs: Vec<_> = dir
            .iter()
            .map(String::as_str)
            .chain(file_or_dir.iter().map(String::as_str))
            .collect();
        if dirs.is_empty() {
            dirs.push(".");
        }
        dirs
    };
    let roots: Vec<String> = dirs.iter().map(|d| d.to_string()).collect();
    let file_set = if let Some(files) = extensions {
        Some(FileSet::Extensions(files))
    } else if let Some(files) = glob {
        Some(FileSet::Glob {
            matches: files,
            case_insensitive: false,
        })
    } else {
        iglob.map(|files| FileSet::Glob {
            matches: files,
            case_insensitive: true,
        })
    };
    let regex_str = match_regex.as_str();
    let subst = subst.as_str();
    let (maybe_escaped_regex, subst) = if fixed_strings {
        (regex_lite::escape(regex_str), subst.replace("$", "$$"))
    } else {
        (regex_str.to_string(), subst.to_string())
    };
    let substitution = Substitution::new(subst, smart_case);
    let case_insensitive =
        ignore_case || (smart_case && !regex_str.chars().any(char::is_uppercase));
    let regex = RegexBuilder::new(&maybe_escaped_regex)
        .case_insensitive(case_insensitive)
        .multi_line(true)
        .dot_matches_new_line(multiline)
        .build()
        .map_err(|e| {
            Failure::usage(format!("Unable to make regex from {}: {}", regex_str, e))
        })?;
    let stdin_tty = std::io::stdin().is_terminal();
    if regex.is_match("") {
        if !stdin_tty {
            return Err(Failure::usage(
                "regex matches the empty string; refusing to continue",
            ));
        }
        match prompt_reply_stderr(&format!(
            "Warning: your regex {:?} matches the empty string. This is probably
not what you want. Press Enter to continue anyway or Ctrl-C to quit.",
            regex,
        )) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(Failure::usage(
                    "refusing to continue: regex matches the empty string",
                ));
            }
            Err(error) => {
                return Err(Failure::io(format!("Unable to read user input: {error}")));
            }
        }
    }
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(case_insensitive)
        .multi_line(true)
        .dot_matches_new_line(multiline)
        .build(&maybe_escaped_regex)
        .map_err(|e| Failure::usage(format!("Unable to make regex from {}: {}", regex_str, e)))?;

    let apply = accept_all || apply;
    let stdout_tty = std::io::stdout().is_terminal();
    // Interactive per-match review needs a terminal on both sides and no
    // explicit apply/dry-run/check/JSON request.
    let interactive = !apply && !dry_run && !check && !json && stdin_tty && stdout_tty;

    if interactive {
        operational(
            Fastermod::new(false, hidden, no_ignore, print_changed_files).run_interactive(
                &regex,
                &matcher,
                &substitution,
                dirs,
                file_set,
            ),
        )?;
        return Ok(0);
    }

    // Plain unattended apply keeps the parallel fast path. Preview, check,
    // JSON and apply-with-JSON share the single-snapshot pipeline so the
    // report describes exactly what was (or would be) written.
    if apply && !json {
        operational(Fastermod::run_fast(
            &regex,
            &matcher,
            &substitution,
            dirs,
            file_set,
            hidden,
            no_ignore,
            print_changed_files,
        ))?;
        return Ok(0);
    }

    run_text_pipeline(
        &regex,
        &matcher,
        &substitution,
        dirs,
        file_set,
        hidden,
        no_ignore,
        print_changed_files,
        apply,
        check,
        json,
        &roots,
    )
}

fn main() {
    match fastermod() {
        Ok(code) => exit(code),
        Err(failure) => {
            eprintln!("error: {}", failure.message);
            exit(failure.code);
        }
    }
}

#[doc(hidden)]
pub mod benchmark {
    use super::*;

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
        file_contents_if_matches(searcher, matcher, path).map(|contents| contents.len())
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
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::Write;
    use std::sync::OnceLock;
    use std::time::SystemTime;

    use super::*;

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn with_prefix(prefix: &str) -> TestDir {
            let base = env::temp_dir();
            for _ in 0..1000 {
                let unique = format!(
                    "{prefix}{}-{}",
                    std::process::id(),
                    SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos(),
                );
                let path = base.join(unique);
                if fs::create_dir(&path).is_ok() {
                    return TestDir { path };
                }
            }
            panic!("unable to create temp directory in {}", base.display());
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    // Unit tests are not given CARGO_BIN_EXE_<name>, and `cargo test` does not
    // rebuild this crate's binary. The test executable sits under deps/ or,
    // with a separate build directory, under build/<package>/<hash>/out/.
    fn fastermod_binary() -> &'static Path {
        static BINARY: OnceLock<PathBuf> = OnceLock::new();
        BINARY.get_or_init(|| {
            let env_name = format!("CARGO_BIN_EXE_{}", env!("CARGO_PKG_NAME"));
            if let Some(binary) = env::var_os(&env_name) {
                return PathBuf::from(binary);
            }

            let name = env!("CARGO_PKG_NAME");
            let exe = env::current_exe().unwrap();
            let find = || exe.ancestors().skip(1).map(|dir| dir.join(name)).find(|path| path.is_file());
            if let Some(binary) = find()
                && binary_is_at_least_as_new(&binary, &exe)
            {
                return binary;
            }

            let mut build = Command::new("cargo");
            build.args(["build", "--quiet", "--bin", env!("CARGO_PKG_NAME")]);
            if !cfg!(debug_assertions) {
                build.arg("--release");
            }
            let status = build
                .current_dir(env!("CARGO_MANIFEST_DIR"))
                .status()
                .unwrap();
            assert!(status.success(), "unable to build test binary");
            find().expect("built test binary not found next to the test executable")
        })
    }

    fn binary_is_at_least_as_new(binary: &Path, exe: &Path) -> bool {
        let binary_modified = fs::metadata(binary).and_then(|metadata| metadata.modified());
        let exe_modified = fs::metadata(exe).and_then(|metadata| metadata.modified());
        match (binary_modified, exe_modified) {
            (Ok(binary_modified), Ok(exe_modified)) => binary_modified >= exe_modified,
            _ => false,
        }
    }

    fn fastermod_command() -> Command {
        Command::new(fastermod_binary())
    }

    fn run_fastermod<I, S>(args: I, input: Option<&str>) -> std::process::Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let mut command = fastermod_command();
        command
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if input.is_some() {
            command.stdin(std::process::Stdio::piped());
        } else {
            command.stdin(std::process::Stdio::null());
        }

        let mut child = command.spawn().unwrap();
        if let Some(input) = input {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
        }
        child.wait_with_output().unwrap()
    }

    fn assert_fastermod<I, S>(args: I, input: Option<&str>, expected_stdout: Option<&str>)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let output = run_fastermod(args, input);
        assert!(
            output.status.success(),
            "fastermod failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        if let Some(expected_stdout) = expected_stdout {
            assert_eq!(String::from_utf8(output.stdout).unwrap(), expected_stdout);
        }
    }

    #[test]
    fn test_index_to_row_col() {
        assert_eq!(index_to_row_col("abc", 1), (0, 1));
        assert_eq!(index_to_row_col("abc\ndef", 2), (0, 2));
        assert_eq!(index_to_row_col("abc\ndef", 3), (0, 3));
        assert_eq!(index_to_row_col("abc\ndef", 4), (1, 0));
        assert_eq!(index_to_row_col("abc\ndef\nghi", 8), (2, 0));
    }

    fn create_test_files<'a>(
        names_and_contents: impl IntoIterator<Item = &'a (&'a str, &'a str)>,
    ) -> TestDir {
        let dir = TestDir::with_prefix("fastermodtest.");
        for (name, contents) in names_and_contents {
            let path = dir.path().join(name);
            let mut file = File::create(path.clone()).unwrap();
            file.write_all(contents.as_bytes()).unwrap();
            file.sync_all().unwrap();
        }
        dir
    }

    #[test]
    fn test_simple_replace_all() {
        let dir = create_test_files(&[("file1.c", "foo\nfoo blah foo")]);
        assert_fastermod(
            [
                "foo",
                "bar",
                "--accept-all",
                "--dir",
                dir.path().to_str().unwrap(),
            ],
            None,
            None,
        );
        let contents = read_to_string(dir.path().join("file1.c")).unwrap();
        assert_eq!(contents, "bar\nbar blah bar");
    }

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

    #[test]
    fn test_tags_like_filenames_are_rewritten() {
        let dir = create_test_files(&[
            ("keep.rs", "foo"),
            ("ctags", "foo"),
            ("mytags", "foo"),
            ("file.tags", "foo"),
            ("tags", "foo"),
        ]);
        assert_fastermod(
            [
                "foo",
                "bar",
                "--accept-all",
                "--dir",
                dir.path().to_str().unwrap(),
            ],
            None,
            None,
        );
        for name in ["keep.rs", "ctags", "mytags", "file.tags"] {
            assert_eq!(read_to_string(dir.path().join(name)).unwrap(), "bar");
        }
        assert_eq!(read_to_string(dir.path().join("tags")).unwrap(), "foo");
    }

    #[test]
    fn test_glob_matches() {
        let dir = create_test_files(&[
            ("f1.txt", "some awesome text"),
            ("f2.TXT", "some more awesome text"),
            ("skip.rs", "i should be skipped but i am still awesome"),
        ]);

        assert_fastermod(
            [
                "awesome",
                "great",
                "--accept-all",
                "--iglob",
                "*.txt",
                "--dir",
                dir.path().to_str().unwrap(),
            ],
            None,
            None,
        );

        let lower_translated = read_to_string(dir.path().join("f1.txt")).unwrap();
        let upper_translated = read_to_string(dir.path().join("f2.TXT")).unwrap();
        let skipped_translated = read_to_string(dir.path().join("skip.rs")).unwrap();
        assert_eq!(lower_translated, "some great text");
        assert_eq!(upper_translated, "some more great text");
        assert_eq!(
            skipped_translated,
            "i should be skipped but i am still awesome"
        );
    }

    #[test]
    fn test_fixed_strings() {
        let dir = create_test_files(&[("file1.txt", "foo+bar\nfoooobar")]);
        let file_path = dir.path().join("file1.txt");
        assert_fastermod(
            [
                "foo+bar",
                "baz",
                "--accept-all",
                "--dir",
                dir.path().to_str().unwrap(),
                "--fixed-strings",
            ],
            None,
            None,
        );
        let contents = read_to_string(file_path).unwrap();
        assert_eq!(contents, "baz\nfoooobar");
    }

    #[test]
    fn test_diff_with_unchanged_line_in_middle() {
        let fm = Fastermod::new(false, false, false, false);
        let diffs = fm.diffs_to_print("foo\nbar\nbaz", "bat\nbar\nqux");
        assert_eq!(
            diffs,
            vec![
                DiffResult::Left("foo"),
                DiffResult::Right("bat"),
                DiffResult::Both("bar", "bar"),
                DiffResult::Left("baz"),
                DiffResult::Right("qux"),
            ]
        )
    }

    #[test]
    fn test_diff_no_changes() {
        let fm = Fastermod::new(false, false, false, false);
        let diffs = fm.diffs_to_print("foo", "foo");
        assert_eq!(diffs, vec![]);
    }

    #[test]
    fn test_diff_context_does_not_change_for_large_unchanged_prefix() {
        let original: String = (0..100).map(|line| format!("line {line}\n")).collect();
        let edited = original.replacen("line 50", "changed", 1);
        let fm = Fastermod::new(false, false, false, false);

        let diffs = fm.diffs_to_print(&original, &edited);

        assert_eq!(diffs.len(), 5);
        assert_eq!(diffs[0], DiffResult::Both("line 49", "line 49"));
        assert_eq!(diffs[1], DiffResult::Left("line 50"));
        assert_eq!(diffs[2], DiffResult::Right("changed"));
        assert_eq!(diffs[3], DiffResult::Both("line 51", "line 51"));
        assert_eq!(diffs[4], DiffResult::Both("line 52", "line 52"));
    }

    #[test]
    fn test_diff_context_preserves_insertions_and_removals() {
        let fm = Fastermod::new(false, false, false, false);

        assert_eq!(
            fm.diffs_to_print("a\nb", "a\nx\nb"),
            vec![
                DiffResult::Both("a", "a"),
                DiffResult::Right("x"),
                DiffResult::Both("b", "b"),
            ]
        );
        assert_eq!(
            fm.diffs_to_print("a\nx\nb", "a\nb"),
            vec![
                DiffResult::Both("a", "a"),
                DiffResult::Left("x"),
                DiffResult::Both("b", "b"),
            ]
        );
    }

    #[test]
    fn test_print_changed_files() {
        let dir = TestDir::with_prefix("fastermodtest.");
        let mut expected_changed_files = Vec::new();
        for file_num in 1..6 {
            let path = dir.path().join(format!("file{}.c", file_num));
            let mut file = File::create(path.clone()).unwrap();
            file.write_all(if file_num % 2 == 0 {
                b"foo\n"
            } else {
                b"bar\n"
            })
            .unwrap();
            file.sync_all().unwrap();
            if file_num % 2 == 0 {
                expected_changed_files.push(path.as_os_str().to_string_lossy().into_owned());
            }
        }
        assert_fastermod(
            [
                "foo",
                "baz",
                "--accept-all",
                "--print-changed-files",
                "--dir",
                dir.path().to_str().unwrap(),
            ],
            None,
            Some(&format!("{}\n", expected_changed_files.join("\n"))),
        );
    }

    #[test]
    fn test_accept_longer_replacement_before_multibyte_char() {
        let dir = create_test_files(&[("foo.txt", "aé")]);
        let file_path = dir.path().join("foo.txt");
        let regex = RegexBuilder::new("a").build().unwrap();
        let mut fm = Fastermod::new(true, false, false, false);
        fm.present_and_apply_patches(
            &regex,
            &Substitution::new("bb".to_owned(), false),
            &file_path,
            "aé".into(),
        )
            .unwrap();
        let contents = read_to_string(file_path).unwrap();
        assert_eq!(contents, "bbé");
    }

    #[test]
    fn test_zero_length_match() {
        let dir = create_test_files(&[("foo.txt", "foo")]);
        let file_path = dir.path().join("foo.txt");
        let regex = RegexBuilder::new("").multi_line(true).build().unwrap();
        let mut fm = Fastermod::new(true, false, false, false);
        fm.present_and_apply_patches(
            &regex,
            &Substitution::new("x".to_owned(), false),
            &file_path,
            "foo".into(),
        )
            .unwrap();
        let contents = read_to_string(file_path).unwrap();
        assert_eq!(contents, "xfxoxo");
    }

    #[test]
    fn test_zero_length_replacement() {
        let dir = create_test_files(&[("foo.txt", "foofoo")]);
        let file_path = dir.path().join("foo.txt");
        let regex = RegexBuilder::new("foo").multi_line(true).build().unwrap();
        let mut fm = Fastermod::new(true, false, false, false);
        fm.present_and_apply_patches(
            &regex,
            &Substitution::new(String::new(), false),
            &file_path,
            "foofoo".into(),
        )
            .unwrap();
        let contents = read_to_string(file_path).unwrap();
        assert_eq!(contents, "");
    }

    #[test]
    fn test_replacement_matches_search() {
        let dir = create_test_files(&[("foo.txt", "foo")]);
        let file_path = dir.path().join("foo.txt");
        let regex = RegexBuilder::new("foo").build().unwrap();
        let mut fm = Fastermod::new(true, false, false, false);
        fm.present_and_apply_patches(
            &regex,
            &Substitution::new("foofoo".to_owned(), false),
            &file_path,
            "foo".into(),
        )
            .unwrap();
        let contents = read_to_string(file_path).unwrap();
        assert_eq!(contents, "foofoo");
    }

    #[test]
    fn test_interactive_yes_to_all_finishes_fast() {
        let dir = create_test_files(&[
            ("a.txt", "foo"),
            ("b.txt", "foo"),
            ("c.txt", "foo"),
        ]);
        assert_fastermod(
            [
                "foo",
                "bar",
                "--accept-all",
                "--dir",
                dir.path().to_str().unwrap(),
            ],
            Some("A\n"),
            None,
        );
        for name in ["a.txt", "b.txt", "c.txt"] {
            assert_eq!(read_to_string(dir.path().join(name)).unwrap(), "bar");
        }
    }

    #[test]
    fn test_empty_contents() {
        let dir = create_test_files(&[("foo.txt", "foo")]);
        assert_fastermod(
            ["foo", "baz", "--dir", dir.path().to_str().unwrap()],
            Some("n\n"),
            None,
        );
    }

    #[cfg(target_family = "unix")]
    #[test]
    fn test_hidden_files() {
        for &hidden in &[false, true] {
            let dir = create_test_files(&[(".hidden", "foo")]);
            let mut args = vec![
                "foo",
                "bar",
                "--dir",
                dir.path().to_str().unwrap(),
                "--accept-all",
            ];
            if hidden {
                args.push("--hidden");
            }

            assert_fastermod(args.iter().copied(), None, None);
            let file_path = dir.path().join(".hidden");
            let contents = read_to_string(file_path).unwrap();
            if hidden {
                assert_eq!(contents, "bar");
            } else {
                assert_eq!(contents, "foo");
            }
        }
    }

    #[test]
    fn test_no_ignore_files() {
        for &no_ignore in &[false, true] {
            let dir = create_test_files(&[
                (".ignore", "ignored.txt"),
                ("ignored.txt", "original text"),
                ("not-ignored.txt", "original text"),
            ]);
            let mut args = vec![
                "original",
                "replacement",
                "--dir",
                dir.path().to_str().unwrap(),
                "--accept-all",
            ];
            if no_ignore {
                args.push("--no-ignore");
            }

            assert_fastermod(args.iter().copied(), None, None);

            let file_path_regular = dir.path().join("not-ignored.txt");
            let contents_regular = read_to_string(file_path_regular).unwrap();
            assert_eq!(contents_regular, "replacement text");

            let file_path_ignored = dir.path().join("ignored.txt");
            let contents_ignored = read_to_string(file_path_ignored).unwrap();
            if no_ignore {
                assert_eq!(contents_ignored, "replacement text");
            } else {
                assert_eq!(contents_ignored, "original text");
            }
        }
    }

    #[test]
    fn test_replace_next_to_unicode_character() {
        let contents = "I have “unicodequotes”";
        let dir = create_test_files(&[("foo.txt", contents)]);
        assert_fastermod(
            [
                "quotes",
                "characters",
                "--dir",
                dir.path().to_str().unwrap(),
            ],
            Some("n\n"),
            None,
        );
    }

    #[test]
    fn test_dollar_sign_in_replacement_string_with_fixed_strings() {
        let contents = "something";
        let dir = create_test_files(&[("foo.txt", contents)]);
        assert_fastermod(
            [
                "-F",
                "something",
                "$foo.bar",
                "--accept-all",
                "--dir",
                dir.path().to_str().unwrap(),
            ],
            Some("y\n"),
            None,
        );
        let file_path = dir.path().join("foo.txt");
        let new_contents = read_to_string(file_path).unwrap();
        assert_eq!(new_contents, "$foo.bar");
    }

    #[test]
    fn test_invalid_regex_exits_nonzero() {
        let dir = TestDir::with_prefix("fastermodtest.");
        let output = run_fastermod(["(", "x", dir.path().to_str().unwrap()], None);
        assert!(
            !output.status.success(),
            "invalid regex exited {:?}",
            output.status.code()
        );
    }

    #[test]
    fn test_empty_pattern_eof_exits_nonzero() {
        let dir = create_test_files(&[("file.txt", "aaa")]);
        let output = run_fastermod(
            [
                "--accept-all",
                "a*",
                "x",
                dir.path().to_str().unwrap(),
            ],
            None,
        );
        assert!(
            !output.status.success(),
            "empty pattern with closed stdin exited {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
        let contents = read_to_string(dir.path().join("file.txt")).unwrap();
        assert_eq!(contents, "aaa");
    }

    #[cfg(unix)]
    #[test]
    fn test_permission_denied_write_exits_nonzero() {
        use std::os::unix::fs::PermissionsExt;

        let dir = create_test_files(&[("ro.txt", "foo"), ("rw.txt", "foo")]);
        let ro = dir.path().join("ro.txt");
        let mut permissions = fs::metadata(&ro).unwrap().permissions();
        permissions.set_mode(0o444);
        fs::set_permissions(&ro, permissions).unwrap();

        let output = run_fastermod(
            [
                "--accept-all",
                "foo",
                "BAR",
                dir.path().to_str().unwrap(),
            ],
            None,
        );
        assert!(
            !output.status.success(),
            "permission denied write exited {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(read_to_string(&ro).unwrap(), "foo");
        assert_eq!(read_to_string(dir.path().join("rw.txt")).unwrap(), "BAR");
    }
}
