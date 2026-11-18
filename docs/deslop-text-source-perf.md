# Text source read-error performance check

[Fastermod #32](https://github.com/laputa-systems/fastermod/pull/32)
makes source read failures visible to the command. The timed comparison
used base `181697963fe51229f3f1d76c0beb5f3ce8aae3ff` and candidate
`964cd37e4a13155a7df17a06e50d66e9325bc481`. No workload showed a median
slowdown larger than both 2% and the observed spread.

The macOS arm64 run used the pinned Rust toolchain, the installed
Rustybench 0.4.0 driver, and the locked 0.5.0 harness. Both bench profiles
were built before timing. Five pairs alternated which revision ran
first, with one timed harness run per side per pair. The configured
`perf-gate-ready` load check exited zero immediately before and after
every sample. It also reported background CPU activity on some samples.
The ranges below include that variation. All seven workload names and
success results matched across the ten runs.

| Workload | Base median, range (ms) | Candidate median, range (ms) |
| --- | ---: | ---: |
| Sparse diff, 10k Rust lines | 0.270 (0.241–0.370) | 0.287 (0.248–0.340) |
| Patch one 1 MB file | 13.983 (13.102–16.002) | 14.972 (13.986–17.422) |
| Replace all, 10k Rust lines | 5.684 (5.035–6.890) | 5.904 (5.771–6.311) |
| 100-file walk with matches | 376.437 (349.283–487.981) | 360.844 (355.785–369.638) |
| 100-file walk without matches | 4.487 (4.250–5.983) | 4.367 (4.339–4.487) |
| Search one 1 MB file with a match | 0.144 (0.130–0.265) | 0.119 (0.112–0.134) |
| Search one 1 MB file without a match | 0.088 (0.084–0.145) | 0.081 (0.075–0.122) |

The benchmark repeatedly reads the generated source and passes results
through the harness. Both direct searches allocated the same number of
objects and bytes. The worker walk allocation counters cover the
calling thread. These are measurements of the checked fixture and host.

The release binary was 109,366,856 bytes on the base and 109,348,952 on
the candidate. Five independent executions per side previewed a
1,045,030-byte Rust file. Both binaries returned success, printed
byte-identical diffs containing the replacement, and left the input
unchanged. Median peak RSS was 10,125,312 versus 10,223,616 bytes,
with ranges of 10,059,776–10,158,080 and 10,158,080–11,321,344.

## Reproduce

Use separate clean checkouts of the recorded revisions. Build the
optimized harness with `cargo bench --locked --bench bench --no-run`.
Then repeat this command under the readiness guard and alternate the
checkout order between pairs.

```sh
rustybench baseline --root "$CHECKOUT" --baseline "$OUTPUT" --warmup 0 --runs 1 \
  -- cargo bench --locked --bench bench
```

Use the same release toolchain on both revisions for the memory check.
The fixture is 95,000 lines of `// fixture`, followed by
`const VALUE: &str = "needle";` and a newline. Run
`/usr/bin/time -l fastermod --dry-run --fixed-strings needle replacement FILE`
with stdin redirected and compare the complete preview and source hash.
The existing CLI witness separately exercises the source read failure.
