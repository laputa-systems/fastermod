use std::process::exit;

use lexopt::prelude::*;

use crate::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Text,
    Ident,
    Rename,
    Rewrite,
    Rules,
    ApplyPlan,
    Languages,
}

impl Mode {
    pub(crate) fn name(self) -> &'static str {
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

    pub(crate) fn is_semantic(self) -> bool {
        !matches!(self, Mode::Text)
    }
}

pub(crate) struct Args {
    pub(crate) mode: Mode,
    pub(crate) multiline: bool,
    pub(crate) dir: Vec<String>,
    pub(crate) file_or_dir: Vec<String>,
    pub(crate) ignore_case: bool,
    pub(crate) smart_case: bool,
    pub(crate) extensions: Option<Vec<String>>,
    pub(crate) glob: Option<Vec<String>>,
    pub(crate) hidden: bool,
    pub(crate) no_ignore: bool,
    pub(crate) iglob: Option<Vec<String>>,
    pub(crate) accept_all: bool,
    pub(crate) apply: bool,
    pub(crate) dry_run: bool,
    pub(crate) check: bool,
    pub(crate) json: bool,
    pub(crate) plan: Option<String>,
    pub(crate) apply_plan: Option<String>,
    pub(crate) rules_file: Option<String>,
    pub(crate) at_anchor: Option<String>,
    pub(crate) langs: Vec<String>,
    pub(crate) print_changed_files: bool,
    pub(crate) fixed_strings: bool,
    pub(crate) match_regex: String,
    pub(crate) subst: String,
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
      --lang <LANG>         Force a language/grammar (repeatable). --ident/--rename/--rewrite/--rules.
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

pub(crate) fn parse_args() -> Args {
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
        langs.is_empty()
            || matches!(
                mode,
                Mode::Ident | Mode::Rewrite | Mode::Rules | Mode::Rename
            ),
        "--lang is only valid with --ident, --rename, --rewrite or --rules"
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
    })}
