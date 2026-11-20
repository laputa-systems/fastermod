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

#[test]
fn saved_plan_revalidates_the_read_set_before_writing() {
    for (case, expected_error) in [
        ("unchanged", None),
        ("edited", Some("files changed since analysis")),
        ("unedited", Some("analyzed files changed since analysis")),
        ("added", Some("files added or removed since analysis")),
        ("removed", Some("files added or removed since analysis")),
    ] {
        let dir = TestDir::with_prefix("fastermod-plan-read-set-");
        let source = dir.path().join("input.py");
        let plan = dir.path().join("plan.py");
        let nested = dir.path().join("nested");
        let unedited = nested.join("plan.py");
        fs::create_dir(&nested).unwrap();
        fs::write(&source, "foo = 1\n").unwrap();
        fs::write(&unedited, "# foo\n").unwrap();
        let preview = fastermod_command()
            .args(["--ident", "foo", "bar", "--plan"])
            .arg(&plan)
            .arg(dir.path())
            .output()
            .unwrap();
        assert!(
            preview.status.success(),
            "{case}: {}",
            String::from_utf8_lossy(&preview.stderr)
        );
        assert_eq!(fs::read_to_string(&source).unwrap(), "foo = 1\n");

        match case {
            "edited" => fs::write(&source, "foo = 2\n").unwrap(),
            "unedited" => fs::write(&unedited, "# foo changed\n").unwrap(),
            "added" => fs::write(dir.path().join("new.py"), "new\n").unwrap(),
            "removed" => fs::remove_file(&unedited).unwrap(),
            _ => {}
        }
        let bytes_before = fs::read(&source).unwrap();
        let applied = fastermod_command()
            .arg("--apply-plan")
            .arg(&plan)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&applied.stderr);
        if let Some(error) = expected_error {
            assert_eq!(applied.status.code(), Some(4), "{case}: {stderr}");
            assert!(stderr.contains(error), "{case}: {stderr}");
            assert_eq!(fs::read(&source).unwrap(), bytes_before, "{case}");
        } else {
            assert!(applied.status.success(), "{case}: {stderr}");
            assert_eq!(fs::read_to_string(&source).unwrap(), "bar = 1\n");
            assert_eq!(fs::read_to_string(&unedited).unwrap(), "# foo\n");
        }
    }
}

#[test]
fn text_commands_report_matching_source_decode_failure() {
    let dir = TestDir::with_prefix("fastermod-source-error-");
    let source = dir.path().join("invalid.txt");
    let invalid = b"foo\xff\n";
    fs::write(&source, invalid).unwrap();
    for mode in ["--check", "--apply", "--dry-run"] {
        let output = run_fastermod([mode, "foo", "bar", source.to_str().unwrap()], None);
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(output.status.code(), Some(4), "{mode}: {stderr}");
        assert!(stderr.contains("invalid.txt") && stderr.contains("UTF-8"), "{stderr}");
        assert_eq!(fs::read(&source).unwrap(), invalid);
    }
}

#[test]
fn text_commands_skip_nul_detected_binary_input() {
    let dir = TestDir::with_prefix("fastermod-binary-source-");
    let source = dir.path().join("binary.dat");
    let binary = b"\0foo\xff\n";
    fs::write(&source, binary).unwrap();
    for mode in ["--check", "--apply"] {
        assert_fastermod([mode, "foo", "bar", source.to_str().unwrap()], None, Some(""));
        assert_eq!(fs::read(&source).unwrap(), binary);
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

/// Run fastermod with a pseudo-terminal on stdin and stdout, feed it `replies`
/// up front, and return its exit status, terminal output and stderr.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn run_on_terminal(args: &[&str], replies: &str) -> (std::process::ExitStatus, String, String) {
    use rustix::io::{FdFlags, fcntl_setfd};
    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
    use std::io::Read;
    use std::os::unix::ffi::OsStrExt;
    use std::time::{Duration, Instant};

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
    let mut master = File::from(master);
    master.write_all(replies.as_bytes()).unwrap();
    let mut child = fastermod_command()
        .args(args)
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave))
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let screen = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 4096];
        while let Ok(read @ 1..) = master.read(&mut chunk) {
            bytes.extend_from_slice(&chunk[..read]);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            child.wait().unwrap();
            panic!("interactive review did not finish with replies {replies:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    let screen = screen.join().unwrap();
    (output.status, screen, String::from_utf8_lossy(&output.stderr).into_owned())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn interactive_review_applies_each_reply_to_its_own_match() {
    let source = "foo 1\nfoo 2\nfoo 3\n";
    // One reply per line: n declines, a blank line takes the default (yes),
    // an unknown reply is asked again, and q quits with success.
    for (replies, expected, prompts, retries) in [
        ("n\ny\nn\n", "foo 1\nbar 2\nfoo 3\n", 3, 0),
        ("\nx\nn\ny\n", "bar 1\nfoo 2\nbar 3\n", 4, 1),
        ("y\nq\n", "bar 1\nfoo 2\nfoo 3\n", 2, 0),
    ] {
        let dir = create_test_files(&[("a.txt", source)]);
        let (status, screen, stderr) =
            run_on_terminal(&["foo", "bar", dir.path().to_str().unwrap()], replies);
        assert!(status.success(), "{replies:?}: {stderr}");
        assert_eq!(fs::read_to_string(dir.path().join("a.txt")).unwrap(), expected, "{replies:?}");
        assert_eq!(screen.matches("Accept change").count(), prompts, "{replies:?}: {screen}");
        assert_eq!(screen.matches("Come again?").count(), retries, "{replies:?}: {screen}");
    }

    // Closing the terminal input (Ctrl-D) before the second reply fails the
    // review and leaves the unanswered matches alone.
    let dir = create_test_files(&[("a.txt", source)]);
    let (status, _, stderr) =
        run_on_terminal(&["foo", "bar", dir.path().to_str().unwrap()], "y\n\x04");
    assert!(!status.success(), "a missing reply must fail");
    assert!(stderr.contains("Unable to read user input"), "{stderr}");
    assert_eq!(fs::read_to_string(dir.path().join("a.txt")).unwrap(), "bar 1\nfoo 2\nfoo 3\n");
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
fn json_apply_observes_the_full_ignore_configuration() {
    for no_ignore in [false, true] {
        let root = TestDir::with_prefix("fastermod-ignore-sources-");
        let scan = root.path().join("scan");
        let xdg = root.path().join("xdg");
        let home = root.path().join("home");
        fs::create_dir(&scan).unwrap();
        fs::create_dir_all(xdg.join("git")).unwrap();
        fs::create_dir(&home).unwrap();
        let config = home.join(".gitconfig");
        fs::write(&config, "").unwrap();
        let init = Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(root.path())
            .env("GIT_CONFIG_GLOBAL", &config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_CONFIG_COUNT")
            .output()
            .unwrap();
        assert!(init.status.success(), "{}", String::from_utf8_lossy(&init.stderr));
        fs::write(root.path().join(".ignore"), "parent.txt\n").unwrap();
        fs::write(scan.join(".ignore"), "local.txt\n").unwrap();
        fs::write(scan.join(".gitignore"), "tracked-ignore.txt\n").unwrap();
        fs::write(root.path().join(".git/info/exclude"), "exclude.txt\n").unwrap();
        fs::write(xdg.join("git/ignore"), "global.txt\n").unwrap();
        let ignored = ["parent.txt", "local.txt", "tracked-ignore.txt", "exclude.txt", "global.txt"];
        for name in ignored.into_iter().chain(["visible.txt", ".hidden.txt"]) {
            fs::write(scan.join(name), "old content\n").unwrap();
        }

        let mut command = fastermod_command();
        command.args(["--apply", "--json", "old content", "new content"]).arg(&scan)
            .current_dir(root.path())
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &xdg)
            .env("GIT_CONFIG_GLOBAL", &config)
            .env("GIT_CONFIG_NOSYSTEM", "1");
        if no_ignore {
            command.arg("--no-ignore");
        }
        let output = command.output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(fs::read_to_string(scan.join("visible.txt")).unwrap(), "new content\n");
        assert_eq!(fs::read_to_string(scan.join(".hidden.txt")).unwrap(), "old content\n");
        for name in ignored {
            let expected = if no_ignore { "new content\n" } else { "old content\n" };
            assert_eq!(fs::read_to_string(scan.join(name)).unwrap(), expected, "{name}");
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
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "permission denied write exited {:?}: {}",
        output.status.code(),
        stderr
    );
    assert!(
        stderr
            .lines()
            .any(|line| line.starts_with("Warning: ") && line.contains("ro.txt")),
        "{stderr}"
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

fn assert_scoped_rename(extension: &str, marked: &str) {
    let root = TestDir::with_prefix("fastermod-scope-");
    let path = root.path().join(format!("input.{extension}"));
    let at = marked.find('@').unwrap();
    let prefix = &marked[..at];
    let line = prefix.bytes().filter(|&b| b == b'\n').count() + 1;
    let column = prefix.rsplit('\n').next().unwrap().len() + 1;
    let source = marked.replace(['@', '#'], "");
    let expected = marked.replace("@item", "renamed").replace("#item", "renamed");
    fs::write(&path, source).unwrap();
    let location = format!("input.{extension}:{line}:{column}");
    let output = fastermod_command().current_dir(root.path())
        .args(["--rename", "item", "renamed", "--at", &location, "--apply", "."])
        .output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(fs::read_to_string(path).unwrap(), expected);
}

#[test]
fn c_compound_scope_rename() {
    assert_scoped_rename("c", "int f(void) { int item = 1; { int @item = 2; #item++; } return item; }\n");
    assert_scoped_rename("c", "int f(void) { int @item = 1; { int item = 2; item++; } return #item; }\n");
}

#[test]
fn c_for_scope_rename() {
    assert_scoped_rename("c", "int f(void) { int item = 1; for (int @item = 0; #item < 3; #item++) {} return item; }\n");
    assert_scoped_rename("c", "int f(void) { int @item = 1; for (int item = 0; item < 3; item++) {} return #item; }\n");
}

#[test]
fn c_struct_scope_rename() {
    assert_scoped_rename("c", "int @item; struct S { int item; }; int f(void) { return #item; }\n");
    assert_scoped_rename("c", "int item; struct S { int @item; }; int f(void) { return item; }\n");
}

#[test]
fn groovy_class_scope_rename() {
    assert_scoped_rename("groovy", "def @item = 1;\nclass C { int item; }\n#item\n");
    assert_scoped_rename("groovy", "def item = 1;\nclass C { int @item; }\nitem\n");
}

#[test]
fn groovy_method_scope_rename() {
    assert_scoped_rename("groovy", "class C { int item; int f(int @item) { return #item; } int g() { return item; } }\n");
    assert_scoped_rename("groovy", "class C { int @item; int f(int item) { return item; } int g() { return #item; } }\n");
}

#[test]
fn groovy_closure_scope_rename() {
    assert_scoped_rename("groovy", "def item = 1\ndef f = { def @item = 2; #item }\nitem\n");
    assert_scoped_rename("groovy", "def @item = 1\ndef f = { def item = 2; item }\n#item\n");
}

#[test]
fn groovy_for_scope_rename() {
    assert_scoped_rename("groovy", "def item = 1\nfor (int @item = 0; #item < 3; #item++) {}\nitem\n");
    assert_scoped_rename("groovy", "def @item = 1\nfor (int item = 0; item < 3; item++) {}\n#item\n");
}

