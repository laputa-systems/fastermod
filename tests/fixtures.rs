//! Fixture-driven CLI tests.
//!
//! Each case is a directory under `tests/fixtures/`:
//!
//! - `args`: required, one argument per line; `{ROOT}` expands to a temporary
//!   copy of the fixture's `files/` tree (created fresh for every run).
//! - `files/`: optional input tree copied into the temp root.
//! - `stdin`: optional bytes piped to the child's stdin.
//! - `expected/`: optional tree compared byte-for-byte with the temp root
//!   after the run (the `args` file itself is not part of either tree).
//! - `expect-status`: optional expected exit code (default 0).
//! - `expect-stdout`, `expect-stderr`: optional exact-output files.
//!
//! Directories starting with `.` or `_` are skipped, so shared helpers can
//! live beside the cases.

use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

struct Case {
    name: String,
    dir: PathBuf,
}

fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for entry in fs::read_dir(FIXTURES).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name.starts_with('_') || !entry.file_type().unwrap().is_dir() {
            continue;
        }
        cases.push(Case {
            name,
            dir: entry.path(),
        });
    }
    cases.sort_by(|a, b| a.name.cmp(&b.name));
    cases
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> TempDir {
        for _ in 0..1000 {
            let unique = format!(
                "fastermod-fixture-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
            );
            let path = std::env::temp_dir().join(unique);
            if fs::create_dir(&path).is_ok() {
                return TempDir(path);
            }
        }
        panic!("unable to create fixture temp dir");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn copy_tree(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        let ty = entry.file_type().unwrap();
        if ty.is_dir() {
            fs::create_dir(&dest).unwrap();
            copy_tree(&entry.path(), &dest);
        } else if ty.is_file() {
            fs::copy(entry.path(), &dest).unwrap();
        }
    }
}

fn read_optional(path: &Path) -> Option<Vec<u8>> {
    fs::read(path).ok()
}

fn tree_files(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let ty = entry.file_type().unwrap();
            if ty.is_dir() {
                stack.push(path);
            } else if ty.is_file() {
                out.push((path.strip_prefix(root).unwrap().to_path_buf(), fs::read(&path).unwrap()));
            }
        }
    }
    out.sort();
    out
}

fn run_case(case: &Case) -> Output {
    let args_text = fs::read_to_string(case.dir.join("args")).expect("fixture missing args file");
    let temp = TempDir::new();
    let root = temp.0.clone();
    let files = case.dir.join("files");
    if files.is_dir() {
        copy_tree(&files, &root);
    }
    // One argument per line; a trailing newline does not create an empty arg.
    let args_text = args_text.strip_suffix('\n').unwrap_or(&args_text);
    let args: Vec<String> = args_text
        .lines()
        .map(|line| line.trim_end_matches('\r').replace("{ROOT}", root.to_str().unwrap()))
        .collect();

    let stdin_data = read_optional(&case.dir.join("stdin"));
    let mut command = Command::new(env!("CARGO_BIN_EXE_fastermod"));
    command
        .args(&args)
        .current_dir(&root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if stdin_data.is_some() {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }
    let mut child = command.spawn().unwrap();
    if let Some(data) = &stdin_data {
        child.stdin.take().unwrap().write_all(data).unwrap();
    }
    let output = child.wait_with_output().unwrap();

    let expect_status = fs::read_to_string(case.dir.join("expect-status"))
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
        .unwrap_or(0);
    assert_eq!(
        output.status.code(),
        Some(expect_status),
        "{}: unexpected exit status\nstderr: {}",
        case.name,
        String::from_utf8_lossy(&output.stderr)
    );

    if let Some(expected) = read_optional(&case.dir.join("expect-stdout")) {
        assert_eq!(
            output.stdout,
            expected,
            "{}: stdout mismatch",
            case.name
        );
    }
    if let Some(expected) = read_optional(&case.dir.join("expect-stderr")) {
        assert_eq!(
            output.stderr,
            expected,
            "{}: stderr mismatch",
            case.name
        );
    }
    if case.dir.join("expected").is_dir() {
        let expected_root = case.dir.join("expected");
        let got = tree_files(&root);
        let want = tree_files(&expected_root);
        let got: Vec<(PathBuf, Vec<u8>)> = got
            .into_iter()
            .filter(|(p, _)| p != Path::new("args"))
            .collect();
        assert_eq!(got, want, "{}: resulting tree mismatch", case.name);
    }
    output
}

#[test]
fn fixture_cases() {
    let cases = cases();
    assert!(!cases.is_empty(), "no fixture cases found in {FIXTURES}");
    for case in &cases {
        run_case(case);
    }
}

// Keep OsStr referenced so fixture names with non-UTF8 paths could be added
// without a signature change.
#[allow(dead_code)]
fn _unused(_: &OsStr) {}
