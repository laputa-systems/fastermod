Prefix: FAST
Next: 14

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

### FAST-13 · P3 · C ident edits skip macro-braced test files

- **Area:** fastermod · C language coverage
- **Where:** src/syntax.rs
- **Problem:** fastermod 0.0.1 on macOS arm64 skips Asterinas regression C files such as test/initramfs/src/regression/process/sched/sched_attr_getset.c for --ident because FN_TEST/END_TEST macros supply the function braces and the unpreprocessed file parses with syntax errors; the requested rename of static sched_setattr wrappers therefore makes no edit, while fixed-string mode is a workable but less precise fallback.
- **Recommendation:** Support an evidenced-safe syntactic token path for C files whose parse errors are confined to macro-generated structure, or document and test the fixed-string fallback for macro-braced C; acceptance is a fixture shaped like the Asterinas FN_TEST file where --ident either performs only exact identifier-token edits with zero nonidentifier replacements or fails with a documented actionable diagnostic.
- **Fix with:** language-coverage
- **Effort:** M

