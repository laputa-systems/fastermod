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

## Saved-plan read-set revalidation

The CLI generates a fresh identifier plan inside its own discovery root
and then applies it in a separate process. An unedited file in a nested
directory has the same basename as the plan. It enters the read set
because it contains the searched identifier in a comment. Application
excludes the plan by its resolved identity.

The unchanged case applies exactly the planned edit. An edited file,
an unedited file whose contents changed, or an added or removed file
each produce the corresponding stale diagnostic and leave the target
bytes intact. Run `cargo test --test cli
saved_plan_revalidates_the_read_set_before_writing`.

The eight cargo-mutants 27.1.0 identities at `src/plan.rs:489–513`
were already caught by the original suite at `739619d` and remained
caught with the CLI cases. The source blob is
`05677778d1395b85d706b26817021beabe3830c4`. Reproduce with
`cargo mutants --no-config --gitignore true --file src/plan.rs -F
'src/plan.rs:(48[0-9]|49[0-9]|50[0-9]|51[0-7]):' --timeout 30`.

## Hosted source mutation trial

The [positive hosted run](https://github.com/laputa-systems/fastermod/actions/runs/44277884233)
at `d34b68c` caught seven of nine source identities on Linux ARM64
and macOS. Both runners classified two builds as unviable and had no
timeouts. Gate times were 108.9 and 239.8 seconds. The two jobs used
425 runner seconds.

| Control | Result on both platforms |
| --- | --- |
| [Documentation edit](https://github.com/laputa-systems/fastermod/pull/38) | No mutants selected. |
| [Warning test deletion](https://github.com/laputa-systems/fastermod/pull/39) | The empty `DisplayWarning::fmt` implementation survived. |
| [Mixed change](https://github.com/laputa-systems/fastermod/pull/40) | The warning mutation survived and the three viable changed discovery identities were caught. One discovery build was unviable. |
| [Uncovered branch](https://github.com/laputa-systems/fastermod/pull/41) | Two of six changed-region identities survived. |

Ordinary CI and lint passed all controls. The mixed control took 199.6
seconds on Linux and 518.7 on macOS inside the gate. Run
`make mutants-diff BASE=origin/main` from a clean checkout with Python
3.14, cargo-mutants 27.1.0 and ignored `.work/` scratch. This trial
qualifies the source and diagnostic scope. The wider saved-plan
inventory, terminal review and suite cost remain in DSLP-4.

The workflow watches the source module, tests, fixtures, the gate,
settings and documentation. Test and fixture changes protect the entire
source module and include production edits in the same change. Other
production changes use the ordinary CI check until their mutation
scopes are qualified.
