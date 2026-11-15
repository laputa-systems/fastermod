Prefix: REFAC
Next: 3

# Stateless polyglot refactoring campaign ledger

Status: in progress. Contract and launch prompt:
[docs/CAMPAIGN-STATELESS-REFACTOR.md](docs/CAMPAIGN-STATELESS-REFACTOR.md).

This ledger tracks execution, not shipped functionality. Take the lowest-numbered
unblocked ticket; use dependencies to parallelize independent language adapters
only after shared contracts are stable. Retain evidence when closing tickets.
Keep existing TODO.md and TODO-DESLOP.md findings; cross-reference rather than
copying them. All P1 capabilities/accuracy work precedes performance tuning.

## Report

- REFAC-1 done (see evidence below). Base `365cd94`; HEAD at commit time is
  the REFAC-1 commit on `main`.
- Verification: `make build`, `make test` (24 unit tests x2 targets + fixture
  suite), `cargo check --workspace`, `make lint` all pass; `make bench` ran
  and refreshed `benches/baseline.json` (large positive deltas on
  `patch_single_1mb_file`/`run_fast_match_100_files` recorded in
  `docs/refactor/baseline.md` for REFAC-14).
- Implemented through REFAC-6: dual-license resolution (LICENSE-APACHE,
  `MIT OR Apache-2.0`), fixture runner, frozen CLI grammar, library split
  + typed plans, safe multi-file apply, ast-grep/tree-sitter reuse
  decision, `--ident`/`--rewrite` syntax engines + capability registry
  for wave A. `--rename`, `--rules`, `--apply-plan`, `--plan` remain
  exit-3 stubs (REFAC-7/8/12).

## Tickets

### REFAC-1 · P1 · Ground behavior, verification and licensing

- **Area:** baseline
- **Depends on:** none
- **Where:** AGENTS.md, src/, benches/, Makefile, CI, existing TODO files
- **Work:** Read live HEAD; establish text/interactive/smartcase/ignore and file-write witnesses, actual test/build commands, dependency/license inventory, and measured baseline where tools exist. Coordinate with deslop and FAST-7. Set up a fixture runner suitable for later engines without prescribing a large framework.
- **Accept:** Reproducible baseline and meaningful positive/negative CLI tests; current failures/environment gaps distinguished; short evidence report under docs/refactor/. Existing behavior is not assumed from README alone.
- **Effort:** M
- **Done:** Evidence in `docs/refactor/baseline.md`. Fixture runner at
  `tests/fixtures.rs` + `tests/fixtures/` (cases: text-basic,
  text-fixed-strings, text-smartcase, text-ignore, interactive-decline).
  FAST-7 resolved via dual-license files and manifest. Bench deltas recorded
  as pre-existing/environmental.

### REFAC-2 · P1 · Choose a small CLI and explicit mode contracts

- **Area:** CLI design
- **Depends on:** REFAC-1
- **Where:** CLI parser/help, tests, docs/refactor/cli.md
- **Work:** Prototype the campaign's mode flags and two alternatives using realistic transcripts. Prefer unchanged OLD NEW [PATH...] operands and explicit --ident/--rewrite/--rename modes. Resolve legacy regex/subcommand collisions, anchors, TTY/non-TTY defaults, apply/preview/plan, invalid flag combinations, exit codes and JSON schema. Decide autonomously, then stop redesign churn.
- **Accept:** Golden help/transcript/parser tests, ordinary tasks with no config, no silent semantics change, no implicit non-TTY writes, exact ambiguity diagnostics. Document proposed versus implemented behavior honestly. Whole-operation review is required for semantic rename.
- **Effort:** M
- **Done:** Grammar frozen in `docs/refactor/cli.md` (mode flags chosen over
  subcommands/`--mode`; rejected alternatives recorded). Implemented: all
  mode flags, validation, exit codes 0-4, non-TTY preview, `--dry-run`,
  `--check`, `--json` (schema v1), `--apply`, `--languages`, `--at`/`--lang`
  parsing. Engine modes return exit 3 until REFAC-6+. Fixtures:
  preview-non-tty, check-pending, check-clean, json-text, apply-json,
  mode-conflict, at-requires-rename, ident-unimplemented, languages-json.

### REFAC-3 · P1 · Extract the shared engine and typed edit plans

- **Area:** core architecture
- **Depends on:** REFAC-1, REFAC-2
- **Where:** src/lib.rs, src/main.rs, discovery/source/planning/output modules
- **Work:** Replace include!-of-main with a real library and thin CLI. Separate source snapshots, engine selection, typed operations/edits, diagnostics, review and application. Route legacy text behavior through tested reusable boundaries; avoid per-engine writers and premature crate proliferation.
- **Accept:** Text parity witnesses pass; deterministic preview/JSON plans record expected bytes and edit provenance; overlap and invalid-range validation exists; no AST/LSP startup cost on the text path beyond demonstrated necessities.
- **Effort:** L
- **Done:** `include!`-of-main replaced by a real library (`src/lib.rs`) with
  thin `src/main.rs`. Modules: `cli` (frozen grammar), `discovery`,
  `source`, `text`, `diff`, `review`, `plan`, `output`, `engine`,
  `benchmark`. `src/plan.rs` adds typed `EditPlan`/`SpanEdit`/`Anchor`/
  `Diagnostic`/`Coverage`, content-hash staleness, `validate()` (schema,
  unsafe paths, bounds, UTF-8 boundaries, overlap) and `apply()`
  (validate-all-then-write, per-file atomic stage via `Fastermod::save`,
  no cross-file transaction claim). Preview/`--check`/`--json`/`--apply
  --json` route through `build_text_plan`; plain `--accept-all` keeps the
  parallel fast path. `operation_id` is content-derived (stable across
  checkouts). JSON schema v1 now emits `operation_id`. Spawn tests moved
  to `tests/cli.rs`; unit tests colocated per module; plan validation
  unit tests added. No new dependencies; text path has no parser/LSP
  startup cost.

### REFAC-4 · P1 · Make multi-file application and review safe

- **Area:** filesystem safety
- **Depends on:** REFAC-3
- **Where:** apply/validation modules and fault-injection integration tests
- **Work:** Preflight whole operations; stage per-file writes; preserve bytes/newlines/metadata under an explicit policy; detect stale snapshots, links and root escapes. Implement safe failure reporting and recovery without Git resets or overwriting concurrent changes. Specify non-global atomicity and residual TOCTOU limits.
- **Accept:** Read-only preview and all-or-refuse validation tests pass; injected failures, cancellation, concurrent changes and permissions have accurate outcomes. A semantic rename cannot accept an arbitrary subset of required references. No claim of atomicity across multiple ordinary files.
- **Effort:** L
- **Done:** `Fastermod::save` now guards with `symlink_metadata` (refuses
  symlinks and non-regular files on every write path). `plan::apply`
  preflights root containment (canonicalized roots), file type and
  staleness before any write; writes attempt every file, set per-file
  `applied`, and report applied/failed paths on partial failure. JSON
  gains per-edit `applied` and an `apply-failed` diagnostic emitted even
  when apply fails. Policy documented in `docs/refactor/apply-safety.md`:
  per-file atomicity only, residual TOCTOU window, hardlink rename
  semantics, permission preservation, xattr/ACL limitation, cancellation
  leftovers, no git rollback. Tests: stale-refusal, root-escape,
  symlink-refusal, partial-failure reporting (all in `plan.rs`).

### REFAC-5 · P1 · Prove reusable syntax and resolution foundations

- **Area:** implementation spikes
- **Depends on:** REFAC-3
- **Where:** dependency manifests, adapter boundaries, docs/refactor/reuse.md
- **Work:** Compare ast-grep library reuse, direct Tree-sitter queries and relevant srgn patterns. Exercise maintained semantic candidates, including Oxc for JS/TS, and inspect scope/stack-graph approaches with current maintenance and grammar compatibility. Use executable representative rename cases; reject LSP dependencies and hidden runtimes. Do not adopt archived stack-graphs blindly.
- **Accept:** A bounded, evidence-backed choice with real code, licenses, dependency/portability findings and unresolved semantics; core interfaces support both syntax and binding engines. Do not leave multiple speculative backends or turn this into an endless research ticket.
- **Effort:** M
- **Done:** Executable spike at `spikes/reuse-spike/` (vendored deps,
  gitignored). Decision in `docs/refactor/reuse.md`: `ast-grep-core` +
  `ast-grep-language` 0.51.2 for structural matching/rewriting (proven:
  metavar pattern, comment decoy ignored, byte-range edits, 26
  feature-gated grammars, path->language detection); `oxc_*` 0.158.0 for
  JS/TS binding semantics (proven: scopes, symbol spans, resolved
  references); direct `tree-sitter` 0.27 + grammar crates for wave B/C
  languages outside ast-grep's set; explicit per-language resolver for
  non-JS/TS bindings. `srgn` reviewed and rejected as dependency
  (binary-first, narrow); `stack-graphs` rejected (archived). All MIT.
  Mirror gap resolved 2026-11-15: `stuff publish cargo --lockfile`
  published the spike's dependency set (~252 MB) to the `stuff`
  crates-io mirror; `cargo` resolves the crates normally (see
  `docs/refactor/reuse.md`).

### REFAC-6 · P1 · Ship identifier and structural modes with a real registry

- **Area:** syntax editing
- **Depends on:** REFAC-2, REFAC-3, REFAC-4, REFAC-5
- **Where:** language registry, identifier/structural engines, fixture runner
- **Work:** Implement exact identifier-role edits, AST patterns/captures/constraints and byte-preserving replacements for wave A. Generate machine/human capabilities from one registry with grammar pins/licenses and fixture IDs. Handle invalid syntax, language detection/overrides and ignored/generated files explicitly.
- **Accept:** Useful end-to-end Rust/Python/JS/TS edits with comment/string decoys, Unicode, escaping and parse-error fixtures. Syntactic operations are labeled as such. Ordinary use works offline with bundled grammars, no installed sg, no dynamically discovered native plugin.
- **Effort:** L
- **Done:** `src/lang.rs` typed capability registry (wave A: rust,
  python, javascript/jsx, typescript, tsx) drives both human and JSON
  `--languages` output with grammar bundle pin, license, extensions,
  identifier roles, rewrite/binding support, limitations and fixture IDs;
  `binding:"none"` is reported honestly until REFAC-7. `src/syntax.rs`
  implements `--ident` (identifier-kind nodes only — `identifier` or
  `*_identifier` — so comments/strings/decoys can never match) and
  `--rewrite` (ast-grep `Pattern`/`replace_all` with `$$$`/`$VAR`
  metavars, per-language pattern cache). Files with parse errors are
  skipped with an explicit reason; unknown extensions are skipped;
  `--lang` overrides detection for `--ident`/`--rewrite`/`--rules`.
  Both modes emit typed plans through `plan::assemble_plan`, share the
  REFAC-4 apply path (staleness, containment, per-file atomicity),
  support `--check`/`--dry-run`/`--json`, and get per-file interactive
  y/n/A/q review on TTY. Deps: `ast-grep-core`/`ast-grep-language` 0.51.2
  + `tree-sitter` 0.27 (all MIT, bundled grammars, no runtime). Verified:
  `cargo test --workspace` (22 unit + 14 CLI + fixture suite incl. new
  ident-{rust,python,javascript,typescript,tsx}, rewrite-rust,
  ident-parse-error, ident-unmatched, ident-lang-override,
  ident-unicode, languages-json fixtures), `cargo clippy
  --all-targets --all-features -- --deny warnings`, `cargo check
  --workspace` all pass. Manual smoke: `--apply` on mixed rs/py/js tree
  renamed identifiers only; parse-error file skipped; `--lang rust`
  override parsed a `.txt` file.

### REFAC-7 · P1 · Implement binding identity and capture-safe local rename

- **Area:** resolution
- **Depends on:** REFAC-4, REFAC-5, REFAC-6
- **Where:** resolution/validation adapters and wave-A semantic fixtures
- **Work:** Build invocation-local scopes/bindings/references and exact symbol selection. Implement language-specific scope rules, destination validity, collision/capture detection in both directions, shorthand expansion, and post-edit binding equivalence. Include real Rust/Python/JS/TS local success cases, not only refusals.
- **Accept:** Nested unrelated same-name bindings stay untouched; introduced names cannot capture old references; ambiguous/macro/dynamic cases fail explicitly when relevant. Before/after binding mapping is checked. No name-only fallback, confidence cutoff or semantic --force escape.
- **Effort:** L
- **Done:** `--rename OLD NEW` dispatches to `src/rename.rs`, which
  selects one binding (unique spelling or `--at path:line:col` on the
  declaration or any resolved reference), emits typed
  `RenameReference` span edits, then re-parses and re-resolves the
  edited file (`verify_rename`) proving the renamed binding keeps its
  reference count at intentionally edited positions and every other
  binding keeps declaration site and reference count — capture,
  unbinding and parse failure all refuse. Per-language analyzers:
  `src/bind_js.rs` (oxc semantic, exact unresolved sets, import/export/
  shorthand expansions), `src/bind_py.rs` (tree-sitter scope walk:
  module/function/lambda/class/comprehension scopes, `global`/
  `nonlocal` redirects, walrus, import forms; `import *` and `match`
  patterns mark analysis inexact), `src/bind_rs.rs` (tree-sitter arena
  scopes with item hoisting, let/pattern/closure/for bindings, `use`
  trees, module-path resolution `m::f`/`self::`/`crate::`, struct
  shorthand, method-call blocking; macro token identifiers recorded as
  unresolved so macro-covered spellings always block). Coupled syntax
  uses `Expansion` templates (`ShorthandColon`, `ImportAlias`,
  `ExportAlias`) so `{ x }`→`{ x: NEW }`, `import { x }`→`{ x as NEW }`,
  `export { x }`→`{ NEW as x }`, `use m::x`→`use m::NEW as x` preserve
  the second meaning. `RefSite.remote` marks other-namespace path
  segments that cannot refer to lexical bindings. Registry now reports
  `binding:"local"` for rust/python/javascript/typescript/tsx.
  Fixtures: rename-javascript (shorthand+export+import), rename-python
  (global/nonlocal), rename-rust (`use` cascade `use m::renamed as
  target`), rename-typescript, rename-ambiguous and rename-blocked
  (macro) exit-3 cases. `cargo test --workspace`, clippy `-D warnings`,
  `cargo check --workspace` pass.

### REFAC-8 · P1 · Resolve ordinary workspace renames for the first wave — DONE

- **Area:** cross-file semantics
- **Depends on:** REFAC-7
- **Where:** module/import graph, configuration readers, wave-A corpus
- **Work:** Extend to module declarations, explicit imports/exports, aliases and re-exports, nested workspace/package boundaries, relevant conditional configuration and public API scope reporting. Distinguish analysis roots from edit filters. Analyze dependency edges without requiring the old spelling in each file; detect incomplete relevant coverage.
- **Accept:** Real multi-file Rust/Python/JS/TS renames update the exact reference set and preserve unrelated imports/bindings; changed alias/config and excluded required files are tested. Successful parse or leftover-text grep is not the completeness oracle. No build-script execution or LSP backend.
- **Effort:** XL
- **Landed:** `src/workspace.rs` module graph (JS/TS relative+dynamic `import()`/`require`, Python dotted/relative imports and `__all__`, Rust `crate::`/`self::`/`super::` module tree); import/export/re-export edges with alias-preserving expansions; same-spelling symbol clustering via canonical import resolution; per-file re-parse + binding-equivalence verification plus a post-edit cross-file resolution proof. `--rename` discovery no longer prefilters on the old spelling. Fixtures: `rename-workspace-{javascript,python,rust}`.

### REFAC-9 · P1 · Complete wave-B application and systems adapters

- **Area:** language breadth B
- **Depends on:** REFAC-6, REFAC-7, REFAC-8
- **Where:** Go, C, C++, Java, C#, Kotlin, Swift, PHP, Ruby, Dart, Lua, Bash/POSIX-shell and PowerShell adapters
- **Work:** Split into language-sized child tickets while retaining this aggregate gate. For each, ship syntax edits, meaningful local binding renames, ordinary module/workspace resolution where applicable, and explicit difficult-construct boundaries. Reuse proven adapters/libraries, not generic identifier spelling masquerading as semantics.
- **Accept:** Every named language has fixture-backed capability rows, useful positive semantic renames, shadowing/capture decoys, ordinary cross-file examples where applicable and explicit unsupported cases. Parser-only rows or refusal-only tests do not close the ticket. Native portability remains intact.
- **Effort:** XL
- **Progress:** complete for all analyzable languages. Syntax (`--ident`/`--rewrite`) + capability rows for all 13; semantic local rename landed for Go (`845b516`), Lua (`290d5c0`/`74956fd`), Java (`90f1c65`/`85810be`), PHP (`1c0f366`/`ea2d658`), Ruby (`91cef0b`/`689ce83`), C# (`f244fbe`), Kotlin (`c8d2f89`/`f209f82`), Swift (`8b42608`), Dart (`29dcf52`), C (`406bc3c`), C++ (`f50e236`), Bash (`a943cee`) — each with shadowing/capture fixtures and explicit unresolved-construct blocking. PowerShell: no grammar in ast-grep-language 0.51.2 (verified against bundled `SupportLang` enum) — evidenced open, not in registry. Cross-file workspace resolution remains wave-A only (Rust/Python/JS); wave-B languages conservatively block on unresolved same-name occurrences across files.

### REFAC-10 · P1 · Complete the remaining major-language inventory

- **Area:** language breadth C
- **Depends on:** REFAC-6, REFAC-7, REFAC-8
- **Where:** Scala, R, Julia, Perl, Objective-C, Zig, Elixir, Erlang, Haskell, OCaml, F#, Clojure, Groovy, Fortran, MATLAB/Octave, COBOL and Visual Basic .NET adapters
- **Work:** Add language-sized child tickets and apply the same capability contract as REFAC-9. Model namespaces, pattern binding, dynamic scope and macros specifically. Missing grammar/rules remain evidenced open work; do not quietly remove a language from the inventory.
- **Accept:** The entire wave meets syntax/local-rename and applicable workspace gates with meaningful success and adversarial fixtures. Unsupported constructs are granular and tested. A partial subset can ship but does not close this aggregate ticket.
- **Effort:** XL
- **Progress:** all bundled wave-C grammars landed — Scala (`eaf8a9f`), Zig (`09f88e6`), Elixir (`8cb8f6e`), Haskell (`ad4aba2`, module prescan + let/where/do scopes) each with binding analyzers, rename fixtures, shadowing decoys and member-access blocking. Vacuous wave-B ident/rewrite fixtures repaired in `0b7ced5`. External grammar integration (`d152ff2`): new `Grammar` abstraction unifies ast-grep `SupportLang` with standalone `tree-sitter-*` crates published to the stuff registry; `--ident`/`--rewrite` verified for R, Julia, Objective-C, Erlang, OCaml(+interface), F#(+signature), Groovy, Fortran, MATLAB/Octave, VB.NET with real fixtures. All analyzable wave-C languages now have binding analyzers: Erlang (`bind_erlang.rs`, arity-keyed `name/arity` functions, `-export`/call refs, prescan for forward refs, literal-atom blockers), Julia (`bind_julia.rs`, overload-shared bindings, RHS-before-LHS), R (`bind_r.rs`, single-namespace rebinding merge), Groovy (`bind_groovy.rs`, `7cd51c2`), VB.NET (`bind_vb.rs`, `f18365d`, case-insensitive keys), OCaml (`bind_ocaml.rs`, `722b7dc`: let/fn-param/let-in scopes, recursive-vs-sequential binding order, module-member access unresolved), F# (`bind_fsharp.rs`, `edf224a`: sequential `let` scopes, recursive/non-recursive defns), Fortran (`bind_fortran.rs`, `1e282ef`: procedure scopes, dummy args, case-insensitivity, end-procedure name refs, same-scope redeclaration merge), MATLAB/Octave (`bind_matlab.rs`: script workspace scope vs function-local scope, outputs+args+body vars, `obj.field` unresolved). `--lang` extended to `--rename` so `.m`/`.fsi`-style extension ambiguity is resolvable per-file. OCaml-interface and F#-signature rows remain syntax-only (grammar shapes differ from implementation files). Objective-C stays syntax-only: message-send receiver types cannot be resolved, so binding rename would be guessing. Perl, Clojure and COBOL have no tree-sitter-0.27-compatible crates.io grammar (links conflict on `tree-sitter` or binary-only crate) — evidenced open pending vendored grammars.

### REFAC-11 · P1 · Cover structured assets and embedded languages

- **Area:** mixed repositories
- **Depends on:** REFAC-6, REFAC-7
- **Where:** SQL dialects, HTML/CSS/SCSS, JSON/YAML/TOML, Vue/Svelte host mappings
- **Work:** Implement suitable structural/rule edits for each named format and exact mappings between embedded/host source ranges. Treat SQLite/PostgreSQL/MySQL separately; bind schema references only within a declared supported schema domain. No live database access and no pretending data keys are lexical variables.
- **Accept:** Host-language bytes and delimiters survive embedded edits; source anchors map accurately through Unicode/newlines; dialect and script ambiguity produce explicit diagnostics. Tests distinguish syntax operations, genuine supported bindings and cross-language boundary assumptions.
- **Effort:** L
- **Progress:** structured rows landed earlier (css/html/json/yaml/hcl/nix/markdown/solidity). This slice adds: TOML (`tree-sitter-toml-updated`, `bare_key` ident kinds), SCSS (dedicated `tree-sitter-scss` grammar; css row no longer claims `.scss`), PostgreSQL (`tree-sitter-postgres` LANGUAGE; `.sql` stays dialect-ambiguous and requires `--lang`), and `src/embed.rs` embedded-region mapping: `<script>`/`<style>` blocks in HTML/Vue/Svelte (lang/type attrs select js/ts/css/scss) and Markdown fenced code blocks (info-string tag -> language) are parsed with the inner grammar and edits are offset back into host bytes, so delimiters and host text are preserved; `--ident`/`--rewrite`/`--rules` ops all reach embedded regions. Fixtures: ident-toml, ident-scss, ident-postgresql, embedded-ident, embedded-rewrite. Rules residual scan now excludes the rules file itself (it is config, and now parses as TOML). Evidenced open: Vue/Svelte host grammars — `tree-sitter-vue-updated` vendors an html scanner that exports colliding `tree_sitter_html_external_scanner_*` symbols and corrupts the bundled html grammar at static-link time (verified: well-formed HTML parses with ERROR nodes only when the crate is linked); Svelte has no crates.io grammar; SQLite (`willbooster-tree-sitter-sql`) and MySQL have no tree-sitter-0.27-ABI-compatible crates (`tree-sitter-sql-polyglot` pins `cc ~1.2.1` which breaks the C build cache/toolchain and its sqlite/mysql grammars were not verified); `--rename` inside embedded regions is unsupported (binding analysis is per-file; region plumbing open); SQL schema binding intentionally unmodeled (no declared schema domain).

### REFAC-12 · P1 · Add powerful batches, portable plans and residual review

- **Area:** migrations and automation
- **Depends on:** REFAC-4, REFAC-8, REFAC-11
- **Where:** rule schema, plan serialization/application, batch validation, reports
- **Work:** Add declarative language-scoped migration rules, simultaneous rename maps and explicitly ordered in-memory rewrites. Save/apply plans with relevant read-set plus discovery/config fingerprints. Add residual diagnostics and explicit cross-language bridge/migration intent without inferring shared identity from spelling. Keep all configuration data-only.
- **Accept:** Swap/cycle/conflict fixtures pass; stale imports, added files, changed configs and modified sources invalidate plans. Partial-reference acceptance cannot sneak through batch review. Plan schemas reject incompatible versions and path escapes. Repeated calls need no registration or cache.
- **Effort:** L
- **Progress:** complete for the declared scope. Versioned JSON plan serialization (`plan::save`/`load`, schema-gated, data-only) with `--plan FILE` save-without-write and `--apply-plan FILE` load+revalidate+apply; plans now carry `operation_id`, `config` fingerprint, full `read_set` (every analyzed file + content hash), and `discovery` fingerprint (walk params + seen-path hash) so modified sources, changed unedited dependencies/manifests, added/removed files, and `.gitignore` drift all invalidate at apply. The plan file itself is excluded from re-walk when it lives inside a scanned root. Declarative `--rules FILE`: data-only TOML `[[op]]` tables (`ident`/`rewrite`/`rename`, optional `lang` scope, ordered, executed sequentially over in-memory contents), swap/cycle-safe, rename ops reuse the fail-closed binding engine (unresolved refs abort the whole rules run — no partial-reference batch acceptance), residual `old`-spelling diagnostics emitted as warnings scoped to analyzable files. Cross-language identity is never inferred: each op names its language explicitly. Fixtures: `rules-basic`, `rules-swap`, `rules-residual`, `plan-save`, `apply-plan`; unit tests cover save/load round-trip, changed-read-set and added-file invalidation.

### REFAC-13 · P1 · Qualify correctness, portability and stateless execution

- **Area:** release-quality evidence
- **Depends on:** REFAC-9, REFAC-10, REFAC-11, REFAC-12
- **Where:** integration/property/mutation/corpus tests, CI, release verification
- **Work:** Grow these tests throughout prior slices; here run the full matrix, development-only compiler corroboration, semantic binding witnesses, malformed inputs and filesystem fault cases. Audit license artifacts and native bundled scanner linking. Exercise the built binary with empty HOME/XDG, offline networking, no LSP/runtimes, no index and read-only preview workspaces; inspect persistent writes and child processes.
- **Accept:** No unexplained correctness failures or false success; every promoted capability is backed by meaningful tests; macOS/Linux-musl advertised targets are verified or explicitly still open. No hidden state, daemon, network or runtime language-tool dependency. Missing tooling is not reported as passing.
- **Effort:** L

### REFAC-14 · P2 · Optimize demonstrated cold-process workloads

- **Area:** performance
- **Depends on:** REFAC-13
- **Where:** realistic corpus benchmarks, engine profiles and allocation hot paths
- **Work:** Preserve baseline measurements, then benchmark whole-process text/identifier/AST/semantic/batch operations over pinned real repos. Separate phases, output work, OS-cache state, latency distribution and peak RSS. Compare only equivalent edit/identity sets. Optimize profiled bottlenecks with bounded parallelism and sound candidate pruning.
- **Accept:** Correctness and capability coverage remain unchanged; no daemon/index/cache shortcut. Report measured deltas and tradeoffs rather than claiming regex-equivalent semantic costs. Old text behavior gets its own regression analysis. Features are not removed or made less accurate for a benchmark win.
- **Effort:** L

### REFAC-15 · P1 · Review the actual product and close truthfully

- **Area:** integration and handoff
- **Depends on:** REFAC-13, REFAC-14
- **Where:** README/help/examples, capability matrix, docs/refactor/, this ledger
- **Work:** Review actual CLI transcripts and implementation, independently where tools permit. Re-run relevant checks after integration; remove abandoned scaffolding and duplicate paths. Document modes, exact safety boundaries, incomplete-analysis behavior, source selection, saved plans and measured performance. Verify examples against the built binary.
- **Accept:** Full required inventory and operation gates pass before declaring completion. Otherwise provide a usable partial result with exact open tickets; never call parser coverage full semantic support. Report commits, evidence, limitations and next actions. Do not tag, publish or install a release without separate permission.
- **Effort:** M
