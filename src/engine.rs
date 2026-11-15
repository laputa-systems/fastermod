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
use crate::syntax;
use crate::text::Substitution;
use crate::{Failure, operational};

/// Shared tail for every engine: validate, then JSON/check/apply/preview.
/// `roots` are the declared coverage roots used by plan::apply containment.
#[allow(clippy::too_many_arguments)]
fn finish_pipeline(
    mut plan: plan::EditPlan,
    roots: &[String],
    apply: bool,
    check: bool,
    json: bool,
    print_changed_files: bool,
    hidden: bool,
    no_ignore: bool,
) -> std::result::Result<i32, Failure> {
    let problems = plan::validate(&plan);
    if !problems.is_empty() {
        return Err(Failure::io(format!(
            "invalid edit plan: {}",
            problems.join("; ")
        )));
    }
    let files_edited = plan.coverage.files_edited;
    let _ = roots;

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

/// Text-mode entry: build the plan, then share the report/apply tail.
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
    let plan = plan::build_text_plan(
        Mode::Text.name(),
        regex,
        substitution,
        &files,
        roots.to_vec(),
    );
    finish_pipeline(
        plan,
        roots,
        apply,
        check,
        json,
        print_changed_files,
        hidden,
        no_ignore,
    )
}

/// Interactive per-file review for syntax modes: show each file's diff and
/// accept/reject it, then apply the accepted set as one plan. Mirrors the
/// text mode's y/n/A/q review at file granularity — --ident and --rewrite
/// edits are already whole-file consistent, so per-file review is the
/// correct unit (rename will review as one operation per cli.md).
fn interactive_syntax_review(
    mut plan: plan::EditPlan,
    hidden: bool,
    no_ignore: bool,
    print_changed_files: bool,
) -> std::result::Result<i32, Failure> {
    use crate::review::prompt;

    let mut fm = Fastermod::new(false, hidden, no_ignore, print_changed_files);
    let mut accepted = Vec::new();
    let mut yes_to_all = false;
    for file in std::mem::take(&mut plan.files) {
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
        let answer = if yes_to_all {
            'y'
        } else {
            match prompt(
                "Accept change (y = yes [default], n = no, A = yes to all, q = quit)?\n",
                "ynAq",
                Some('y'),
            ) {
                Ok(c) => c,
                Err(e) => {
                    return Err(Failure::io(format!("Unable to read user input: {e}")));
                }
            }
        };
        match answer {
            'y' => accepted.push(file),
            'A' => {
                yes_to_all = true;
                accepted.push(file);
            }
            'q' => {
                println!("Interrupted; {} file(s) accepted before quit.", accepted.len());
                break;
            }
            'n' => {}
            _ => unreachable!(),
        }
    }
    plan.files = accepted;
    plan.coverage.files_edited = plan.files.len();
    if plan.files.is_empty() {
        return Ok(0);
    }
    plan::apply(&mut plan)?;
    fm.print_changed_files_if_needed();
    Ok(0)
}

/// Discovery + engine dispatch for --ident and --rewrite.
#[allow(clippy::too_many_arguments)]
fn run_syntax_pipeline(
    mode: Mode,
    pattern: &str,
    replacement: &str,
    dirs: Vec<&str>,
    file_set: Option<FileSet>,
    hidden: bool,
    no_ignore: bool,
    print_changed_files: bool,
    apply: bool,
    dry_run: bool,
    check: bool,
    json: bool,
    interactive: bool,
    roots: &[String],
    langs: &[String],
) -> std::result::Result<i32, Failure> {
    // Discovery prefilter: --ident can require the literal OLD bytes;
    // --rewrite patterns have no reliable literal head, so every candidate
    // file is read (correctness first; REFAC-14 owns performance).
    let matcher = if mode == Mode::Ident {
        RegexMatcherBuilder::new()
            .build(&regex_lite::escape(pattern))
            .map_err(|e| Failure::usage(format!("Unable to make matcher: {e}")))?
    } else {
        RegexMatcherBuilder::new()
            .build("")
            .map_err(|e| Failure::usage(format!("Unable to make matcher: {e}")))?
    };
    let files = operational(matching_files(dirs, file_set, hidden, no_ignore, &matcher))?;
    let scanned = files.len();
    let run = match mode {
        Mode::Ident => syntax::run_ident(pattern, replacement, &files, langs)?,
        Mode::Rewrite => syntax::run_rewrite(pattern, replacement, &files, langs)?,
        _ => unreachable!(),
    };
    let plan = plan::assemble_plan(
        mode.name(),
        run.files,
        scanned,
        roots.to_vec(),
        run.skipped,
        run.diagnostics,
    );
    if interactive {
        return interactive_syntax_review(plan, hidden, no_ignore, print_changed_files);
    }
    let _ = dry_run; // dry_run already forced non-interactive; preview never writes.
    finish_pipeline(
        plan,
        roots,
        apply,
        check,
        json,
        print_changed_files,
        hidden,
        no_ignore,
    )
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
        (mode == Mode::Rename, "--rename"),
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
    let _ = (apply_plan, rules_file, at_anchor);

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

    // Syntax modes dispatch before any regex is compiled; `match_regex` and
    // `subst` still hold the raw OLD/NEW positionals here.
    if matches!(mode, Mode::Ident | Mode::Rewrite) {
        let apply = accept_all || apply;
        let interactive = !apply
            && !dry_run
            && !check
            && !json
            && std::io::stdin().is_terminal()
            && std::io::stdout().is_terminal();
        return run_syntax_pipeline(
            mode,
            &match_regex,
            &subst,
            dirs,
            file_set,
            hidden,
            no_ignore,
            print_changed_files,
            apply,
            dry_run,
            check,
            json,
            interactive,
            &roots,
            &langs,
        );
    }

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
