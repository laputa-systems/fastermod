# REFAC-14 performance evidence

Whole-process measurements, release build (`cargo build --release`),
macOS aarch64, corpus = this repository pinned at the measured commit
(`src/` = 48 Rust files, ~748 KiB; `.` adds tests/fixtures/docs).
`/usr/bin/time -l`, median of 5 warm runs (OS page cache warm).

| Workload | Median wall | Peak RSS |
|---|---|---|
| `--ident Parser Parsed --json src` | ~0.05 s | ~10.6 MB |
| `--ident Parser Parsed --json .` (whole repo) | ~0.10 s | ~11.2 MB |
| text `Parser Parsed --json src` | ~0.02 s | — |
| `--languages --json` | <0.01 s | — |
| `--rename content_hash hash_content src/plan.rs` | ~0.01 s (refused: ambiguous) | — |

Debug build comparison (`target/debug`): `--ident` over `src/` ≈ 0.15 s,
~13.5 MB RSS — release is ~3x faster, ~20% less RSS.

## Phase notes

- Whole-process cost is dominated by process startup + grammar static
  init, not per-file work: scanning `src/` vs the whole repo differs by
  only ~2x despite ~4x the files, and `--languages` (no parsing) is
  ~5x faster than a full `--ident` pass.
- Semantic `--rename` pays one tree-sitter parse per candidate file plus
  binding analysis; on single-file Rust it stays in the same order as
  `--ident` because unresolved-ref blocking exits early on ambiguity.
- No daemon, index, or cache exists to warm: every invocation pays full
  discovery+parse. That is the intended stateless contract; the numbers
  above are the cold-equivalent cost (page-cache-warm only).

## Tradeoffs

- No optimization was applied in this slice: measured costs are already
  bounded by grammar parse time and there is no profiled hot path that
  justifies bounded parallelism without changing the single-pass,
  deterministic plan ordering the CLI guarantees.
- `--rewrite` compiles the pattern once per registered language up
  front (~45 grammars); this is amortized across files and keeps pattern
  errors a usage error even on empty file sets.
