# Development

The Cargo workspace contains the `fastermod` executable and the local
`imara-histogram-diff` library. The Rust toolchain is pinned in
[`rust-toolchain.toml`](../rust-toolchain.toml).
The command definitions are in the [`Makefile`](../Makefile).

| Task | Command |
| --- | --- |
| Debug build | `make build` |
| Tests | `make test` |
| Check the workspace | `cargo check --workspace` |
| Lint | `make lint` |
| Install a local release build | `make install` |
| Run benchmarks | `make bench` |

`make install` builds a release binary and copies it to
`~/.local/bin/fastermod`; on macOS it also applies ad hoc code signing.
The `fastermod` library `include!`s `src/main.rs`, so `make bench` runs the
same code as the binary.

`make bench` uses `benches/baseline.json` and expects `../rustybench` by default.
Set `RUSTYBENCH` to override that command. See [release builds](release.md)
for target and packaging details.
