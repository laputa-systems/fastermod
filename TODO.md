Prefix: FAST
Next: 9

### FAST-3 · P3 · Path suffix filter skips unrelated files

- **Area:** Data
- **Where:** `src/main.rs:338`
- **Problem:** `looks_like_code` drops any path whose lossy string ends in `~`, `tags`, or `TAGS`. That skips editor backups and a file named `tags`, and also `ctags`, `mytags`, and `file.tags`. Nothing is printed. A probe with `--accept-all foo bar` rewrote `keep.rs` to `bar` and left `ctags`, `mytags`, and `file.tags` as `foo`.
- **Recommendation:** Decide from the file name. Skip a name equal to `tags` or `TAGS`, and a name that ends in `~`. Test that `ctags`, `mytags`, and `file.tags` change, and that a file named `tags` does not.
- **Fix with:** none
- **Effort:** S

### FAST-4 · P3 · Accepted writes truncate before the new bytes land

- **Area:** Data
- **Where:** `src/main.rs:665`
- **Problem:** `save` uses `std::fs::write`. In nightly-2026-09-15 that function is `File::create` followed by `write_all`, and `File::create` truncates. A signal between the truncate and the completed write leaves an empty or partial source file. Both interactive accept and `fast_patch` go through `save`. The kill window was not reproduced inside `fastermod`. The truncate-on-create behavior is the standard library used by that call.
- **Recommendation:** Write a temporary file in the destination directory, `fsync` it, rename it over the target, and remove the temporary file if the write fails. Keep the existing contents if the rename does not happen.
- **Fix with:** os-boundaries
- **Effort:** S

### FAST-6 · P3 · Release toolchain and panic flags are copied

- **Area:** Build
- **Where:** `rust-toolchain.toml:2`, `Dockerfile:86`, `Makefile:11`, `Dockerfile:128`
- **Problem:** Local builds take `nightly-2026-09-15` from `rust-toolchain.toml`. The release image installs that same date itself, so a toolchain bump that edits only the manifest does not change Linux release builds. `-Cpanic=immediate-abort` and the other release `-Z` flags are set in `RELEASE_RUSTFLAGS` and again in the `musl-cargo` script. Those two lists can diverge. macOS `make release` and the Linux image would then not be the same panic mode.
- **Recommendation:** Copy `rust-toolchain.toml` into the image and install that toolchain. Build the `musl-cargo` release flags from the same list `Makefile` uses.
- **Fix with:** single-source-of-truth
- **Effort:** S

### FAST-7 · P3 · License files disagree with the notices in the tree

- **Area:** Docs
- **Where:** `LICENSE:1`, `Cargo.toml:10`, `src/main.rs:1`, `src/terminal.rs:1`, `imara-histogram-diff/Cargo.toml:5`, `imara-histogram-diff/src/lib.rs:5`
- **Problem:** `LICENSE` and the `fastermod` manifest say MIT. `src/main.rs` and `src/terminal.rs` still carry the Meta Apache-2.0 header. `imara-histogram-diff` says it was extracted from `imara-diff`, and both that crate and upstream `imara-diff` are Apache-2.0. The repo does not ship an Apache-2.0 license text. The histogram sources have no copyright notice. Publishing the repo or the release zips under MIT-only drops notices those files still claim.
- **Recommendation:** Keep the Apache headers until you confirm the code is no longer a derivative. Ship the Apache-2.0 text next to `LICENSE`, and stop describing the whole tree as MIT-only. See the draft RFC for the choice.
- **Fix with:** none
- **Design:** [docs/rfc/FAST-7-license-notices.md](docs/rfc/FAST-7-license-notices.md)
- **Effort:** S

### FAST-8 · P4 · Smartcase has no test

- **Area:** Tests
- **Where:** `src/main.rs:143`, `src/main.rs:1109`, `README.md:31`
- **Problem:** `--smartcase` is part of the documented contract and has no test. A probe of `-S tansu2 tansu --accept-all` on `tansu2 TANSU2 Tansu2 TaNsU2` wrote `tansu TANSU Tansu tansu`, which matches the README. A later edit to `adapt_case` or to the uppercase check would not fail CI.
- **Recommendation:** Add that input and output as an `--accept-all` test, including the mixed-case row that stays as `SUBST` was written.
- **Fix with:** none
- **Effort:** S
