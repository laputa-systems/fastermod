Prefix: DSLP
Next: 10

Deslop campaign backlog ([plan](https://github.com/laputa-systems/stuff/blob/main/docs/campaigns/deslop.md), tier Calibration). One agent works this file with the `deslop` skill: take the lowest-numbered open ticket whose area does not start with `Needs owner ·`. After DSLP-5, work the open P1 and P2 findings, then fill in `## Report` below.

## Report

Ground at `fda1e8fa0d1be5c8933f319d749c211b0c5f5fec`: 42 tests and
Clippy passed on macOS arm64 with nightly `5b7b37aef`. The
[Verification section](AGENTS.md#verification) maps interfaces to tests and
records recent local suite times. Both Linux arm64 PR checks passed for
[#27](https://github.com/laputa-systems/fastermod/pull/27), which changed only
the verification documentation.

The same source ran `make bench RUSTYBENCH=rustybench` with the installed
rustybench 0.4.0 driver and the locked 0.5.0 harness. The macOS arm64 host
passed `perf-gate-ready` before the run and `perf-gate-ready -1` after it.
The run used one warmup and three measured passes, with 171.08s total wall
time. Keep this as a baseline for later paired measurements. The earlier
`benches/baseline.json` remains the configured reference.

| Scenario | Run medians in ns | Allocated bytes per operation |
| --- | --- | ---: |
| `diff_sparse_rust_10k` | 243500, 240770, 239916 | 784300 |
| `patch_single_1mb_file` | 13129208, 12990521, 13125854 | 476889 |
| `replace_all_rust_10k` | 5094562, 5002500, 4998000 | 429344 |
| `run_fast_match_100_files` | 348053750, 356039932, 341214100 | 1141731 |
| `run_fast_no_match_100_files` | 3661417, 3488286, 3691601 | 1141729 |
| `search_matching_1mb_file` | 122771, 83500, 123313 | 472956 |
| `search_missing_1mb_file` | 79771, 59667, 79167 | 0 |

The source revision of the modified `imara-diff` extraction predates
the available import history.

Diagnose at `47771a5` reviewed CLI routing, discovery, source reads,
plan revalidation, editor and terminal handling, and the supported
rename boundaries. The saved-plan duplicate finding remains DSLP-6.
Two new P2 findings cover terminal plan routing and read errors that
return success. A debug-only short-terminal failure is DSLP-9. The
macOS arm64 CLI and fixture suite passed all 42 tests, and Clippy passed.
The review sampled language adapters through their fixtures and did
not rerun the performance workload.

## Tickets



### DSLP-3 · P2 · Types: strict profile and typing findings

- **Area:** deslop · types
- **Where:** type checker and lint configuration; Rust sources
- **Problem:** The strict-profile candidates in the deslop skill's tools reference have not been measured here.
- **Recommendation:** Run the deslop Types step. Accept: clean at the adopted profile, escape hatches not increased, witnesses recorded.
- **Fix with:** deslop
- **Effort:** M

### DSLP-4 · P2 · Tests: coverage, boundary gaps, mutation

- **Area:** deslop · tests
- **Where:** test suites and CI
- **Problem:** Coverage and mutation results are unmeasured; no deslop mutation lane exists.
- **Recommendation:** Run the deslop Tests step. Accept: no undispositioned survivor in critical code, the test cost budget holds, and the lane is green where the tools are mirrored.
- **Fix with:** deslop
- **Effort:** M

### DSLP-5 · P2 · Dead code: tools and debloat

- **Area:** deslop · dead code
- **Where:** whole repository
- **Problem:** No dead-code scan has run.
- **Recommendation:** Run the deslop Dead code step. Accept: tools clean or allowlisted with reasons; walk-backs filed as Needs owner tickets.
- **Fix with:** deslop
- **Effort:** M


### DSLP-7 · P2 · Plan creation can enter interactive apply on a terminal

- **Area:** fastermod · saved plans
- **Where:** src/engine.rs:417, src/engine.rs:441, src/engine.rs:533, docs/refactor/cli.md
- **Problem:** At 47771a5, run --plan against a scratch text file from a 24-row PTY with stdin and stdout attached. Fastermod prompts to accept the edit. Accepting exits 0, changes the source, and creates no plan. --ident reproduces the same result. The non-TTY control writes the plan and preserves the source.
- **Recommendation:** Route --plan through the existing non-interactive plan tail for every content mode. Add a real PTY fixture that checks the exact plan, original source bytes and absence of a review prompt.
- **Fix with:** design-cli
- **Effort:** S


### DSLP-8 · P2 · An unreadable explicit source reports a clean check

- **Area:** fastermod · discovery
- **Where:** src/source.rs:32, src/discovery.rs:117, src/engine.rs:87
- **Problem:** At 47771a5, create a scratch text file containing foo and set its mode to 000 as an unprivileged user. --check foo bar FILE warns Permission denied, prints no pending path and exits 0. With a matching file containing invalid UTF-8, --apply also warns, leaves the edit unapplied and exits 0.
- **Recommendation:** Propagate read and decode failures into the existing operational error path while preserving binary exclusions. Exercise the command with an explicitly named file and check exit 4 and its diagnostic.
- **Fix with:** error-model
- **Effort:** M


### DSLP-6 · P3 · Saved plans apply conflicting entries for one file

- **Area:** fastermod · saved plans
- **Where:** src/plan.rs:307, src/plan.rs:525, src/plan.rs:1077
- **Problem:** At fda1e8f, generate a one-file plan with --plan. Duplicate its only file entry and give the duplicate different replacement bytes and after text. --apply-plan exits 0, reports two files applied, and leaves only the second entry's bytes. The checked-in generator produced one entry. The demonstrated failure requires a modified saved plan.
- **Recommendation:** Reject duplicate target entries before any write. Keep the accepted plan format and add a CLI fixture that checks the failure status and the input file's original bytes.
- **Fix with:** durable-state
- **Effort:** S


### DSLP-9 · P3 · Short terminals underflow the preview height budget

- **Area:** fastermod · terminal review
- **Where:** src/review.rs:377, src/terminal.rs:62
- **Problem:** At 47771a5 in a debug build, a PTY reporting 12 rows and 80 columns aborts a one-line diff preview with attempt to subtract with overflow before the review prompt.
- **Recommendation:** Saturate the terminal-height budget with a narrow-terminal test that still prints the changed lines.
- **Fix with:** os-boundaries
- **Effort:** S

