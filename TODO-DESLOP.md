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

Types at `88d1e1f` used all-target, all-feature Clippy with the pinned
nightly `5b7b37aef` on macOS arm64. Candidate warnings use `-W unsafe_code`
and `-W clippy::<rule>` in place of `--deny warnings`. Counts deduplicate
JSON diagnostics by rule, file and line.

| Candidate | Locations | Decision |
| --- | ---: | --- |
| `unsafe_code` | 0 | No demonstrated rejection witness. |
| `wildcard_enum_match_arm`, `match_wildcard_for_single_variants` | 20, 2 | These include deliberate unrecognized-option and grammar fallbacks. |
| `cast_possible_truncation`, `cast_sign_loss` | 14, 2 | A saved-plan version could be narrowed before comparison. Reject at its original width. Remaining warnings include conversions outside the shipped target widths. |
| `unwrap_used`, `expect_used` | 233, 2 | Includes infallible writes to strings, grammar invariants and test assertions. |
| `redundant_clone`, `needless_pass_by_value` | 4, 2 | Cleanup candidates outside the type contract. |

The loader rejects an unsupported schema number in full and continues to
read plans written by the existing serializer. The regression fails on the
previous comparison, while the existing round-trip fixture still passes.
The ordinary lint profile remains clean and no escape hatch was added.

Tests inventory at `d99400a` (2026-11-20, macOS arm64; Linux arm64 is the hosted consumer):

| Public contract / critical decision | Guard and remaining gap |
| --- | --- |
| Text matching, replacement, glob and ignore discovery, error exits and offline help | `tests/cli.rs`: `test_simple_replace_all`, `test_glob_matches`, `json_apply_observes_the_full_ignore_configuration`, decode, binary, permission and help cases. |
| Identifier and binding-aware rename, embedded languages, JSON plans and applied bytes | `tests/fixtures.rs::fixture_cases` and `tests/fixtures/`; language gaps are listed in `docs/languages.md`. |
| Saved-plan schema, edit ranges, containment, symlinks, stale read set and writes | `src/plan.rs` validation/apply tests and `tests/cli.rs::saved_plan_revalidates_the_read_set_before_writing`. Conflicting duplicate entries remain DSLP-6 (accepted P3 risk). Wider plan admission and write decisions have not been mutated. |
| Human preview and terminal review | `src/review.rs` diff and replacement tests, `tests/cli.rs::test_interactive_yes_to_all_finishes_fast` and `plan_writes_only_the_plan_with_a_terminal_attached`. Exact interactive output and other responses are untested by a byte oracle; short terminals remain DSLP-9 (accepted P3 risk). |
| CLI modes, status, JSON and language matrix | `tests/cli.rs`, fixture cases and `src/plan.rs` tests; see `docs/refactor/cli.md`. Not every flag combination has an independent boundary test. |

The Edit tier is a selected `cargo test` case; Change is `make test` and
`make lint` (all workspace targets), also the acceptance commands and local
consumers. Scenario is the hosted Linux/macOS mutation gate, not ordinary
iteration. Ground observed 5.99s warm at `fda1e8f`; current paired suite cost
is unmeasured: historical timings on different revisions and host load are not
a paired comparison. No new test is justified solely by the unrun regions.

Mutation dispositions: source at `a4040a1` caught 7, 2 unviable, 0 misses or
timeouts; discovery at `1468b653` caught 6, 1 unviable; read-set decisions at
`739619d` caught 8. Discovery and read-set records do not separately state
timeout counts; none is inferred. Unviable default replacements do not compile, rather than
being equivalent runtime behavior. Hosted negative controls deliberately left
warning and uncovered-branch survivors; these demonstrate the gate rather than
unresolved product mutants. No equivalent survivors are claimed. Wider saved-plan
and terminal review mutations were not run (advisory); coverage and branch
percentages are unmeasured because `cargo-llvm-cov` is unavailable locally.
The retained source mutation CI selection and its hosted controls are documented
in `docs/deslop-source-tests.md`; no scope expansion. Needs environment: rerun
coverage and supported Linux ARM64 checks there; macOS ARM64 is the local shape.
Other platforms have not been qualified. Existing suite flake rate is unmeasured.

## Tickets

### DSLP-5 · P2 · Dead code: tools and debloat

- **Area:** deslop · dead code
- **Where:** whole repository
- **Problem:** No dead-code scan has run.
- **Recommendation:** Run the deslop Dead code step. Accept: tools clean or allowlisted with reasons; walk-backs filed as Needs owner tickets.
- **Fix with:** deslop
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

