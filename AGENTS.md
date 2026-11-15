# fastermod

Rust CLI for reviewing or applying regular-expression substitutions across files.

## Commands

- Build: `make build`; test: `make test`; check: `cargo check --workspace`.
- `make lint` runs Clippy with warnings denied; inspect the Makefile before running it.

## Invariants

- Interactive review is the default; `--accept-all` enables unattended writes. See [CLI usage](README.md).
- Stateless: no daemon, persistent index, watcher, or LSP dependency; every invocation re-analyzes sources in memory.

## Topics

- CLI contract (modes, flags, exit codes, JSON schema): [docs/refactor/cli.md](docs/refactor/cli.md)
- Apply and write safety: [docs/refactor/apply-safety.md](docs/refactor/apply-safety.md)
- Language coverage and evidenced-open gaps: [docs/languages.md](docs/languages.md) and `fastermod --languages`
- Design notes: [docs/refactor/README.md](docs/refactor/README.md)
- Verification evidence (dated): [baseline](docs/refactor/baseline.md), [reuse](docs/refactor/reuse.md), [qualification](docs/refactor/qualification.md), [performance](docs/refactor/performance.md)
- Local development: [docs/development.md](docs/development.md)
- Release builds and artifacts: [docs/release.md](docs/release.md)
- License: [LICENSE](LICENSE) (dual MIT / Apache-2.0)
