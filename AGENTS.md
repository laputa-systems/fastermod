# fastermod

Rust CLI for reviewing or applying regular-expression substitutions across files.

## Commands

- Build: `make build`; test: `make test`; check: `cargo check --workspace`.
- `make lint` runs Clippy with warnings denied; inspect the Makefile before running it.

## Invariants

- Interactive review is the default; `--accept-all` enables unattended writes. See [CLI usage](README.md).
- Stateless: no daemon, persistent index, watcher, or LSP dependency; every invocation re-analyzes sources in memory.

## Verification

- Run `make test` and `make lint`. The macOS arm64 baseline at `fda1e8f` passed 42 tests. `make test` took 26.47s from a fresh target and 5.99s warm. `make lint` took 6.57s after tests. These are local suite observations under host load. PR CI uses both entry points on Linux arm64.
- CLI substitution, discovery, exit codes and offline help: `tests/cli.rs`. Binding-aware renames, embedded languages, JSON edit plans, and applied file bytes: `tests/fixtures.rs` and `tests/fixtures/`.
- Saved-plan validation and filesystem effects: `src/plan.rs` tests, including `apply_rejects_added_file_via_seen_fingerprint`.
- Performance workload: `make bench RUSTYBENCH=rustybench` uses `benches/bench.rs` and writes `benches/baseline.json`. Use a disposable checkout to collect a candidate measurement. The checked-in file covers search, patch, replace, diff and tree traversal. A quiet-host rerun is pending for `fda1e8f`.
- `imara-histogram-diff/` is a modified extraction from [imara-diff](https://github.com/pascalkuthe/imara-diff), present since the initial commit `f9b8969`. The import history does not identify an upstream revision. `LICENSE` records the Apache-2.0 terms.

## Topics

- CLI contract (modes, flags, exit codes, JSON schema): [docs/refactor/cli.md](docs/refactor/cli.md)
- Apply and write safety: [docs/refactor/apply-safety.md](docs/refactor/apply-safety.md)
- Language coverage and evidenced-open gaps: [docs/languages.md](docs/languages.md) and `fastermod --languages`
- Design notes: [docs/refactor/README.md](docs/refactor/README.md)
- Verification evidence (dated): [baseline](docs/refactor/baseline.md), [reuse](docs/refactor/reuse.md), [qualification](docs/refactor/qualification.md), [performance](docs/refactor/performance.md)
- Local development: [docs/development.md](docs/development.md)
- Release builds and artifacts: [docs/release.md](docs/release.md)
- License: [LICENSE](LICENSE) (dual MIT / Apache-2.0)
