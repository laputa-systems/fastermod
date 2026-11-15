# Vendored spike dependencies

`vendor/` was produced by `cargo vendor` in a scratch crate pinned to the
versions in `Cargo.lock` (real crates.io index, 2026-11-15). It is not
committed to git — regenerate with:

    cargo fetch && cargo vendor vendor

then restore the `[source.*]` replacement already present in
`.cargo/config.toml`. License inventory for the chosen crates is in
`docs/refactor/reuse.md`.
