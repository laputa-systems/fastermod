use std::borrow::Cow;
use std::cmp::max;
use std::cmp::min;
use std::collections::HashSet;
use std::env;
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
use ignore::WalkState;
use regex_lite::Regex;

use crate::diff::{DiffResult, compute_diff, diff_lines_in_range, diff_output_shape};
use crate::discovery::{FileSet, available_worker_threads, looks_like_code, walk_builder_with_file_set};
use crate::source::{display_warning, file_contents_if_matches, make_searcher, remember_first_error};
use crate::terminal::{self, Color};
use crate::text::{Substitution, backward_to_char_boundary, index_to_row_col, resume_after};
use crate::{Result, error_with_context};

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

pub(crate) fn prompt_reply_stderr(prompt: &str) -> std::io::Result<String> {
    let mut stderr = std::io::stderr();
    write!(stderr, "{}", prompt)?;
    stderr.flush()?;
    read_reply()
}

pub(crate) fn prompt(prompt_text: &str, letters: &str, default: Option<char>) -> Result<char> {
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

pub(crate) struct Fastermod {
    yes_to_all: bool,
    hidden: bool,
    no_ignore: bool,
    changed_files: Option<Vec<PathBuf>>,
}

impl Fastermod {
    pub(crate) fn new(
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

    pub(crate) fn save(&mut self, path: &Path, text: &str) -> Result<()> {
        // symlink_metadata so a symlink is rejected rather than followed:
        // discovery never yields links, and this is the last-line guard for
        // every write path (interactive, fast, plan apply).
        if let Ok(metadata) = fs::symlink_metadata(path) {
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                return Err(error_with_context(
                    format!("Unable to write to {:?}", path),
                    "refusing to write through a symbolic link",
                ));
            }
            if !file_type.is_file() {
                return Err(error_with_context(
                    format!("Unable to write to {:?}", path),
                    "refusing to write to a non-regular file",
                ));
            }
            if metadata.permissions().readonly() {
                return Err(error_with_context(
                    format!("Unable to write to {:?}", path),
                    std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "permission denied",
                    ),
                ));
            }
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

    pub(crate) fn record_change(&mut self, path: PathBuf) {
        if let Some(ref mut changed_files) = self.changed_files {
            changed_files.push(path);
        }
    }

    pub(crate) fn print_changed_files_if_needed(&mut self) {
        if let Some(ref mut changed_files) = self.changed_files {
            changed_files.sort();
            for file in changed_files {
                println!("{}", file.display());
            }
        }
    }

    pub(crate) fn fast_patch(
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

    pub(crate) fn diffs_to_print<'a>(&self, orig: &'a str, edit: &'a str) -> Vec<DiffResult<'a>> {
        let diff = compute_diff(orig, edit);
        let lines_to_print = match terminal::size() {
            Some((_w, h)) => h,
            None => 25,
        }
        .saturating_sub(20);

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

    pub(crate) fn print_diff<'a>(&mut self, diffs: &[DiffResult<'a>]) {
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

    pub(crate) fn run_interactive(
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
                        let entry = match file_contents_if_matches(&mut searcher, &matcher, path) {
                            Ok(Some(contents)) => Ok((path.to_path_buf(), contents)),
                            Ok(None) => return WalkState::Continue,
                            Err(error) => Err(error),
                        };
                        if tx.send(entry).is_err() {
                            return WalkState::Quit;
                        }
                    }
                    WalkState::Continue
                })
            })
        });

        let mut visited = HashSet::default();
        while let Ok(entry) = rx.recv() {
            let (path, contents) = entry?;
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
    pub(crate) fn run_fast(
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
    pub(crate) fn run_fast_impl(
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
        let substitution = Arc::new(substitution);
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
                    let contents = match file_contents_if_matches(&mut searcher, &matcher, path) {
                        Ok(Some(contents)) => contents,
                        Ok(None) => return WalkState::Continue,
                        Err(error) => {
                            remember_first_error(&file_error, error);
                            return WalkState::Continue;
                        }
                    };
                    let patching_result = fm.fast_patch(&regex, substitution.as_ref(), path, &contents);
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
#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::Write;
    use std::time::SystemTime;

    use regex_lite::RegexBuilder;

    use super::*;
    use crate::diff::DiffResult;

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
}
