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
- Implemented: dual-license resolution (LICENSE-APACHE, `MIT OR
  Apache-2.0`), directory-driven fixture runner `tests/fixtures.rs` with 5
  seed cases including smartcase coverage (FAST-8).
- Limitations: non-TTY preview behavior, `--ident`/`--rename`/`--rewrite`,
  JSON output and plans do not exist yet; those are REFAC-2+ scope.

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

### REFAC-5 · P1 · Prove reusable syntax and resolution foundations

- **Area:** implementation spikes
- **Depends on:** REFAC-3
- **Where:** dependency manifests, adapter boundaries, docs/refactor/reuse.md
- **Work:** Compare ast-grep library reuse, direct Tree-sitter queries and relevant srgn patterns. Exercise maintained semantic candidates, including Oxc for JS/TS, and inspect scope/stack-graph approaches with current maintenance and grammar compatibility. Use executable representative rename cases; reject LSP dependencies and hidden runtimes. Do not adopt archived stack-graphs blindly.
- **Accept:** A bounded, evidence-backed choice with real code, licenses, dependency/portability findings and unresolved semantics; core interfaces support both syntax and binding engines. Do not leave multiple speculative backends or turn this into an endless research ticket.
- **Effort:** M

### REFAC-6 · P1 · Ship identifier and structural modes with a real registry

- **Area:** syntax editing
- **Depends on:** REFAC-2, REFAC-3, REFAC-4, REFAC-5
- **Where:** language registry, identifier/structural engines, fixture runner
- **Work:** Implement exact identifier-role edits, AST patterns/captures/constraints and byte-preserving replacements for wave A. Generate machine/human capabilities from one registry with grammar pins/licenses and fixture IDs. Handle invalid syntax, language detection/overrides and ignored/generated files explicitly.
- **Accept:** Useful end-to-end Rust/Python/JS/TS edits with comment/string decoys, Unicode, escaping and parse-error fixtures. Syntactic operations are labeled as such. Ordinary use works offline with bundled grammars, no installed sg, no dynamically discovered native plugin.
- **Effort:** L

### REFAC-7 · P1 · Implement binding identity and capture-safe local rename

- **Area:** resolution
- **Depends on:** REFAC-4, REFAC-5, REFAC-6
- **Where:** resolution/validation adapters and wave-A semantic fixtures
- **Work:** Build invocation-local scopes/bindings/references and exact symbol selection. Implement language-specific scope rules, destination validity, collision/capture detection in both directions, shorthand expansion, and post-edit binding equivalence. Include real Rust/Python/JS/TS local success cases, not only refusals.
- **Accept:** Nested unrelated same-name bindings stay untouched; introduced names cannot capture old references; ambiguous/macro/dynamic cases fail explicitly when relevant. Before/after binding mapping is checked. No name-only fallback, confidence cutoff or semantic --force escape.
- **Effort:** L

### REFAC-8 · P1 · Resolve ordinary workspace renames for the first wave

- **Area:** cross-file semantics
- **Depends on:** REFAC-7
- **Where:** module/import graph, configuration readers, wave-A corpus
- **Work:** Extend to module declarations, explicit imports/exports, aliases and re-exports, nested workspace/package boundaries, relevant conditional configuration and public API scope reporting. Distinguish analysis roots from edit filters. Analyze dependency edges without requiring the old spelling in each file; detect incomplete relevant coverage.
- **Accept:** Real multi-file Rust/Python/JS/TS renames update the exact reference set and preserve unrelated imports/bindings; changed alias/config and excluded required files are tested. Successful parse or leftover-text grep is not the completeness oracle. No build-script execution or LSP backend.
- **Effort:** XL

### REFAC-9 · P1 · Complete wave-B application and systems adapters

- **Area:** language breadth B
- **Depends on:** REFAC-6, REFAC-7, REFAC-8
- **Where:** Go, C, C++, Java, C#, Kotlin, Swift, PHP, Ruby, Dart, Lua, Bash/POSIX-shell and PowerShell adapters
- **Work:** Split into language-sized child tickets while retaining this aggregate gate. For each, ship syntax edits, meaningful local binding renames, ordinary module/workspace resolution where applicable, and explicit difficult-construct boundaries. Reuse proven adapters/libraries, not generic identifier spelling masquerading as semantics.
- **Accept:** Every named language has fixture-backed capability rows, useful positive semantic renames, shadowing/capture decoys, ordinary cross-file examples where applicable and explicit unsupported cases. Parser-only rows or refusal-only tests do not close the ticket. Native portability remains intact.
- **Effort:** XL

### REFAC-10 · P1 · Complete the remaining major-language inventory

- **Area:** language breadth C
- **Depends on:** REFAC-6, REFAC-7, REFAC-8
- **Where:** Scala, R, Julia, Perl, Objective-C, Zig, Elixir, Erlang, Haskell, OCaml, F#, Clojure, Groovy, Fortran, MATLAB/Octave, COBOL and Visual Basic .NET adapters
- **Work:** Add language-sized child tickets and apply the same capability contract as REFAC-9. Model namespaces, pattern binding, dynamic scope and macros specifically. Missing grammar/rules remain evidenced open work; do not quietly remove a language from the inventory.
- **Accept:** The entire wave meets syntax/local-rename and applicable workspace gates with meaningful success and adversarial fixtures. Unsupported constructs are granular and tested. A partial subset can ship but does not close this aggregate ticket.
- **Effort:** XL

### REFAC-11 · P1 · Cover structured assets and embedded languages

- **Area:** mixed repositories
- **Depends on:** REFAC-6, REFAC-7
- **Where:** SQL dialects, HTML/CSS/SCSS, JSON/YAML/TOML, Vue/Svelte host mappings
- **Work:** Implement suitable structural/rule edits for each named format and exact mappings between embedded/host source ranges. Treat SQLite/PostgreSQL/MySQL separately; bind schema references only within a declared supported schema domain. No live database access and no pretending data keys are lexical variables.
- **Accept:** Host-language bytes and delimiters survive embedded edits; source anchors map accurately through Unicode/newlines; dialect and script ambiguity produce explicit diagnostics. Tests distinguish syntax operations, genuine supported bindings and cross-language boundary assumptions.
- **Effort:** L

### REFAC-12 · P1 · Add powerful batches, portable plans and residual review

- **Area:** migrations and automation
- **Depends on:** REFAC-4, REFAC-8, REFAC-11
- **Where:** rule schema, plan serialization/application, batch validation, reports
- **Work:** Add declarative language-scoped migration rules, simultaneous rename maps and explicitly ordered in-memory rewrites. Save/apply plans with relevant read-set plus discovery/config fingerprints. Add residual diagnostics and explicit cross-language bridge/migration intent without inferring shared identity from spelling. Keep all configuration data-only.
- **Accept:** Swap/cycle/conflict fixtures pass; stale imports, added files, changed configs and modified sources invalidate plans. Partial-reference acceptance cannot sneak through batch review. Plan schemas reject incompatible versions and path escapes. Repeated calls need no registration or cache.
- **Effort:** L

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
