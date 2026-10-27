Prefix: FAST
Next: 9

### FAST-1 · P1 · Interactive review panics or skips the next match

- **Area:** Data
- **Where:** `src/main.rs:704`, `src/main.rs:740`, `src/main.rs:749`, `src/main.rs:754`
- **Problem:** `present_and_apply_patches` keeps a byte cursor in the pre-replacement string, snaps it with `to_char_boundary` on that old string, then reloads the file and slices at that cursor. Accepting a change whose new byte length lands inside a later UTF-8 character panics, after the first write. Rejecting a match advances one byte past `mat.end()`, so a second match that starts on the next byte is never offered. `--accept-all` does not use this cursor. It calls `replace_all` in `fast_patch` and is fine. Probes with `target/debug/fastermod` before the comment-only edits: `printf 'y\n' | fastermod a bb` on `aé` wrote `bbé` and panicked (`start byte index 3 is not a char boundary`), exit 101. `printf 'n\ny\n' | fastermod foo bar` on `foofoo` prompted once and left `foofoo`, exit 0. The same `a` to `bb` replacement with `--accept-all` wrote `bbé` and exited 0. `printf 'y\ny\n'` on ASCII `foofoo` wrote `barbar`.
- **Recommendation:** Resume each search in the string that will actually be scanned. On accept, take the cursor from the post-replacement text. On reject, resume at `mat.end()`, and add one byte only when the match is empty. After `e` or `E`, start again at 0 on the reread file. Add the two probes above as tests.
- **Fix with:** none
- **Effort:** S

### FAST-2 · P2 · Failures exit 0

- **Area:** Errors
- **Where:** `src/main.rs:1147`, `src/main.rs:1041`, `src/main.rs:1117`, `src/main.rs:1012`
- **Problem:** `main` prints an error with `Debug` and returns, so the process exits 0. Fast mode prints a warning and continues, then returns `Ok`. A pattern that matches the empty string still waits for Enter when `--accept-all` is set, and EOF on that prompt is a failure that also exits 0. Probes: `fastermod '(' x .` printed `Unable to make regex from (` and exited 0. `fastermod --accept-all 'a*' x` on `aaa` with stdin closed left `aaa` and exited 0. `--accept-all foo BAR` on a directory with one mode-0444 file and one writable file updated only the writable file, printed `Unable to write to "ro/ro.txt": Permission denied`, and exited 0.
- **Recommendation:** Map `Err` from `fastermod` to a non-zero status. In `run_fast_impl`, keep walking after a file error if you want partial progress, but return that error so the process does not report success. Treat EOF on the empty-pattern prompt as a refusal. Add the three probes as exit-status tests.
- **Fix with:** error-model
- **Effort:** S

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

### FAST-5 · P3 · Interactive review queues every matching file

- **Area:** Concurrency
- **Where:** `src/main.rs:887`
- **Problem:** Interactive mode spawns a parallel walk that sends each matching path and its full contents on an unbounded `mpsc` channel. The user answers one prompt at a time, and the walk does not stop. On a large tree the queue holds the rest of the matching files. Pressing `A` leaves that channel open until `run_fast_impl` returns, so the first walk keeps filling it during the second pass. Memory growth was not measured. A stdin-closed probe exited before a sample.
- **Recommendation:** Bound the channel and block the walk when it is full, or read the next file only after the current prompt returns. Drop the sender when switching to fast mode so the first walk stops.
- **Fix with:** lifecycle-concurrency
- **Effort:** M

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
