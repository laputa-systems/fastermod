# CLI grammar and mode contracts

This document is the contract for `fastermod`'s command line: operand
shape, mode flags, validation, exit codes, and the JSON result schema.
`--help` prints the same grammar; this file adds the rules help cannot
carry.

## Grammar

```text
fastermod [OPTIONS] <OLD> <NEW> [PATH...]            # text mode (default)
fastermod --ident <OLD> <NEW> [PATH...]
fastermod --rename <OLD> <NEW> [PATH...] [--at FILE:LINE:COL]
fastermod --rewrite <PATTERN> <REPLACEMENT> [PATH...] [--lang LANG]...
fastermod --rules <RULES_FILE> [PATH...]
fastermod --apply-plan <PLAN_FILE>
fastermod --languages [--json]
```

Mode flags are mutually exclusive; at most one may appear. `--rules`,
`--apply-plan`, and `--languages` take their file or operand as option
values, so positionals after them are roots (default `.`). Mode flags
were chosen over subcommands because `rename`, `text`, and `apply` are
plausible regex operands.

### Carried-over options

`-m/--multiline`, `-d/--dir`, `-i/--ignore-case`, `-S/--smartcase`,
`-e/--extensions`, `-g/--glob`, `--hidden`, `-u/--no-ignore`,
`--iglob`, `-F/--fixed-strings`, `--print-changed-files`, `-h/--help`,
`-V/--version` keep their text-mode meaning.

### New options

| Flag | Meaning |
| --- | --- |
| `--ident` | Syntactic identifier-role edits; not binding-aware |
| `--rename` | Resolved binding rename; `--at` selects the declaration |
| `--rewrite` | Structural AST pattern rewrite; `--lang` forces a grammar |
| `--rules FILE` | Declarative multi-operation migration rules (data only) |
| `--at ANCHOR` | `FILE:LINE:COL`, 1-based line and 1-based UTF-8 byte column, char-boundary only; rename only |
| `--lang LANG` | Force language/grammar (repeatable); default per-file detection |
| `--apply` | Unattended write (alias spelling of `--accept-all`) |
| `--accept-all` | Existing unattended-write spelling; unchanged |
| `--dry-run` | Preview only; never writes (same as non-TTY default) |
| `--check` | Exit 1 when edits are pending; never writes |
| `--json` | Machine-readable result on stdout |
| `--plan FILE` | Preview and save a versioned plan; conflicts with apply |
| `--apply-plan FILE` | Apply a saved plan after revalidation |
| `--languages` | Print the capability matrix (`--json` for machine form) |

## Mode semantics

| Mode | Selection | Claim |
| --- | --- | --- |
| text (default) / `-F` | regex / literal bytes | text replacement, comments and strings included |
| `--ident` | language identifier roles + exact name | syntactic occurrences; bindings may differ |
| `--rewrite` | AST patterns, captures, constraints | structural transformation only |
| `--rename` | unique binding + resolved references | resolution-backed, reported domain |
| `--rules` | declarative per-language operations | ordered in-memory rewrites + renames |
| `--apply-plan` | versioned saved plan | revalidated explicit write |

There is no automatic fallback from `--rename` to `--ident` or text.
File discovery prefilters on the literal OLD spelling for text and
`--ident` only; `--rewrite` and `--rename` read every candidate file
because patterns and cross-file dependencies have no reliable literal
head.

## Validation rules (usage errors, exit 2)

- More than one mode flag; missing/extra operands for the mode.
- `--at` without `--rename`; `--lang` outside `--ident`/`--rename`/`--rewrite`/`--rules`.
- `-i`, `-S`, `-m`, `-F` with `--ident`/`--rename`/`--rewrite`/`--rules`
  (regex/case options are meaningless there; rejected rather than ignored).
- `--plan` with `--apply`/`--accept-all`; `--check` with any apply flag or
  `--plan`; `--apply-plan` combined with other modes or positionals.
- `--json` with interactive review (JSON implies machine output; see below).
- `--at` anchor that is not `FILE:LINE:COL`, column off a char boundary, or
  OLD that does not match the selected symbol's name (exit 3 for the last).

## Interactivity and exit codes

Exit codes:

| Code | Meaning |
| --- | --- |
| 0 | success: applied, previewed, checked-clean, or informational |
| 1 | `--check` found pending changes |
| 2 | usage error (flags, operands, anchors, regex compile) |
| 3 | analysis could not complete safely: ambiguous target, unsupported construct, unresolved binding |
| 4 | I/O or application failure (read/write/stat, stale plan, permission) |

Rules:

- Interactive per-match review requires stdin to be a TTY **and** no apply
  flag. On a non-TTY stdin without `--apply`/`--accept-all`, the tool previews
  diffs and exits 0 without writing; it never hangs or writes. `--dry-run`
  forces this on a TTY too.
- `--json` selects machine output; review prompts are unavailable, so JSON
  runs preview unless `--apply`/`--accept-all` is given.
- A `--rename` is reviewed as **one** operation: a single accept/reject for
  the whole edit set; rejecting rejects the rename.
- `--plan` previews and writes a versioned plan file; it cannot be combined
  with application. `--apply-plan` revalidates fingerprints and rejects stale
  plans (exit 4) rather than silently replanning.

## JSON result schema (v1)

```json
{
  "schema": 1,
  "mode": "text | ident | rewrite | rename | rules | apply-plan | languages",
  "applied": false,
  "operation_id": "optional stable id",
  "coverage": {
    "roots": ["."],
    "files_scanned": 0,
    "files_edited": 0,
    "files_skipped": [{"path": "...", "reason": "..."}],
    "unresolved": [{"path": "...", "detail": "..."}]
  },
  "edits": [
    {"path": "...", "byte_start": 0, "byte_end": 0, "applied": false,
     "anchor": {"line": 1, "column": 1, "excerpt": "..."},
     "kind": "text | ident | rewrite | rename-reference | mixed | ..."}
  ],
  "diagnostics": [{"severity": "error | warning | info", "code": "...",
                   "message": "...", "path": "..."}]
}
```

JSON never implies permission to mutate; `applied` reports what happened.
Top-level `applied` is true only when every planned file was written; the
per-edit `applied` flag records each file's outcome so a partial apply is
visible in the result, not just the exit code. Apply safety, link and
metadata policy, and the residual TOCTOU model are specified in
`docs/refactor/apply-safety.md`.
Truncation is forbidden: `files_skipped`/`unresolved` must be complete or the
run fails.

## `--languages` output

One typed capability registry (`src/lang.rs`)
generates both the human matrix and `--languages --json`. Each row records
grammar bundle id, license, extensions, identifier roles, structural
rewrite support, binding-resolution level, symbol kinds, limitations and
fixture IDs. A parser row is never reported as rename support: `binding`
is `"none"` for languages whose resolver has not landed; `"local"` marks
single-file binding renames and `"workspace"` marks cross-file
module-graph rename scope (rust, python, javascript, typescript, tsx).
