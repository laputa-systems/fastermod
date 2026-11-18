Prefix: DSLP
Next: 6

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

Diagnose is next. The source revision of the modified `imara-diff`
extraction predates the available import history.

## Tickets


### DSLP-2 · P2 · Diagnose: ultrareview into this backlog

- **Area:** deslop · diagnose
- **Where:** whole repository
- **Problem:** No review has ranked this repository's risks for the deslop pass.
- **Recommendation:** Run the deslop Diagnose step. Accept: the diff touches only docs, comments, and this file, and every finding names its Fix with.
- **Fix with:** deslop
- **Effort:** S

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

