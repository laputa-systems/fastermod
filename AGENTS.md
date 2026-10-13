# fastermod

Rust CLI for reviewing or applying regular-expression substitutions across files.

## Commands

- Build: `make build`; test: `make test`; check: `cargo check --workspace`.
- `make lint` runs Clippy with `--fix` and may edit source files.

## Invariants

- Interactive review is the default; `--accept-all` enables unattended writes. See [CLI usage](README.md).

## Topics

- Local development: [docs/development.md](docs/development.md).
- Release builds and artifacts: [docs/release.md](docs/release.md).
