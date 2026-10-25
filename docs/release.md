# Release builds

The manual [`release` workflow](../.github/workflows/release.yml) builds macOS
ARM64 and Linux x86-64 and ARM64 binaries. It checks that Linux binaries have
no ELF interpreter or shared-library dependency before publishing zipped
pre-release artifacts to GitHub Releases. Windows is not supported.

For local builds, `TARGET` selects the target triple. It defaults to the host;
the default replaces `-unknown-linux-gnu` with `-unknown-linux-musl`.
`make release` builds the release binary. The [`Makefile`](../Makefile)
provides `make verify-release` to check Linux artifacts for static linking.

Linux release builds in the workflow run in the Alpine image defined by
[`Dockerfile`](../Dockerfile). The pinned nightly toolchain and required Rust
components are listed in [`rust-toolchain.toml`](../rust-toolchain.toml).
