Prefix: DSLP
Next: 12

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
| Saved-plan schema, edit ranges, containment, symlinks, stale read set and writes | `src/plan.rs` validation/apply tests and `tests/cli.rs::saved_plan_revalidates_the_read_set_before_writing`. Conflicting duplicate entries remain DSLP-6 (accepted P3 risk). Wider admission and write decisions are guarded by the unit tests listed in the disposition below. |
| Human preview and terminal review | `src/review.rs` diff and replacement tests, `tests/cli.rs::interactive_review_applies_each_reply_to_its_own_match` (added in this pass: n, default, unknown, q and end-of-input replies on a pseudo-terminal, file bytes and prompt counts) and `plan_writes_only_the_plan_with_a_terminal_attached`. The similarly named `test_interactive_yes_to_all_finishes_fast` uses `--accept-all`, so it does not guard review. The exact rendered diff and editor (`e`, `E`) replies stay untested by a byte oracle (advisory); short terminals remain DSLP-9 (accepted P3 risk). |
| Public Rust `run()` and `benchmark` helpers | `src/main.rs` calls `run()` and the CLI and fixture tests exercise it; `benches/bench.rs` consumes the helpers. Their direct API is not independently tested. |
| CLI modes, status, JSON and language matrix | `tests/cli.rs`, fixture cases and `src/plan.rs` tests; see `docs/refactor/cli.md`. Not every flag combination has an independent boundary test. |

The Edit tier is a selected `cargo test` case; Change is `make test` and
`make lint` (all workspace targets), also the acceptance commands and local
consumers. Scenario is the hosted Linux/macOS mutation gate, not ordinary
iteration.

Remaining roadmap items, dispositioned (2026-11-20, macOS arm64):

- Review output: one demonstrated gap, that no test sent a reply on a terminal
  (piped input never enters review), so the choice between writing and
  declining a match was unguarded. The new pseudo-terminal case closes it.
  Proof: making the `n` reply save the match fails it (`left: "bar 1\nbar 2\nbar
  3\n"`, `right: "foo 1\nbar 2\nfoo 3\n"`); the file was restored byte for byte
  (`cmp`) and the case passes. The rendered diff text and editor replies are
  not asserted byte for byte: accepted, advisory.
- Wider plan decisions: `validate` and `apply` already have unit cases for
  ordering, bounds, UTF-8 boundaries, parent paths, schema, containment,
  symlinks, staleness, partial failure and read-set drift. One hand-injected
  fault (`byte_start < prev_end` changed to `<=` in `validate`) failed 8 plan
  tests and was restored byte for byte, so no test was added. A mutation run
  of `apply_excluding` was not possible: `cargo-mutants` is not installed here
  (Needs environment); the retained hosted lanes are unchanged.
- Paired suite cost: six interleaved pairs of the three compiled debug test
  binaries (unit, `cli`, `fixtures`), baseline running `cli` with the new case
  skipped, candidate running it, measured in user plus system CPU on a busy
  host (load 13 to 35). Medians: 2.20s baseline, +0.09s, ratio 1.04 (pairs
  0.99 to 1.21), well inside 20% or 30s. The Ground warm figure of 5.99s was
  taken on a different revision and host load and is not compared.

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

Dead code: on macOS arm64, `cargo rustc --lib -- --force-warn dead_code
--force-warn unreachable_pub` exited 0 and reported six unread field groups and
eight visibility warnings. Removed: the Rust `ModDecl` vector (no readers; its
record and constructor), the unused fixture `OsStr` placeholder, and the unread
`kind` field with its `ScopeKind` enum in the C, Groovy, VB, Fortran and Zig
analyzers. Their only consumers are the fixture cases `ident-c`, `ident-groovy`,
`ident-vb-dotnet`, `ident-fortran`, `ident-zig`; they
pass. With the last unread item gone, the crate-wide `#![allow(dead_code)]` in
`src/lib.rs` (its comment named exactly those fields) was removed, so the
compiler guards the crate again; the same scan now reports only
`unreachable_pub`, and `make lint` (clippy over all targets and features)
stays clean. Allowlist (one entry): `unreachable_pub` on eight items, `Color`
and the terminal functions, `plan.rs:18`, `bind.rs:125,139`; they are live
items in private modules and the lint is a visibility preference outside the
project's lint profile, so narrowing them removes no code. Linux and other
platform builds were not locally checked. `cargo-machete` and `cargo-udeps`
are unavailable (Needs environment); no dependency was removed. Debloat traced
the shared plan tail: syntax apply reaches it, despite a contrary comment, so
it remains. No walk-back was identified: no flag, saved-plan field or output
changed. The removed code is outside the seven declared text-workload paths
(`src/diff.rs`, source reading, replacement and tree walks), so no paired
timing was owed and none is claimed.

Bench audit (2026-11-20): the seven `benches/bench.rs` workloads cover
search hit/miss, replacement, sparse diff, patch and 100-file tree walks with
generated inputs. They use wall time and allocator counters; tree counters
exclude worker allocations. They do not independently assert complete outputs,
cover CLI startup, saved-plan or semantic rename, or provide process-tree CPU
and I/O attribution. The historical baseline has three measured passes and is
not an interleaved comparison; the five-pair text-source comparison is recorded
separately in `docs/deslop-text-source-perf.md`. DSLP-10 ranks those coverage and
attribution gaps; DSLP-11 records preflight, comparison and reporting gaps.
`docs/bench-tiers.md` describes the entrypoint: `make bench-list`,
`bench-check`, `bench-tier` (Edit by default; Change with `--tier change`
and `--name`, `--area`, `--skip`). Full remains the explicit `make bench`
command and was never run.

Entrypoint proof: Edit (`make bench-tier`) exited 0 in 0.20s with the selection
self-test. Selection checks: an unknown name and an empty selection each exit 2
with a named reason and no run directory. With `cargo` missing from `PATH` the
Change preflight exits 2 naming the tool and nothing ran; `--dry-run` prints the
command and creates no run directory. One Change workload,
`search_missing_1mb_file`, ran end to end through `cpu-guard`: exit 0, the
release-profile build took 7m44s on a busy host (the first build took 20m51s),
and the output has one sample of 224.7 us with 1 allocation of 16 B. It is
completion evidence only; no timing verdict follows from one sample. The other
six workloads and Scenario were not run.

Review of the cumulative diff from `d99400a` (docs, ten source and test files,
the bench entrypoint) found no demonstrated defect to fix. Checked: the removed
`ModDecl` and `kind` items have no reader (the compiler, now without the crate
`dead_code` allow, proves it); no CLI flag, saved-plan field or exit code
changed; the review test writes only inside its own temporary directories
and bounds the child at 10s. Residual risks: the pseudo-terminal case ran on
macOS arm64 only (the Linux arm64 PR check is its first run there, where the
master read ends with an I/O error rather than end of file, which the reader
loop treats as the end); no change touched a declared performance workload, so
no paired timing was owed. The Edit tier's over-strict tool check is in DSLP-11.

Summary of this pass. Closed: DSLP-4 (Tests) and DSLP-5 (Dead code); the Bench
step produced the entrypoint and its proof but no ticket of its own. Filed:
DSLP-10 and DSLP-11 (P3). Open: DSLP-6, DSLP-9, DSLP-10, DSLP-11, all P3. No P1
or P2 finding and no `Needs owner ·` item is open. No gap in the shared fleet
files was found. Needs environment: `cargo-mutants`, `cargo-llvm-cov`,
`cargo-machete` and `cargo-udeps` are not installed locally, and Linux
builds and tests were not run locally.

## Tickets

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

### DSLP-10 · P3 · Benchmark end-to-end codemod journeys and attribution

- **Area:** fastermod · benchmarks
- **Where:** benches/bench.rs, tools/bench_tier.py
- **Problem:** The seven generated Rust workloads measure internal search, replacement, diff, patch and tree walks, not complete CLI startup, binding-aware rename, interactive review or saved-plan journeys. Tree-walk allocation accounting excludes worker threads; no CPU or I/O attribution is captured. Their black-box calls do not assert complete output bytes.
- **Recommendation:** Add representative CLI journeys with output and completed-work oracles, and separate process-tree CPU, allocation and I/O attribution from timed runs. Qualify comparisons under the host gate.
- **Fix with:** bench-pyramid
- **Effort:** M

### DSLP-11 · P3 · Strengthen benchmark preflight and comparison reporting

- **Area:** fastermod · benchmarks
- **Where:** tools/bench_tier.py
- **Problem:** The tier driver checks local tools and free space but does not validate tool versions, host readiness, stale inputs, leftover processes or paired comparison budgets. The Edit tier demands cargo, rustc and cpu-guard although its self-test needs only Python, and the run summary has no per-workload timing validity. A Change run is completion evidence only.
- **Recommendation:** Add selected-workload preflight and paired scenario reporting with explicit timing validity and exclusions, retaining manual-only Full.
- **Fix with:** bench-pyramid
- **Effort:** M

