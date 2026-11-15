//! Top-level request dispatch: CLI args -> engine -> typed plan -> output.

use std::io::IsTerminal;

use grep::regex::RegexMatcher;
use grep::regex::RegexMatcherBuilder;
use regex_lite::Regex;
use regex_lite::RegexBuilder;

use crate::cli::{Args, Mode, parse_args};
use crate::diff::{compute_diff, diff_line_span};
use crate::discovery::{FileSet, matching_files};
use crate::output::{languages_report, plan_json_report};
use crate::plan;
use crate::review::{Fastermod, prompt_reply_stderr};
use crate::text::Substitution;
use crate::{Failure, operational};

/// Preview (check/JSON/apply-with-JSON) or report on text-mode edits through
/// a typed edit plan. Plain `--apply`/`--accept-all` without `--json`
/// keeps the parallel fast path and never reaches here.
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
    let mut plan = plan::build_text_plan(
        Mode::Text.name(),
        regex,
        substitution,
        &files,
        roots.to_vec(),
    );
    let problems = plan::validate(&plan);
    if !problems.is_empty() {
        return Err(Failure::io(format!(
            "invalid edit plan: {}",
            problems.join("; ")
        )));
    }
    let files_edited = plan.coverage.files_edited;

    if json {
        let apply_result = if apply {
            Some(plan::apply(&mut plan))
        } else {
            None
        };
        if let Some(Err(e)) = &apply_result {
            plan.diagnostics.push(plan::Diagnostic {
                severity: plan::Severity::Error,
                code: "apply-failed".to_string(),
                message: e.to_string(),
                path: None,
            });
        }
        // The report is emitted even on apply failure: `applied` and the
        // per-edit `applied` flags report exactly which files were written.
        print!("{}", plan_json_report(&plan));
        if let Some(result) = apply_result {
            result?;
        }
        return Ok(if check && files_edited > 0 && !plan.applied {
            1
        } else {
            0
        });
    }

    if check {
        for file in &plan.files {
            println!("{}", file.path.display());
        }
        return Ok(if files_edited > 0 { 1 } else { 0 });
    }

    if apply {
        // Unreachable in practice (plain apply takes run_fast), kept so the
        // pipeline remains a complete apply path for future engines.
        plan::apply(&mut plan)?;
        return Ok(0);
    }

    // Preview: show diffs, never write.
    let mut fm = Fastermod::new(false, hidden, no_ignore, print_changed_files);
    for file in &plan.files {
        let diffs = fm.diffs_to_print(&file.before, &file.after);
        if diffs.is_empty() {
            continue;
        }
        let (start_line, end_line) =
            diff_line_span(&compute_diff(&file.before, &file.after));
        if start_line == end_line {
            println!("{}:{}", file.path.display(), start_line);
        } else {
            println!("{}:{}-{}", file.path.display(), start_line, end_line);
        }
        fm.print_diff(&diffs);
        if print_changed_files {
            fm.record_change(file.path.clone());
        }
    }
    fm.print_changed_files_if_needed();
    Ok(0)
}

pub(crate) fn fastermod() -> std::result::Result<i32, Failure> {
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
        plan: plan_file,
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
    for (unimplemented, flag) in [
        (mode == Mode::Ident, "--ident"),
        (mode == Mode::Rename, "--rename"),
        (mode == Mode::Rewrite, "--rewrite"),
        (mode == Mode::Rules, "--rules"),
        (mode == Mode::ApplyPlan, "--apply-plan"),
    ] {
        if unimplemented {
            return Err(Failure::analysis(format!(
                "{} is not yet implemented in this build",
                flag
            )));
        }
    }
    if plan_file.is_some() {
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
