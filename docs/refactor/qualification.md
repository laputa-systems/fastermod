# REFAC-13 qualification evidence

Statelessness, correctness and portability checks for the refactoring CLI.
All commands run against `target/debug/fastermod` on macOS (aarch64).

## Statelessness

- **Empty HOME/XDG**: `HOME=<empty> XDG_{CONFIG,CACHE,DATA}_HOME=<empty> fastermod --ident helper renamed --json .` completes with exit 0 and produces the full edit plan; zero files created under the empty dirs afterwards (verified with `find`).
- **No persistent state**: no `~/.fastermod`, `~/.cache/fastermod`, index files, sockets, or workspace registration are created by any mode. The only filesystem writes are (a) `--apply`/interactive-accept file edits through `plan::apply`, (b) `--plan FILE` output, (c) `--apply-plan` reads. `fs::write` call sites audited: `engine.rs` (--plan save only), `plan.rs` (apply path + tests).
- **No network/telemetry/LSP**: no HTTP client, socket, or subprocess dependencies in `src/` (grep for `Command`, `std::net`, `reqwest`, `tokio`: only `review.rs` `$EDITOR` spawn in interactive review mode — user-invoked, never in `--accept-all`/`--apply`/`--json`/`--plan` paths). All grammars are compiled in; no language-server or runtime (node/python/jvm) is invoked.
- **Fresh analysis per invocation**: every run re-discovers and re-parses; plans carry read-set + discovery + config fingerprints (`plan.rs`) so stale plans are rejected (exit 4) rather than replayed against changed trees.

## Correctness spot checks

- Read-only preview workspace (`chmod -R a-w`): `--accept-all` reports per-file write failures (`apply incomplete: 0 of 2 files written`) and writes nothing partial.
- Malformed inputs: `ident-parse-error` fixture plus per-language `has_error` gates skip unparseable files with explicit reasons instead of guessing.
- Semantic blockers: unresolved receivers/qualified access record unresolved refs and block rename (fixtures: rename-java, rename-csharp, rename-swift, rename-dart, rename-cpp, rename-fortran, rename-matlab, rename-workspace-*).
- Embedded edits: `embedded-ident`/`embedded-rewrite` fixtures prove host bytes/delimiters survive (`<script>`/`<style>`/fenced blocks).

## Portability

- Advertised targets: `aarch64-apple-darwin`, `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl` (`.github/workflows/release.yml` matrix; `make release` + `verify-release` asserts static linking, stripped, no INTERP/NEEDED).
- Local musl build not re-verified here (`musl-cargo` absent on this host); CI matrix owns that gate. Windows is not advertised.
- `cargo test`, `cargo check --workspace`, `cargo clippy --all-targets --all-features -- -D warnings` all green at commit `a4aadce`.

## Known honest gaps (ledger REFAC-10/11)

- Perl, Clojure, COBOL: no tree-sitter-0.27-compatible grammar crates — evidenced open.
- Objective-C: syntax-only (message receiver types unresolvable).
- OCaml-interface, F#-signature: syntax-only rows.
- Vue/Svelte host grammars, SQLite/MySQL dialects: evidenced open (symbol collision / ABI / no crate).
- `--rename` inside embedded regions: unsupported (per-file binding model).
