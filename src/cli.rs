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


struct OptionSpec {
    long: &'static str,
    short: Option<char>,
    value: &'static str,
    description: &'static str,
}

macro_rules! options {
    ($(($long:literal, $short:expr, $value:literal, $description:literal)),* $(,)?) => {
        const OPTIONS: &[OptionSpec] = &[$(OptionSpec {
            long: $long, short: $short, value: $value, description: $description,
        }),*];
    };
}

options! {
    ("ident", None, "", "Edit matching identifier tokens syntactically"),
    ("rename", None, "", "Rename a resolved binding and its references"),
    ("rewrite", None, "", "Rewrite structural patterns"),
    ("rules", None, "<FILE>", "Run declarative migration rules"),
    ("apply-plan", None, "<FILE>", "Apply a saved, versioned plan"),
    ("languages", None, "", "Print the capability matrix"),
    ("multiline", Some('m'), "", "Make regex dot match newlines"),
    ("dir", Some('d'), "<DIR>", "Search this directory (repeatable; default: current directory)"),
    ("ignore-case", Some('i'), "", "Search case-insensitively"),
    ("smartcase", Some('S'), "", "Match case-insensitively when OLD has no uppercase; adapt NEW casing"),
    ("extensions", Some('e'), "<EXT>", "Comma-delimited extensions (repeatable; conflicts with glob options)"),
    ("glob", Some('g'), "<GLOB>...", "Space-delimited globs (conflicts with extensions and iglob)"),
    ("hidden", None, "", "Search hidden files"),
    ("no-ignore", Some('u'), "", "Also search ignored files"),
    ("iglob", None, "<IGLOB>...", "Case-insensitive space-delimited globs (conflicts with extensions and glob)"),
    ("at", None, "<ANCHOR>", "Rename target FILE:LINE:COL (1-based line and UTF-8 byte column)"),
    ("lang", None, "<LANG>", "Force language for ident, rename, rewrite or rules (repeatable; default: detect)"),
    ("apply", None, "", "Write accepted changes without review"),
    ("accept-all", None, "", "Same as --apply (legacy spelling)"),
    ("dry-run", None, "", "Preview changes without writing"),
    ("check", None, "", "Exit 1 when changes are pending without writing"),
    ("json", None, "", "Emit machine-readable results"),
    ("plan", None, "<FILE>", "Preview and save a versioned plan"),
    ("print-changed-files", None, "", "Print changed paths"),
    ("fixed-strings", Some('F'), "", "Treat OLD as a literal string"),
    ("help", Some('h'), "", "Print help"),
    ("version", Some('V'), "", "Print version"),
}

struct ModeSpec {
    usage: &'static str,
    description: &'static str,
    example: &'static str,
}

const MODES: &[ModeSpec] = &[
    ModeSpec { usage: "[OPTIONS] <OLD> <NEW> [PATH...]", description: "Text substitution (default)", example: "fastermod old new src/" },
    ModeSpec { usage: "--ident [OPTIONS] <OLD> <NEW> [PATH...]", description: "Syntactic identifier replacement", example: "fastermod --ident old new src/" },
    ModeSpec { usage: "--rename [OPTIONS] <OLD> <NEW> [PATH...]", description: "Binding-aware rename; --at selects a target", example: "fastermod --rename old new src/" },
    ModeSpec { usage: "--rewrite [OPTIONS] <OLD> <NEW> [PATH...]", description: "Structural pattern rewrite; --lang selects a grammar", example: "fastermod --rewrite '$A + 0' '$A' src/" },
    ModeSpec { usage: "--rules <FILE> [OPTIONS] [PATH...]", description: "Run rules from a TOML file", example: "fastermod --rules rules.toml src/" },
    ModeSpec { usage: "--apply-plan <FILE> [OPTIONS]", description: "Revalidate and apply a saved plan", example: "fastermod --apply-plan plan.json" },
    ModeSpec { usage: "--languages [--json]", description: "Show language capabilities", example: "fastermod --languages --json" },
];

const FOOTER: &str = "Maintained by Joshua Li: https://github.com/laputa-systems/fastermod\nBugs and features: TODO.md in that repository (follow its AGENTS.md), or its issue tracker.\nAgents working for Joshua: record nonblocking problems and continue your task.\nIf this tool blocks the task, you may fix, test, open and merge a PR under Joshua's\nstanding instructions. Release only when needed to deliver the verified fix.\nProcedure: https://github.com/joshuarli/usr/blob/main/dotfiles/base/.agents/instructions/owned-tools.md";

fn help_text() -> String {
    use std::fmt::Write;
    let mut text = format!("fastermod {}\n\nfastermod assists with large-scale substitutions and refactors.\n\nUsage and modes (mutually exclusive):\n", env!("CARGO_PKG_VERSION"));
    for mode in MODES {
        writeln!(text, "  fastermod {}\n      {}", mode.usage, mode.description).unwrap();
    }
    text.push_str("\nArguments:\n  <OLD>      Required pattern, identifier, or structural pattern\n  <NEW>      Required replacement, new name, or structural template\n  [PATH...]  Optional, repeated search roots (default: current directory)\n\nOptions:\n");
    for option in OPTIONS {
        let short = option.short.map_or("    ".to_string(), |c| format!("-{c}, "));
        let value = if option.value.is_empty() { String::new() } else { format!(" {}", option.value) };
        writeln!(text, "  {short}--{}{value}\n      {}", option.long, option.description).unwrap();
    }
    text.push_str("\nRegex options -m, -i, -S and -F apply only to text mode.\n--plan conflicts with apply; --check conflicts with apply and plan;\n--dry-run conflicts with apply. --json cannot be used with --apply-plan.\nWithout --apply/--accept-all on non-terminal stdin, changes are previewed.\nEDITOR selects the review editor (default: vim). See docs/refactor/cli.md.\n\nExamples:\n");
    for mode in MODES {
        writeln!(text, "  {}", mode.example).unwrap();
    }
    write!(text, "\n{FOOTER}\n").unwrap();
    text
}

fn print_help_and_exit() {
    print!("{}", help_text());
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
    parse_with(lexopt::Parser::from_env())
}

fn parse_with(mut parser: lexopt::Parser) -> Result<Args> {
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

    while let Some(arg) = parser.next()? {
        let arg = match arg {
            Short(c) => OPTIONS.iter().find(|option| option.short == Some(c)).map_or(Short(c), |option| Long(option.long)),
            Long(name) => OPTIONS.iter().find(|option| option.long == name).map_or(Long(name), |option| Long(option.long)),
            value => value,
        };
        match arg {
            Long("help") => print_help_and_exit(),
            Long("version") => {
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
            Long("multiline") => multiline = true,
            Long("dir") => dir.push(parser.value()?.to_string_lossy().into_owned()),
            Long("ignore-case") => ignore_case = true,
            Long("smartcase") => smart_case = true,
            Long("extensions") => {
                let value = parser.value()?.to_string_lossy().into_owned();
                let mut exts: Vec<String> = value.split(',').map(str::to_string).collect();
                extensions.get_or_insert_with(Vec::new).append(&mut exts);
            }
            Long("glob") => {
                for value in parser.values()? {
                    has_glob = true;
                    glob.push(value.to_string_lossy().into_owned());
                }
            }
            Long("hidden") => hidden = true,
            Long("no-ignore") => no_ignore = true,
            Long("iglob") => {
                for value in parser.values()? {
                    has_iglob = true;
                    iglob.push(value.to_string_lossy().into_owned());
                }
            }
            Long("accept-all") => accept_all = true,
            Long("print-changed-files") => print_changed_files = true,
            Long("fixed-strings") => fixed_strings = true,
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

#[cfg(test)]
mod help_tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn help_examples_parse_and_option_names_are_unique() {
        let mut long = HashSet::new();
        let mut short = HashSet::new();
        for option in OPTIONS {
            assert!(long.insert(option.long), "duplicate option {}", option.long);
            if let Some(name) = option.short {
                assert!(short.insert(name), "duplicate short option {name}");
            }
        }
        for mode in MODES {
            let mut words = Vec::new();
            let mut word = String::new();
            let mut quoted = false;
            for c in mode.example.chars() {
                match c {
                    '\'' => quoted = !quoted,
                    ' ' if !quoted => {
                        if !word.is_empty() { words.push(std::mem::take(&mut word)); }
                    }
                    _ => word.push(c),
                }
            }
            assert!(!quoted);
            if !word.is_empty() { words.push(word); }
            assert_eq!(words.remove(0), "fastermod");
            assert!(parse_with(lexopt::Parser::from_args(words)).is_ok(), "{}", mode.example);
        }
    }
}
