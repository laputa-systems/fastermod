# Design notes

`fastermod` performs syntax-aware (`--ident`, `--rewrite`) and
binding-aware (`--rename`) edits plus declarative batch rules
(`--rules`) and portable plans (`--plan`/`--apply-plan`). Every
invocation rediscovers files, parses sources, and builds symbol graphs
in memory; there is no daemon, index, watcher, cache, LSP, or runtime
language dependency.

## Documents

- [cli.md](cli.md): CLI grammar, mode semantics, exit codes, JSON schema.
- [apply-safety.md](apply-safety.md): write mechanics, staleness and
  TOCTOU, links and file types, metadata policy, cancellation.
- [reuse.md](reuse.md): why ast-grep, oxc, and per-language tree-sitter
  grammars were chosen (dated spike record).
- [baseline.md](baseline.md): pre-refactor verification baseline (dated).
- [performance.md](performance.md): whole-process release-build
  measurements and the no-optimization rationale (dated).
- [qualification.md](qualification.md): statelessness, portability, and
  filesystem-fault evidence snapshot (dated).

Language inventory: see [../languages.md](../languages.md) and the live
`fastermod --languages` matrix.

## Outcome

All planned capability landed: binding analyzers for 28 languages,
workspace rename for Rust/Python/JS/TS, embedded-region edits for
HTML/Vue/Svelte/Markdown, versioned plans with full read-set staleness,
and ordered TOML rules. Remaining gaps are evidenced-open (no compatible
grammar) or explicit syntax-only rows; see `docs/languages.md`.
