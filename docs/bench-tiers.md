# Benchmark tiers

`make bench-list` lists the eleven rustybench workloads and areas. `make
bench-check` runs preflight. The Edit tier needs only Python and the selection
self-test; the Change tier also checks tool versions (cargo/rustc floors, gate
tools by help signature), host readiness, free space, sources newer than the
built benchmark, and leftover benchmark or guard processes.

`make bench-tier` runs the selection, version, budget and validity self-test.
`make bench-tier BENCH_ARGS='--tier change --name search_missing_1mb_file'`
runs that workload (Change). Select names with repeated `--name`, areas with
repeated `--area`, and exclusions with repeated `--skip`; names and areas are
distinct, combined selections intersect, and unknown or empty selections fail.
`--sample-count`, `--sample-size` and `--repeat` must be positive and are held
equal across paired legs. `--dry-run` prints the command without creating a run
directory. Results, per-workload timing validity, exclusions and logs go under
`.work/bench/` in `report.json`. A quiet host gate is required for timing to be
valid; otherwise the workload is excluded with that reason. A Change run proves
completion only, not a performance verdict. The first release-profile build can
take minutes; warm cost is workload dependent.

The `cli` area holds complete CLI journeys: text apply, binding-aware rename,
saved-plan apply and interactive review. Each journey asserts the emitted
output and the resulting file bytes inside the measured operation, so a
journey that stops doing the work fails the benchmark.

`BENCH_ATTRIBUTIONS=1 cargo bench --bench bench` runs every workload once
without timing and prints one JSON record per journey. Records carry
process and child CPU time, process-wide allocation counts and bytes (including
worker threads, unlike rustybench's per-thread tallies), and Linux process-byte
I/O plus child block-operation counts. On macOS byte I/O is reported as
unavailable. Attribution records are not timing samples and carry no verdict.

`make bench-tier BENCH_ARGS='compare --tier change --name cli_saved_plan --baseline-root <root> --candidate-root <root>'`
runs a paired scenario across two prepared checkouts with identical budgets.
Legs alternate order across repeats and each leg is checked with
`perf-gate-ready` before and after; a failed leg or busy gate excludes that
workload from timing. The report records each leg and its validity, and never
turns completion into a speed verdict.

`make bench RUSTYBENCH=rustybench` remains the explicit manual full baseline;
do not invoke it as a prerequisite or automate it here. Build both revisions
before measurement. All fixtures are generated in the benchmark process; set
`TMPDIR="$PWD/.work/tmp"` before running.
