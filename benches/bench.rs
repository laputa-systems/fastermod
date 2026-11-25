//! Realistic fastmod workloads with wall-time and allocation measurements.
//!
//! The direct single-threaded scenarios are the allocation probes. The
//! directory-walk scenarios are primarily wall-time probes; the process-wide
//! allocation counters in attribution mode include their worker threads.
//!
//! `BENCH_ATTRIBUTIONS=1 cargo bench --bench bench` runs every workload once
//! without timing and prints CPU, allocation and I/O records. Timed runs keep
//! using `rustybench::main()`.

#[cfg(unix)]
use std::alloc::{GlobalAlloc, Layout};
#[cfg(target_os = "linux")]
use std::ffi::CStr;
use std::fs;
#[cfg(unix)]
use std::process::Command;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(target_os = "linux")]
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use fastermod::benchmark;
use rustybench::{AllocProfiler, Bencher, black_box};

#[cfg(unix)]
struct CountingAlloc {
    inner: AllocProfiler,
    allocations: AtomicU64,
    allocated_bytes: AtomicU64,
}

#[cfg(unix)]
impl CountingAlloc {
    const fn new() -> Self {
        Self {
            inner: AllocProfiler::system(),
            allocations: AtomicU64::new(0),
            allocated_bytes: AtomicU64::new(0),
        }
    }

    fn snapshot(&self) -> (u64, u64) {
        (
            self.allocations.load(Ordering::Relaxed),
            self.allocated_bytes.load(Ordering::Relaxed),
        )
    }
}

#[cfg(unix)]
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { self.inner.alloc(layout) };
        if !ptr.is_null() {
            self.allocations.fetch_add(1, Ordering::Relaxed);
            self.allocated_bytes
                .fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { self.inner.alloc_zeroed(layout) };
        if !ptr.is_null() {
            self.allocations.fetch_add(1, Ordering::Relaxed);
            self.allocated_bytes
                .fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { self.inner.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new = unsafe { self.inner.realloc(ptr, layout, new_size) };
        if !new.is_null() {
            self.allocations.fetch_add(1, Ordering::Relaxed);
            self.allocated_bytes.fetch_add(new_size as u64, Ordering::Relaxed);
        }
        new
    }
}

#[cfg(unix)]
#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

#[cfg(not(unix))]
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
        use_a.toggle();
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

#[cfg(unix)]
struct Journey {
    name: &'static str,
    op: Box<dyn FnMut() -> Vec<u8>>,
}

#[cfg(unix)]
fn cli_output(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_fastermod"))
        .args(args)
        .output()
        .expect("fastermod binary")
}

#[cfg(unix)]
fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
fn cli_text_apply_journey() -> Journey {
    let dir = BenchDir::new("cli-text-apply");
    let file = dir.path().join("input.rs");
    let file_arg = file.to_str().unwrap().to_owned();
    Journey {
        name: "cli_text_apply_journey",
        op: Box::new(move || {
            let _keep = &dir;
            fs::write(&file, "foo = 1\n").unwrap();
            let output = cli_output(&["--json", "foo", "bar", "--apply", &file_arg]);
            assert_success(&output);
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(stdout.contains("\"applied\":true"), "{stdout}");
            assert!(stdout.contains("\"files_edited\":1"), "{stdout}");
            assert_eq!(fs::read(&file).unwrap(), b"bar = 1\n");
            output.stdout
        }),
    }
}

#[cfg(unix)]
fn cli_rename_journey() -> Journey {
    let dir = BenchDir::new("cli-rename");
    let file = dir.path().join("input.py");
    let file_arg = file.to_str().unwrap().to_owned();
    Journey {
        name: "cli_binding_rename_journey",
        op: Box::new(move || {
            let _keep = &dir;
            fs::write(
                &file,
                "def helper(value):\n    return value + 1\n\nanswer = helper(2)\n",
            )
            .unwrap();
            let output = cli_output(&[
                "--json",
                "--rename",
                "helper",
                "compute",
                "--apply",
                &file_arg,
            ]);
            assert_success(&output);
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(stdout.contains("\"applied\":true"), "{stdout}");
            let edited = fs::read_to_string(&file).unwrap();
            assert!(edited.contains("def compute(value):"), "{edited}");
            assert!(edited.contains("answer = compute(2)"), "{edited}");
            output.stdout
        }),
    }
}

#[cfg(unix)]
fn cli_saved_plan_journey() -> Journey {
    let dir = BenchDir::new("cli-saved-plan");
    let file = dir.path().join("input.rs");
    let plan = dir.path().join("plan.json");
    let file_arg = file.to_str().unwrap().to_owned();
    let plan_arg = plan.to_str().unwrap().to_owned();
    Journey {
        name: "cli_saved_plan_journey",
        op: Box::new(move || {
            let _keep = &dir;
            fs::write(&file, "foo = 1\n").unwrap();
            let saved = cli_output(&["--plan", &plan_arg, "foo", "bar", &file_arg]);
            assert_success(&saved);
            assert_eq!(fs::read(&file).unwrap(), b"foo = 1\n");
            let applied = cli_output(&["--apply-plan", &plan_arg]);
            assert_success(&applied);
            let stdout = String::from_utf8_lossy(&applied.stdout);
            assert!(stdout.contains("applied 1 file(s) from plan"), "{stdout}");
            assert_eq!(fs::read(&file).unwrap(), b"bar = 1\n");
            applied.stdout
        }),
    }
}

#[cfg(unix)]
fn cli_review_journey() -> Journey {
    use rustix::io::{FdFlags, fcntl_setfd};
    use rustix::process::{ioctl_tiocsctty, setsid};
    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
    use rustix::termios::{Winsize, tcsetwinsize};
    use std::io::{Read, Write};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::process::CommandExt;
    use std::time::{Duration, Instant};

    let dir = BenchDir::new("cli-review");
    let file = dir.path().join("input.txt");
    let file_arg = file.to_str().unwrap().to_owned();
    Journey {
        name: "cli_interactive_review_journey",
        op: Box::new(move || {
            let _keep = &dir;
            fs::write(&file, "foo\n").unwrap();
            let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
            fcntl_setfd(&master, FdFlags::CLOEXEC).unwrap();
            grantpt(&master).unwrap();
            unlockpt(&master).unwrap();
            let name = ptsname(&master, Vec::new()).unwrap();
            let slave = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(std::ffi::OsStr::from_bytes(name.as_bytes()))
                .unwrap();
            tcsetwinsize(
                &slave,
                Winsize {
                    ws_row: 40,
                    ws_col: 80,
                    ws_xpixel: 0,
                    ws_ypixel: 0,
                },
            )
            .unwrap();
            let mut master = fs::File::from(master);
            master.write_all(b"y\n").unwrap();
            let controlling = slave.try_clone().unwrap();
            let mut command = Command::new(env!("CARGO_BIN_EXE_fastermod"));
            command
                .args(["foo", "bar", &file_arg])
                .stdin(std::process::Stdio::from(slave.try_clone().unwrap()))
                .stdout(std::process::Stdio::from(slave))
                .stderr(std::process::Stdio::piped());
            unsafe {
                command.pre_exec(move || {
                    setsid()?;
                    ioctl_tiocsctty(&controlling)?;
                    Ok(())
                });
            }
            let mut child = command.spawn().unwrap();
            let screen = std::thread::spawn(move || {
                let mut bytes = Vec::new();
                let mut chunk = [0u8; 4096];
                while let Ok(read @ 1..) = master.read(&mut chunk) {
                    bytes.extend_from_slice(&chunk[..read]);
                }
                bytes
            });
            let deadline = Instant::now() + Duration::from_secs(10);
            while child.try_wait().unwrap().is_none() {
                assert!(Instant::now() < deadline, "interactive review timed out");
                std::thread::sleep(Duration::from_millis(10));
            }
            let output = child.wait_with_output().unwrap();
            assert_success(&output);
            let screen = screen.join().unwrap();
            let screen = String::from_utf8_lossy(&screen);
            assert!(screen.contains("Accept change"), "{screen}");
            assert!(screen.contains("- foo"), "{screen}");
            assert!(screen.contains("+ bar"), "{screen}");
            assert_eq!(fs::read(&file).unwrap(), b"bar\n");
            screen.into_owned().into_bytes()
        }),
    }
}

#[cfg(unix)]
fn timed_bench_journey(b: Bencher, build: fn() -> Journey) {
    let mut journey = build();
    bench_with_syscall_trace(b, || black_box((journey.op)()));
}

#[rustybench::bench]
#[cfg(unix)]
fn cli_text_apply(b: Bencher) {
    timed_bench_journey(b, cli_text_apply_journey);
}

#[rustybench::bench]
#[cfg(unix)]
fn cli_binding_rename(b: Bencher) {
    timed_bench_journey(b, cli_rename_journey);
}

#[rustybench::bench]
#[cfg(unix)]
fn cli_saved_plan(b: Bencher) {
    timed_bench_journey(b, cli_saved_plan_journey);
}

#[rustybench::bench]
#[cfg(unix)]
fn cli_interactive_review(b: Bencher) {
    timed_bench_journey(b, cli_review_journey);
}

#[cfg(unix)]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Usage {
    utime_sec: u64,
    utime_usec: u64,
    stime_sec: u64,
    stime_usec: u64,
    maxrss: i64,
    ixrss: i64,
    idrss: i64,
    isrss: i64,
    minflt: i64,
    majflt: i64,
    nswap: i64,
    inblock: i64,
    oublock: i64,
    msgsnd: i64,
    msgrcv: i64,
    nsignals: i64,
    nvcsw: i64,
    nivcsw: i64,
}

#[cfg(unix)]
unsafe extern "C" {
    fn getrusage(who: i32, usage: *mut Usage) -> i32;
}

#[cfg(unix)]
const RUSAGE_SELF: i32 = 0;
#[cfg(unix)]
const RUSAGE_CHILDREN: i32 = -1;

#[cfg(unix)]
impl Usage {
    fn read(who: i32) -> Self {
        let mut usage = Self::default();
        assert_eq!(unsafe { getrusage(who, &mut usage) }, 0, "getrusage");
        usage
    }

    fn cpu_ms(&self) -> u64 {
        let seconds = self.utime_sec.saturating_add(self.stime_sec);
        let micros = self.utime_usec.saturating_add(self.stime_usec);
        seconds.saturating_mul(1000).saturating_add(micros / 1000)
    }
}

#[cfg(all(unix, target_os = "linux"))]
fn io_bytes() -> (u64, u64) {
    let text = fs::read_to_string("/proc/self/io").unwrap_or_default();
    let mut read = 0;
    let mut write = 0;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("rchar: ") {
            read = value.parse().unwrap_or(0);
        }
        if let Some(value) = line.strip_prefix("wchar: ") {
            write = value.parse().unwrap_or(0);
        }
    }
    (read, write)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn io_bytes() -> (u64, u64) {
    (0, 0)
}

#[cfg(unix)]
#[derive(Clone, Copy)]
struct Snapshot {
    own: Usage,
    children: Usage,
    allocations: (u64, u64),
    io: (u64, u64),
}

#[cfg(unix)]
fn snapshot() -> Snapshot {
    Snapshot {
        own: Usage::read(RUSAGE_SELF),
        children: Usage::read(RUSAGE_CHILDREN),
        allocations: ALLOC.snapshot(),
        io: io_bytes(),
    }
}

#[cfg(unix)]
fn attribution_record(name: &str, before: Snapshot, after: Snapshot, output: &[u8]) -> String {
    serde_json::json!({
        "workload": name,
        "oracle": "ok",
        "output_bytes": output.len(),
        "cpu_process_ms": after.own.cpu_ms().saturating_sub(before.own.cpu_ms()),
        "cpu_children_ms": after.children.cpu_ms().saturating_sub(before.children.cpu_ms()),
        "allocations": after.allocations.0.saturating_sub(before.allocations.0),
        "allocated_bytes": after.allocations.1.saturating_sub(before.allocations.1),
        "allocation_scope": "process-wide, includes worker threads",
        "io_read_bytes": after.io.0.saturating_sub(before.io.0),
        "io_write_bytes": after.io.1.saturating_sub(before.io.1),
        "io_bytes_scope": if cfg!(target_os = "linux") { "process bytes" } else { "unavailable" },
        "children_block_reads": after.children.inblock - before.children.inblock,
        "children_block_writes": after.children.oublock - before.children.oublock,
    })
    .to_string()
}

#[cfg(unix)]
fn attribute_journeys() {
    for build in [
        cli_text_apply_journey as fn() -> Journey,
        cli_rename_journey,
        cli_saved_plan_journey,
        cli_review_journey,
    ] {
        let mut journey = build();
        let before = snapshot();
        let output = (journey.op)();
        let after = snapshot();
        println!("{}", attribution_record(journey.name, before, after, &output));
    }
}

fn main() {
    if std::env::var_os("BENCH_ATTRIBUTIONS").is_some() {
        #[cfg(unix)]
        attribute_journeys();
        #[cfg(not(unix))]
        panic!("BENCH_ATTRIBUTIONS requires a unix host");
        return;
    }
    rustybench::main();
}
