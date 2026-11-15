# fastermod

Rust CLI for reviewing or applying regular-expression substitutions across files.

## Commands

- Build: `make build`; test: `make test`; check: `cargo check --workspace`.
- `make lint` runs Clippy with warnings denied; inspect the current Makefile before running it.

## Invariants

- Interactive review is the default; `--accept-all` enables unattended writes. See [CLI usage](README.md).

## Campaigns

- Stateless polyglot refactoring: [campaign and launch prompt](docs/CAMPAIGN-STATELESS-REFACTOR.md), [execution ledger](TODO-REFACTOR.md). These describe planned work, not shipped features. Execute only when assigned; no daemon, persistent semantic index, or LSP dependency.

## Topics

- Local development: [docs/development.md](docs/development.md).
- Release builds and artifacts: [docs/release.md](docs/release.md).
- License notices: [docs/rfc/FAST-7-license-notices.md](docs/rfc/FAST-7-license-notices.md).
