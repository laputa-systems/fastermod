# Stateless refactoring engine — design notes

`fastermod` performs syntax-aware (`--ident`, `--rewrite`) and
binding-aware (`--rename`) edits plus declarative batch rules
(`--rules`) and portable plans (`--plan`/`--apply-plan`). Every
invocation rediscovers files, parses sources, and builds symbol graphs
in memory; there is no daemon, index, watcher, cache, LSP, or runtime
language dependency.

## Documents

- [cli.md](cli.md) — frozen CLI grammar: modes, flags, exit codes, JSON
  schema, review/apply semantics.
- [apply-safety.md](apply-safety.md) — plan validation, stale-plan
  rejection, atomic apply, filesystem fault behavior.
- [reuse.md](reuse.md) — REFAC-5 spike results: why ast-grep + oxc +
  per-language tree-sitter grammars were chosen (spike code removed
  after digestion; findings live here).
- [baseline.md](baseline.md) — pre-refactor verification baseline and
  benchmark deltas.
- [performance.md](performance.md) — whole-process release-build
  measurements and the no-optimization rationale.
- [qualification.md](qualification.md) — statelessness, portability,
  filesystem-fault, and license/linking evidence.

Language inventory: see [../languages.md](../languages.md) and the live
`fastermod --languages` matrix.

## Outcome

The stateless polyglot refactoring campaign completed with all planned
capability landed: ~30 binding analyzers, workspace rename for
Rust/Python/JS/TS, embedded-region edits for HTML/Vue/Svelte/Markdown,
versioned plans with full read-set staleness, and ordered TOML rules.
Remaining gaps are evidenced-open (no compatible grammar) or explicit
syntax-only rows — see `docs/languages.md`. Campaign history is in git;
the ledger was removed once closed.
