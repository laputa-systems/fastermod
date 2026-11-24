# fastermod

Rust CLI for reviewing or applying regular-expression substitutions across files.

The roadmap, [docs/roadmap.md](docs/roadmap.md), holds all current and future work; its Status is the resume point.

## Commands

- Build: `make build`; test: `make test`; check: `cargo check --workspace`.
- `make lint` runs Clippy with warnings denied; inspect the Makefile before running it.

## Invariants

- Interactive review is the default; `--accept-all` enables unattended writes. See [CLI usage](README.md).
- Stateless: no daemon, persistent index, watcher, or LSP dependency; every invocation re-analyzes sources in memory.

## Verification

`make clean` removes the checkout's ordinary `./target` directory. It refuses a file or symlink at that path.

- Run `make test` and `make lint`. The macOS arm64 baseline at `fda1e8f` passed 42 tests; 49 pass now. `make test` took 26.47s from a fresh target and 5.99s warm. `make lint` took 6.57s after tests. These are local suite observations under host load. PR CI uses both entry points on Linux arm64.
- Test tiers: selected `cargo test` case (Edit); `make test` and `make lint` (Change); hosted mutation controls (Scenario). Ground warm cost was 5.99s; adding the review case cost a median +0.09s CPU (6 interleaved pairs, ratio 1.04).
- CLI substitution, discovery, exit codes and offline help: `tests/cli.rs`. Binding-aware renames, embedded languages, JSON edit plans, and applied file bytes: `tests/fixtures.rs` and `tests/fixtures/`.
- Interactive review replies (y, n, default, unknown, q, end of input) on a pseudo-terminal: `tests/cli.rs::interactive_review_applies_each_reply_to_its_own_match`. Piped input never enters review.
- Saved-plan validation and filesystem effects: `src/plan.rs` tests, including `apply_rejects_added_file_via_seen_fingerprint`.
- Performance workload: `make bench RUSTYBENCH=rustybench` uses `benches/bench.rs` and writes `benches/baseline.json`. Use a disposable checkout to collect measurements. Tiered selection (`make bench-list`, `bench-check`, `bench-tier`; Full stays manual) is in [docs/bench-tiers.md](docs/bench-tiers.md).
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
