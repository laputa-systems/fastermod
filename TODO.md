Prefix: FAST
Next: 13
### FAST-9 · P3 · Saved plans apply conflicting entries for one file

- **Area:** fastermod · saved plans
- **Where:** src/plan.rs:307, src/plan.rs:525, src/plan.rs:1077
- **Problem:** At fda1e8f, generate a one-file plan with --plan. Duplicate its only file entry and give the duplicate different replacement bytes and after text. --apply-plan exits 0, reports two files applied, and leaves only the second entry's bytes. The checked-in generator produced one entry. The demonstrated failure requires a modified saved plan.
- **Recommendation:** Reject duplicate target entries before any write. Keep the accepted plan format and add a CLI fixture that checks the failure status and the input file's original bytes.
- **Fix with:** durable-state
- **Effort:** S

### FAST-10 · P3 · Short terminals underflow the preview height budget

- **Area:** fastermod · terminal review
- **Where:** src/review.rs:377, src/terminal.rs:62
- **Problem:** At 47771a5 in a debug build, a PTY reporting 12 rows and 80 columns aborts a one-line diff preview with attempt to subtract with overflow before the review prompt.
- **Recommendation:** Saturate the terminal-height budget with a narrow-terminal test that still prints the changed lines.
- **Fix with:** os-boundaries
- **Effort:** S

### FAST-11 · P3 · Benchmark end-to-end codemod journeys and attribution

- **Area:** fastermod · benchmarks
- **Where:** benches/bench.rs, tools/bench_tier.py
- **Problem:** The seven generated Rust workloads measure internal search, replacement, diff, patch and tree walks, not complete CLI startup, binding-aware rename, interactive review or saved-plan journeys. Tree-walk allocation accounting excludes worker threads; no CPU or I/O attribution is captured. Their black-box calls do not assert complete output bytes.
- **Recommendation:** Add representative CLI journeys with output and completed-work oracles, and separate process-tree CPU, allocation and I/O attribution from timed runs. Qualify comparisons under the host gate.
- **Fix with:** bench-pyramid
- **Effort:** M

### FAST-12 · P3 · Strengthen benchmark preflight and comparison reporting

- **Area:** fastermod · benchmarks
- **Where:** tools/bench_tier.py
- **Problem:** The tier driver checks local tools and free space but does not validate tool versions, host readiness, stale inputs, leftover processes or paired comparison budgets. The Edit tier demands cargo, rustc and cpu-guard although its self-test needs only Python, and the run summary has no per-workload timing validity. A Change run is completion evidence only.
- **Recommendation:** Add selected-workload preflight and paired scenario reporting with explicit timing validity and exclusions, retaining manual-only Full.
- **Fix with:** bench-pyramid
- **Effort:** M
