//! Realistic fastmod workloads with wall-time and allocation measurements.
//!
//! The direct single-threaded scenarios are the allocation probes. The
//! directory-walk scenarios are primarily wall-time probes because rustybench
//! does not currently tally allocations made by worker threads it did not
//! create.

#[cfg(target_os = "linux")]
use std::ffi::CStr;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use fastermod::benchmark;
use rustybench::{AllocProfiler, Bencher, black_box};

#[global_allocator]
static ALLOC: AllocProfiler = AllocProfiler::system();

#[cfg(target_os = "linux")]
const TRACE_BEGIN: &CStr = c"BENCH_BEGIN";
#[cfg(target_os = "linux")]
const TRACE_END: &CStr = c"BENCH_END";

#[cfg(target_os = "linux")]
unsafe extern "C" {
    fn prctl(option: i32, ...) -> i32;
}

#[cfg(target_os = "linux")]
fn syscall_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("SYSCALL_TRACE").is_some())
}

#[cfg(target_os = "linux")]
fn trace_marker(marker: &CStr) {
    if syscall_trace_enabled() {
        unsafe {
            let _ = prctl(15, marker.as_ptr(), 0, 0, 0);
        }
    }
}

#[cfg(target_os = "linux")]
fn bench_with_syscall_trace<O>(bencher: Bencher, mut operation: impl FnMut() -> O) {
    bencher.bench_local(|| {
        trace_marker(TRACE_BEGIN);
        let result = operation();
        trace_marker(TRACE_END);
        black_box(result);
    });
}

#[cfg(not(target_os = "linux"))]
fn bench_with_syscall_trace<O>(bencher: Bencher, operation: impl FnMut() -> O) {
    bencher.bench_local(operation);
}

struct BenchDir {
    path: PathBuf,
}

impl BenchDir {
    fn new(label: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "fastermod-bench-{label}-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for BenchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn append_rust_line(buffer: &mut Vec<u8>, index: usize, token: &str) {
    match index % 6 {
        0 => buffer.extend_from_slice(
            format!("pub fn transform_{index:05}(input: &[u8]) -> Vec<u8> {{ // {token}\n")
                .as_bytes(),
        ),
        1 => buffer.extend_from_slice(
            format!("    let mut output = Vec::with_capacity(input.len() + {index});\n").as_bytes(),
        ),
        2 => buffer.extend_from_slice(
            format!("    output.extend(input.iter().map(|byte| byte.wrapping_add({index})));\n")
                .as_bytes(),
        ),
        3 => buffer.extend_from_slice(
            format!("    if output.len() > {index} {{ output.truncate({index}); }}\n").as_bytes(),
        ),
        4 => buffer.extend_from_slice(b"    output\n"),
        _ => buffer.extend_from_slice(b"}\n\n"),
    }
}

fn make_rust_source(lines: usize, token: &str) -> Vec<u8> {
    let mut buffer = Vec::with_capacity(lines * 96);
    for index in 0..lines {
        append_rust_line(&mut buffer, index, token);
    }
    buffer
}

fn make_edited_rust_source(lines: usize) -> Vec<u8> {
    let mut source = Vec::with_capacity(lines * 96);
    for index in 0..lines {
        let token = if matches!(index, 1_000 | 5_000 | 9_000) {
            "changed"
        } else {
            "needle"
        };
        append_rust_line(&mut source, index, token);
    }
    source
}

fn write_source_tree(directory: &Path, files: usize, lines_per_file: usize, token: &str) {
    for file_index in 0..files {
        let subdirectory = directory.join(format!("module-{:02}", file_index % 8));
        fs::create_dir_all(&subdirectory).unwrap();
        let extension = if file_index % 5 == 0 { "md" } else { "rs" };
        let path = subdirectory.join(format!("file-{file_index:03}.{extension}"));
        fs::write(path, make_rust_source(lines_per_file, token)).unwrap();
    }
}

#[rustybench::bench]
fn search_matching_1mb_file(b: Bencher) {
    let dir = BenchDir::new("search-match");
    let path = dir.path().join("fixture.rs");
    fs::write(&path, make_rust_source(11_000, "needle")).unwrap();
    let matcher = benchmark::new_matcher("needle").unwrap();
    let mut searcher = benchmark::new_searcher();

    bench_with_syscall_trace(b, || {
        black_box(benchmark::search_file(&mut searcher, &matcher, &path));
    });
}

#[rustybench::bench]
fn search_missing_1mb_file(b: Bencher) {
    let dir = BenchDir::new("search-miss");
    let path = dir.path().join("fixture.rs");
    fs::write(&path, make_rust_source(11_000, "different-token")).unwrap();
    let matcher = benchmark::new_matcher("needle").unwrap();
    let mut searcher = benchmark::new_searcher();

    bench_with_syscall_trace(b, || {
        black_box(benchmark::search_file(&mut searcher, &matcher, &path));
    });
}

#[rustybench::bench]
fn replace_all_rust_10k(b: Bencher) {
    let source = String::from_utf8(make_rust_source(10_000, "needle")).unwrap();
    let regex = benchmark::new_regex(r"needle\b").unwrap();

    bench_with_syscall_trace(b, || {
        black_box(benchmark::replace_all_len(
            &regex,
            &source,
            "replacement_token",
        ));
    });
}

#[rustybench::bench]
fn diff_sparse_rust_10k(b: Bencher) {
    let original = String::from_utf8(make_rust_source(10_000, "needle")).unwrap();
    let edited = String::from_utf8(make_edited_rust_source(10_000)).unwrap();

    bench_with_syscall_trace(b, || {
        black_box(benchmark::diff_line_count(&original, &edited));
    });
}

#[rustybench::bench]
fn patch_single_1mb_file(b: Bencher) {
    let dir = BenchDir::new("patch");
    let path = dir.path().join("fixture.rs");
    let original = String::from_utf8(make_rust_source(11_000, "needle_a")).unwrap();
    let replacement = String::from_utf8(make_rust_source(11_000, "needle_b")).unwrap();
    fs::write(&path, &original).unwrap();
    let regex_a = benchmark::new_regex(r"needle_a\b").unwrap();
    let regex_b = benchmark::new_regex(r"needle_b\b").unwrap();
    let mut use_a = true;

    bench_with_syscall_trace(b, || {
        let (regex, substitution, contents) = if use_a {
            (&regex_a, "needle_b", &original)
        } else {
            (&regex_b, "needle_a", &replacement)
        };
        let changed = benchmark::patch_file(regex, substitution, &path, contents).unwrap();
        use_a = !use_a;
        black_box(changed);
    });
}

#[rustybench::bench]
fn run_fast_no_match_100_files(b: Bencher) {
    let dir = BenchDir::new("walk-miss");
    write_source_tree(dir.path(), 100, 100, "different-token");
    let regex = benchmark::new_regex("needle").unwrap();
    let matcher = benchmark::new_matcher("needle").unwrap();

    bench_with_syscall_trace(b, || {
        benchmark::run_fast(dir.path(), &regex, &matcher, "replacement", Some("rs")).unwrap();
    });
}

#[rustybench::bench]
fn run_fast_match_100_files(b: Bencher) {
    let dir = BenchDir::new("walk-match");
    write_source_tree(dir.path(), 100, 100, "needle");
    let regex = benchmark::new_regex(r"(?:needle|replacement)\b").unwrap();
    let matcher = benchmark::new_matcher(r"(?:needle|replacement)\b").unwrap();

    bench_with_syscall_trace(b, || {
        benchmark::run_fast(dir.path(), &regex, &matcher, "replacement", Some("rs")).unwrap();
    });
}

fn main() {
    rustybench::main();
}
