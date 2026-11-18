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

## JSON apply discovery

The JSON apply route exercises the snapshot discovery path. An isolated
Git fixture supplies a parent and local `.ignore`, a `.gitignore`, a
repository exclude file and a global ignore file. The test checks every
file on disk with and without `--no-ignore`, and keeps a hidden file
separate from the ignore rules.

At `1468b653`, cargo-mutants 27.1.0 selected seven identities in
`walk_builder_with_file_set` and `matching_files`. The baseline missed
all six viable checks. The new test catches all six negation changes.
The remaining build is unviable. Both lists have SHA-256
`ff5f269aad3cc8611cf5dd130f87057813275d3830361ad14853d77996941ef6`
and use discovery source blob `0744a564c952ef3210a78bca44d26bf4967f1e68`.
Use `cargo mutants --no-config --file src/discovery.rs --re
'in (walk_builder_with_file_set|matching_files)$' --timeout 25 -- --locked`
in a disposable checkout.
