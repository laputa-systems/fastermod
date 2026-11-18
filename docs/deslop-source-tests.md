# Fastermod text source mutation evidence

At `a4040a1`, cargo-mutants 27.1.0 selected nine identities in
`src/source.rs`. The original unit and CLI suites caught six and
left one miss and two unviable builds. The missing behavior was the
separate warning for an unwritable matched file. The CLI still returned
its final error when the warning formatter emitted nothing.

The permission regression now verifies the warning identifies the
unwritable file, the process fails, the original bytes are preserved,
and writable siblings are still edited. The same nine-identity run
caught seven with zero misses or timeouts. The other two replace the
warning wrapper or grep searcher with a default that those types do
not implement. The unchanged source blob is
`198d9ce8c6cb658ddb01b6b79c143636be7d8c77`. All 46 ordinary tests and
Clippy passed on macOS arm64.

From a disposable checkout with the pinned toolchain, run:

```sh
TMPDIR="$PWD/.work/tmp" cargo mutants --no-config --file src/source.rs \
  --in-place --timeout 20 --build-timeout 240 --output .work/tmp/source-witness \
  --no-times -- --lib --test cli
```

This selection covers match admission, warning formatting, binary and
text decoding behavior, and first error retention. The whole-change
gate, saved-plan and review boundaries, and paired suite cost remain
in DSLP-4. The product workload acceptance for the source read repair
is recorded separately in [performance evidence](deslop-text-source-perf.md).
