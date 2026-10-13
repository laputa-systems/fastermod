# Release builds

The manual [`release` workflow](../.github/workflows/release.yml) builds macOS
ARM64 and Linux x86-64 and ARM64 binaries. Linux builds include both static and
dynamic musl variants, and the workflow verifies them before publishing zipped
pre-release artifacts to GitHub Releases.

For local builds, `TARGET` selects the target triple. It defaults to the host;
the default replaces `-unknown-linux-gnu` with `-unknown-linux-musl`.
`make release` builds a static release binary. `make release-dynamic` builds
the dynamic variant used by the Linux workflow. The [`Makefile`](../Makefile)
provides `make verify-release` and `make verify-release-dynamic` to check the
corresponding Linux artifacts.

Linux release builds in the workflow run in the Alpine image defined by
[`Dockerfile`](../Dockerfile). The pinned nightly toolchain and required Rust
components are listed in [`rust-toolchain.toml`](../rust-toolchain.toml).
