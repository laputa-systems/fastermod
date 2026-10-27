Prefix: FAST
Next: 9

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
