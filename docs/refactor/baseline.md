# REFAC-1 baseline evidence

Base: `365cd94688f550ca8d47793d096aca4c75cb5f13` (includes the campaign commit
`365cd94`; inspected implementation base `187747e`).
Host: macOS aarch64 (Darwin 27.2.0), rustc/cargo 1.102.0-nightly
(`nightly-2026-11-09` per `rust-toolchain.toml`).

## Verification commands and results

| Command | Result |
| --- | --- |
| `make build` | pass (`dev` profile, 2.3s incremental) |
| `make test` (`cargo test --quiet`) | pass: 24 unit tests (run under both lib and bin targets because `src/lib.rs` `include!`s `src/main.rs`) + fixture suite |
| `cargo check --workspace` | pass |
| `make lint` (`cargo clippy --all-targets --all-features -- --deny warnings`) | pass, zero warnings |
| `make bench` (`rustybench` sibling at `../rustybench`) | ran to completion; rewrote `benches/baseline.json` with this host's numbers |

Bench deltas vs the previous `baseline.json` (same host class,
`macos-aarch64-10-32gb`): `diff_sparse_rust_10k` -10%, `patch_single_1mb_file`
+122%, `replace_all_rust_10k` -9%, `run_fast_match_100_files` +4081%,
`run_fast_no_match_100_files` +6%, `search_matching_1mb_file` +95%,
`search_missing_1mb_file` +102%. These measurements predate the campaign code
changes (license headers, fixture runner, and tests only), so the deltas are
environmental or reflect an older baseline rather than a regression introduced
here. `run_fast_match_100_files` at ~557ms vs ~13ms baseline is the largest
discrepancy and is recorded for REFAC-14 investigation, not explained away.

## Behavior witnesses (pre-change)

Existing unit tests in `src/main.rs` cover: regex replace-all, `-F`
fixed-strings, `--print-changed-files`, interactive accept-all fast path,
interactive decline, zero-length matches/replacements, multibyte-adjacent
patches, hidden files, `--no-ignore`, empty-pattern refusal, invalid regex
exit, and read-only-file write failure. `README.md` documents `-S/--smartcase`
but it had no test (TODO.md FAST-8).

New in this slice: `tests/fixtures.rs`, a directory-driven CLI fixture runner
(`tests/fixtures/<case>/{args,files,expected,stdin,expect-*}`). Each case runs
the built binary in a temp copy of `files/` and byte-compares the resulting
tree. Seed cases: `text-basic`, `text-fixed-strings`, `text-smartcase`
(covers FAST-8), `text-ignore`, `interactive-decline`.

Known pre-change gaps recorded for later tickets:

- Non-TTY invocation without `--accept-all` prompts on stdout and dies on EOF
  (exit 1); the campaign requires preview-instead-of-hang/write. Deferred to
  the CLI slice (REFAC-2/3).
- No `--ident`, `--rename`, `--rewrite`, JSON output, or plan/apply modes.

## Dependency and license inventory

Runtime: `grep 0.4`, `ignore 0.4.34`, `imara-histogram-diff` (path),
`lexopt 0.3.2`, `regex-lite 0.1.9`, `rustix 1.1.6` (unix, termios only).
Dev: `rustybench 0.5` (bench harness). Transitive closure is ~49 crates,
all permissive (MIT/Apache-2.0/BSD/Unicode-DFS-2016 family); see
`cargo tree`.

FAST-7 resolution (per `docs/rfc/FAST-7-license-notices.md` recommended
path): added `LICENSE-APACHE` with the verbatim Apache-2.0 text; `LICENSE`
now states the MIT grant covers only files not already under Apache-2.0;
`Cargo.toml` license is `MIT OR Apache-2.0`; Apache headers retained in
`src/main.rs`/`src/terminal.rs`; `imara-histogram-diff` sources gained
Apache-2.0 headers naming the `imara-diff` origin. Copyright holder named as
"The imara-diff authors" for the extracted algorithm.

## Fixture runner contract

`tests/fixtures/<case>/args` holds one CLI argument per line with `{ROOT}`
expanding to a fresh temp copy of `files/`; `expected/` is byte-compared
after the run; `stdin`, `expect-status`, `expect-stdout`, `expect-stderr` are
optional. Entries starting with `.`/`_` are skipped. Later language slices
add cases here without new harness code.
