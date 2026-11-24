# Benchmark tiers

`make bench-list` lists the seven rustybench workloads and areas. `make
bench-check BENCH_ARGS='--tier change --name search_missing_1mb_file'` checks
local tools and disk. `make bench-tier` runs the selection self-test (Edit).
`make bench-tier BENCH_ARGS='--tier change --name search_missing_1mb_file'`
runs one sample and iteration of that workload (Change). Select names with
repeated `--name`, areas with repeated `--area`, and exclusions with repeated
`--skip`; names and areas are distinct, combined selections intersect, and
unknown or empty selections fail. `--dry-run` prints the command without
creating a run directory. Results and logs go under `.work/bench/`. The Change
run proves completion only, not a performance verdict. The first release-profile
build can take minutes; warm cost is workload dependent.

`make bench RUSTYBENCH=rustybench` remains the explicit manual full baseline;
do not invoke it as a prerequisite. For comparative timing, build both
revisions before measurement, alternate their run order, and check
`perf-gate-ready` before and `perf-gate-ready -1` after each sample. Discard
samples that overlap host load. The tier driver reports host readiness as a
reminder; its one sample is not a quiet-host gate. All fixtures are generated
in the benchmark process; set
`TMPDIR="$PWD/.work/tmp"` before running.
