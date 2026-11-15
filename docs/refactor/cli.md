# REFAC-2: CLI grammar and mode contracts (frozen)

Status: frozen. Implemented at `main`: full flag parsing and validation,
non-TTY preview, `--dry-run`/`--check`/`--json`/`--apply` for text mode,
`--languages`, frozen exit codes, and exit-3 stubs for `--ident`, `--rename`,
`--rewrite`, `--rules`, `--apply-plan`, `--plan` (engines land in
REFAC-6/7/8/12). Base `b00f2af`.

## Alternatives exercised

Three operand shapes were compared against realistic transcripts
(`fastermod OldName NewName src/`, `fastermod --rename foo bar . --at
src/lib.rs:42:9`, `fastermod --rewrite 'old($$$ARGS)' 'new($$$ARGS)' --lang
rust src/`, `fastermod --rules migration.toml . --dry-run`):

1. **Mode flags (chosen).** `fastermod [OPTIONS] <OLD> <NEW> [PATH...]` with
   mutually exclusive `--ident`, `--rename`, `--rewrite`, `--rules`,
   `--apply-plan`, `--languages`. Preserves the established operand shape,
   keeps every old invocation valid, and needs no reserved subcommands that
   could collide with regex operands named `rename` or `text`.
2. **Explicit subcommands** (`fastermod rename OLD NEW .`). Rejected: `rename`,
   `text`, `apply` are plausible regex operands; a subcommand grammar would
   silently reinterpret existing commands like `fastermod rename foo bar`.
3. **Single `--mode` value** (`--mode rename`). Rejected: one flag carrying a
   closed set is harder to validate per-mode (each mode has different
   operands/flags) and produces worse errors than dedicated flags.

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

Mode flags are mutually exclusive; at most one may appear. `--rules` and
`--apply-plan` and `--languages` take their file/operand as option values, so
positionals after them are roots (default `.`).

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

## Validation rules (usage errors, exit 2)

- More than one mode flag; missing/extra operands for the mode.
- `--at` without `--rename`; `--lang` outside `--rewrite`/`--rules`.
- `-i`, `-S`, `-m`, `-F` with `--ident`/`--rename`/`--rewrite`/`--rules`
  (regex/case options are meaningless there; rejected rather than ignored).
- `--plan` with `--apply`/`--accept-all`; `--check` with any apply flag or
  `--plan`; `--apply-plan` combined with other modes or positionals.
- `--json` with interactive review (JSON implies machine output; see below).
- `--at` anchor that is not `FILE:LINE:COL`, column off a char boundary, or
  OLD that does not match the selected symbol's name (exit 3 for the last).

## Interactivity and exit codes

Frozen exit codes:

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
  diffs and exits 0 without writing — it never hangs or writes. `--dry-run`
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
    {"path": "...", "byte_start": 0, "byte_end": 0,
     "anchor": {"line": 1, "column": 1, "excerpt": "..."},
     "kind": "text | ident | rewrite | rename-reference | ..."}
  ],
  "diagnostics": [{"severity": "error | warning | info", "code": "...",
                   "message": "...", "path": "..."}]
}
```

JSON never implies permission to mutate; `applied` reports what happened.
Truncation is forbidden: `files_skipped`/`unresolved` must be complete or the
run fails.

## `--languages` output

One typed capability registry (implemented in REFAC-6) generates both the
human matrix and `--languages --json`. Each row records grammar id+revision,
license, extensions/shebangs, identifier roles, structural rewrite, local
binding resolution, workspace/module resolution, symbol kinds, limitations and
fixture IDs. Until REFAC-6 lands the binary reports text mode only and lists
planned language rows as unsupported; a parser row is never reported as rename
support.
