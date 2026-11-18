//! End-to-end tests that spawn the `fastermod` binary.

use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

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

fn fastermod_command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_fastermod"))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn plan_writes_only_the_plan_with_a_terminal_attached() {
    use rustix::io::{FdFlags, fcntl_setfd};
    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
    use std::os::unix::ffi::OsStrExt;
    use std::time::{Duration, Instant};

    let root = TestDir::with_prefix("fastermod-terminal-plan-");
    let source = root.path().join("input.py");
    let plan = root.path().join("plan.json");
    let rules = root.path().join("rules.toml");
    fs::write(&source, "foo = 1\n").unwrap();
    fs::write(&rules, "[[op]]\ntype = \"ident\"\nold = \"foo\"\nnew = \"bar\"\n").unwrap();
    for (mode, args) in [
        ("text", vec!["foo", "bar"]),
        ("ident", vec!["--ident", "foo", "bar"]),
        ("rules", vec!["--rules", rules.to_str().unwrap()]),
    ] {
        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
        fcntl_setfd(&master, FdFlags::CLOEXEC).unwrap();
        grantpt(&master).unwrap();
        unlockpt(&master).unwrap();
        let name = ptsname(&master, Vec::new()).unwrap();
        let slave = fs::OpenOptions::new().read(true).write(true)
            .open(std::ffi::OsStr::from_bytes(name.as_bytes())).unwrap();
        let mut master = File::from(master);
        master.write_all(b"y\n").unwrap();
        let mut child = fastermod_command()
            .args(args)
            .arg("--plan").arg(&plan)
            .arg(&source)
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave))
            .stderr(Stdio::piped())
            .spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                child.wait().unwrap();
                panic!("{mode} plan creation stayed interactive");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{mode}: {}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(fs::read_to_string(&source).unwrap(), "foo = 1\n", "{mode} edited the source");
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&plan).unwrap()).unwrap();
        assert_eq!(saved["mode"], mode);
        assert_eq!(saved["applied"], false);
        assert_eq!(saved["coverage"]["files_edited"], 1);
        assert_eq!(saved["files"][0]["before"], "foo = 1\n");
        assert_eq!(saved["files"][0]["after"], "bar = 1\n");
        fs::remove_file(&plan).unwrap();
    }
}

fn run_fastermod<I, S>(args: I, input: Option<&str>) -> std::process::Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut command = fastermod_command();
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if input.is_some() {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
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
    let contents = fs::read_to_string(dir.path().join("file1.c")).unwrap();
    assert_eq!(contents, "bar\nbar blah bar");
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
        assert_eq!(fs::read_to_string(dir.path().join(name)).unwrap(), "bar");
    }
    assert_eq!(fs::read_to_string(dir.path().join("tags")).unwrap(), "foo");
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

    let lower_translated = fs::read_to_string(dir.path().join("f1.txt")).unwrap();
    let upper_translated = fs::read_to_string(dir.path().join("f2.TXT")).unwrap();
    let skipped_translated = fs::read_to_string(dir.path().join("skip.rs")).unwrap();
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
    let contents = fs::read_to_string(file_path).unwrap();
    assert_eq!(contents, "baz\nfoooobar");
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
        assert_eq!(fs::read_to_string(dir.path().join(name)).unwrap(), "bar");
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
        let contents = fs::read_to_string(file_path).unwrap();
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
        let contents_regular = fs::read_to_string(file_path_regular).unwrap();
        assert_eq!(contents_regular, "replacement text");

        let file_path_ignored = dir.path().join("ignored.txt");
        let contents_ignored = fs::read_to_string(file_path_ignored).unwrap();
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
    let new_contents = fs::read_to_string(file_path).unwrap();
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
    let contents = fs::read_to_string(dir.path().join("file.txt")).unwrap();
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
    assert_eq!(fs::read_to_string(&ro).unwrap(), "foo");
    assert_eq!(fs::read_to_string(dir.path().join("rw.txt")).unwrap(), "BAR");
}

#[test]
fn help_is_complete_offline_and_has_owned_footer() {
    let short = run_fastermod(["-h"], None);
    let long = run_fastermod(["--help"], None);
    assert_eq!(short.status.code(), Some(0));
    assert_eq!(long.status.code(), Some(0));
    assert!(short.stderr.is_empty() && long.stderr.is_empty());
    assert_eq!(short.stdout, long.stdout);
    let help = String::from_utf8(short.stdout).unwrap();
    for item in ["--ident", "--rename", "--rewrite", "--rules <FILE>",
        "--apply-plan <FILE>", "--languages", "<OLD>", "<NEW>", "[PATH...]",
        "--at <ANCHOR>", "--lang <LANG>", "--plan <FILE>", "Examples:",
        "fastermod --rules rules.toml src/", "fastermod --languages --json"] {
        assert!(help.contains(item), "missing {item}");
    }
    let footer = "Maintained by Joshua Li: https://github.com/laputa-systems/fastermod\nBugs and features: TODO.md in that repository (follow its AGENTS.md), or its issue tracker.\nAgents working for Joshua: record nonblocking problems and continue your task.\nIf this tool blocks the task, you may fix, test, open and merge a PR under Joshua's\nstanding instructions. Release only when needed to deliver the verified fix.\nProcedure: https://github.com/joshuarli/usr/blob/main/dotfiles/base/.agents/instructions/owned-tools.md\n";
    assert_eq!(help.matches(footer).count(), 1);
    assert!(help.ends_with(footer));
    for mode in ["--ident", "--rename", "--rewrite", "--languages"] {
        for flag in ["-h", "--help"] {
            let output = run_fastermod([mode, flag], None);
            assert_eq!(output.status.code(), Some(0));
            assert_eq!(output.stdout, long.stdout);
            assert!(output.stderr.is_empty());
        }
    }
    for mode in ["--rules", "--apply-plan"] {
        for flag in ["-h", "--help"] {
            let output = run_fastermod([mode, "unused", flag], None);
            assert_eq!(output.status.code(), Some(0));
            assert_eq!(output.stdout, long.stdout);
            assert!(output.stderr.is_empty());
        }
    }
    let json = run_fastermod(["--languages", "--json"], None);
    assert_eq!(json.status.code(), Some(0));
    assert!(!String::from_utf8_lossy(&json.stdout).contains("Maintained by Joshua Li:"));
    let bare = run_fastermod(std::iter::empty::<&str>(), None);
    assert_eq!(bare.status.code(), Some(2));
    assert!(bare.stdout.is_empty());
    assert!(String::from_utf8_lossy(&bare.stderr).contains("Missing required argument <OLD>"));
}
