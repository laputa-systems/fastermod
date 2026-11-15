# Stateless polyglot refactoring campaign

Status: ready to execute; this commit is the campaign, not its implementation.
Created: 2026-11-15. Repository: `laputa-systems/fastermod`.
Execution ledger: [TODO-REFACTOR.md](../TODO-REFACTOR.md).

## Launch

> Execute this campaign unattended in fastermod. Read AGENTS.md, this document,
> TODO-REFACTOR.md, and the current implementation. Take the earliest unblocked
> ticket, implement and verify a complete vertical slice, review it, commit it,
> update the ledger, and continue. Deliver a powerful stateless polyglot
> refactoring CLI: no daemon, no persistent index, no LSP, and no language-server
> subprocesses. Prioritize useful features and correct edits over speed, then
> optimize the demonstrated implementation. Preserve simple usage and support
> the full language inventory below, not only Rust/Python/TypeScript. Do not
> stop at a design, parser registry, refusal-only resolver, or three-language
> MVP. Never substitute spelling matches for resolved symbol identity. Make
> technical and CLI decisions autonomously, recording evidence and real gaps.

Creating this document does not start execution or schedule an automation.
When launched, work is confined to this repository and disposable fixture
workspaces. Commit verified slices to the current authorized development branch;
use `main` when the launch authorizes default-branch work. Respect branch
protections; never force-push or bypass checks. No registry publication, release
tag, installation over the owner's binary, edits to other repositories, or
production changes without separate authorization.

## Product contract

The desired tool combines the convenience of `fastermod OLD NEW [PATH...]` with
syntax-aware editing and genuine binding-aware renaming across major languages.
These constraints supersede earlier suggestions to use leta or another LSP
backend.

1. **Stateless means stateless.** Every invocation discovers and analyzes its
   inputs anew and exits. No daemon, watcher, socket service, workspace
   registration, persistent semantic cache, hidden database, or background
   child. No runtime network access, grammar downloads, telemetry, cloud/AI
   service, language server, or required Node/Python/JVM installation. Bundle
   pinned grammars/rules; build-time code generation is permitted. In-memory
   graphs, bounded workers, user-supplied configuration and explicitly requested
   plan/report files are compatible with this contract. A saved plan is an
   inspectable output artifact, not a hidden index.
2. **Accuracy is not optional.** Prefer correct, useful supported operations to
   either speculative edits or blanket refusals. Build real resolution and
   collision analysis. Unsupported constructs produce a specific explanation,
   not a guessed edit or successful no-op. There is no automatic fallback from
   symbol rename to identifier/text replacement.
3. **Breadth and power precede optimization.** Rust, Python and JS/TS are the
   first vertical slices, not the final scope. Syntax coverage alone does not
   complete a language. The full inventory remains a delivery obligation.
4. **One deployable native CLI.** Keep macOS and Linux-musl support; do not add
   Windows scope. Tree-sitter's generated native parsers are acceptable: Rust
   orchestration does not require rewriting every parser in Rust. External
   compilers/interpreters may be used by development tests, not as hidden runtime
   rename backends. Reusable in-process semantic libraries are allowed after
   checking portability, licensing and correctness.
5. **Keep ordinary use ordinary.** No mandatory project configuration or setup.
   A small command line should cover common tasks; advanced rules are optional.
   Deterministic machine output must make the same capabilities usable by agents.

Do not claim universal semantic completeness. Report the exact workspace,
configuration, operation and supported language constructs covered by a rename.
External consumers, reflection and FFI are real boundaries, not reasons to
abandon useful static resolution or silently edit less than requested.

## Grounding in the current repository

The inspected base is `187747e268ebfada5e3abb170048b022eb7969d1`. Re-read HEAD
before implementation; paths and details here are orientation, not immutable
truth. Existing code has a parallel `ignore` walker, grep-based prefilter,
regex replacement, smartcase, interactive review, and same-directory temporary
file saves. Most behavior is in `src/main.rs`; `src/lib.rs` includes it for the
benchmarks. Preserve behavior with witnesses before separating components.

Read `Makefile`, `docs/development.md`, `.github/workflows/`, `Cargo.toml`,
`benches/bench.rs`, `benches/baseline.json`, `TODO.md` and `TODO-DESLOP.md`.
Current verification entry points include `make test`, `cargo check --workspace`
and `make lint`. The current Makefile's lint recipe does not use `--fix`, despite
older AGENTS wording. Inspect commands rather than copying stale descriptions.
`make bench` currently expects the sibling rustybench tool unless overridden.

Coordinate with the existing deslop backlog; do not duplicate or erase it.
Resolve the existing FAST-7 license-notice inconsistency before distributing
new bundled dependencies. Carry upstream notices and grammar licenses forward.
Do not rewrite the toolchain, release system or diff library just to begin this
campaign.

## CLI: simple operands, explicit meaning

Start with **mutually exclusive mode flags**, preserving the established operand
shape and avoiding reserved subcommands that collide with old regex operands
such as `rename`, `text` or `apply`. These examples are proposed interfaces to
implement, not features already present:

```sh
# Existing default: regex text substitution; literal matching remains explicit.
fastermod 'OldName' 'NewName' src/
fastermod -F 'old.literal' 'new.literal' src/

# All matching identifier tokens, excluding comments/string contents by default.
# This deliberately does NOT mean all references to one binding.
fastermod --ident old_name new_name src/

# One binding and its resolvable references, with an optional exact anchor.
fastermod --rename old_name new_name .
fastermod --rename old_name new_name . --at src/lib.rs:42:9

# Structural pattern rewrite, not a semantic function rename.
fastermod --rewrite 'old($$$ARGS)' 'new($$$ARGS)' --lang rust src/

# Optional multi-operation, multi-language migration rules.
fastermod --rules migration.toml . --dry-run

# Discover what this exact binary really supports.
fastermod --languages --json

# Review now, explicitly apply later; no persistent analysis service.
fastermod --rename old_name new_name . --at src/lib.rs:42:9 --plan changes.json
fastermod --apply-plan changes.json
```

Before freezing the interface, exercise this design, explicit subcommands, and
one other minimal alternative against real usage transcripts. Choose within
REFAC-2, without asking the owner to arbitrate taste. The flag design is the
default unless another demonstrably improves clarity without silently changing
legacy commands. Record rejected alternatives and freeze one coherent grammar;
do not ship competing interfaces or a large alias collection.

Required behavioral distinctions:

| Mode | Selection | Claim |
| --- | --- | --- |
| Default text / `-F` | Regex / literal bytes or existing text semantics | Text replacement, including comments and strings |
| `--ident` | Language-defined identifier roles and exact names | Syntactic occurrences, potentially unrelated bindings |
| `--rewrite` | AST patterns, captures, constraints and replacements | Structural transformation, not inferred symbol identity |
| `--rename` | A uniquely selected binding plus resolved references | Resolution-backed rename within the reported analysis domain |

For rename, a bare name must resolve uniquely within the supplied roots; otherwise
return candidate declarations with exact anchors. `--at` selects identity, not
just a spelling. Verify that OLD matches the selected symbol. Define anchors as
1-based line and UTF-8 byte column, reject mid-codepoint positions, and include
byte offsets plus source excerpts in machine output so agents need not count
columns. Quote paths and parse numeric suffixes carefully. Qualified-name
selection can be added if it materially simplifies common tasks.

Keep current interactive text behavior and `--accept-all` compatibility. Provide
`--apply` as the clear unattended-write spelling, `--dry-run`, `--check`, stable
JSON output, deterministic diffs, and ignore/glob/language controls. Non-TTY
commands without explicit apply must preview rather than hang or write. JSON
must never imply permission to mutate. `--plan` previews and saves a versioned
plan; it is incompatible with simultaneous application. `--apply-plan` is an
explicit write command but must reject stale/unsupported plans. Validate
contradictory flags before opening files for writing.

Review a semantic rename as **one complete logical operation**, not independent
y/n prompts for each reference. Rejecting one required edit rejects the rename.
Batch review may accept independently verified operations only if dependencies
and post-edit resolution are revalidated. Avoid a partial-refactor footgun.

Freeze exit codes and a versioned machine schema in REFAC-2. Distinguish success,
`--check` finding changes, usage errors, ambiguous/unsupported analysis, and
I/O/application failure. Include `mode`, `applied`, coverage, edits, unresolved
references, skipped files and diagnostics; never let truncation or a resource
limit produce apparent complete success. Reject semantic use of regex/smartcase
options rather than quietly changing their meaning.

## Language coverage is a tested product surface

Maintain one typed capability registry generating `--languages --json` and a
human-readable matrix. Each language/dialect row records grammar identity and
revision, license, extensions/shebangs, embedded-language support, identifier
roles, structural rewrites, local binding resolution, workspace/module
resolution, supported symbol kinds, limitations and fixture IDs. Mark a feature
implemented only when its acceptance fixtures pass. Parser availability is not
rename support; a refusal is not a successful rename test.

The minimum inventory is intentionally broad. These are sequencing waves, not
permission to stop after the first one:

| Wave | Required languages / dialects |
| --- | --- |
| A: deep reference implementations | Rust; Python; JavaScript/JSX; TypeScript/TSX |
| B: widely used systems and application languages | Go; C; C++; Java; C#; Kotlin; Swift; PHP; Ruby; Dart; Lua; Bash and POSIX shell; PowerShell |
| C: complete the major-language inventory | Scala; R; Julia; Perl; Objective-C; Zig; Elixir; Erlang; Haskell; OCaml; F#; Clojure; Groovy; Fortran; MATLAB/Octave; COBOL; Visual Basic .NET |
| D: mixed repositories and structured assets | SQL with explicit SQLite/PostgreSQL/MySQL dialects; HTML; CSS/SCSS; JSON; YAML; TOML; Vue and Svelte embedded scripts/styles |

For A-C, deliver tested syntax-aware identifier editing and structural rewriting,
plus genuine local binding rename for a meaningful documented subset. Ordinary
module-level declarations, explicit imports/exports and workspace references
must be implemented where applicable and tested; local-only support is not
workspace support. Shell dynamic scope, macro systems and type-directed member
lookup need explicit models, not a universal identifier-node query.

For D, promise syntax/rule operations appropriate to the format, not fictional
lexical bindings. SQL rename needs a dialect and resolvable schema scope; do not
connect to a live database or infer SQL semantics from string spelling. Embedded
edits need exact host-offset mapping. Treat languages sharing extensions (C/C++
headers, Objective-C, TSX/JSX, shell variants) explicitly; do not guess silently
when the decision changes edits. Language overrides must be available.

A missing grammar or hard semantic feature remains a named open capability
with evidence and a follow-up ticket; do not delete the row or relabel syntax
matching as semantic. A partial release can be useful, but cannot be reported as
full campaign completion. Breadth must not be deferred indefinitely while
polishing one language's rare features.

## Reuse strategy and architecture

Use the smallest proven reusable pieces, not the entire architecture of another
tool. Perform bounded implementation spikes, not an open-ended literature tour:

- Evaluate `ast-grep-core` and relevant language/config crates for structural
  matching and rewriting; compare `srgn` for syntax-constrained replacement
  ideas. Do not depend on an installed `sg` executable or inherit its runtime
  plugin/config discovery accidentally. ast-grep explicitly does not supply
  scope/type analysis; it is not the semantic rename implementation.
- Evaluate maintained scope/name-resolution libraries and language-specific
  in-process frontends. Oxc's semantic/scoping APIs are a candidate for JS/TS
  bindings, not proof of whole-program types or complete cross-file renaming.
  Adapt source spans without reprinting entire files.
- Investigate stack/scope graphs and existing per-language rules before creating
  a generic resolver. GitHub's `stack-graphs` repository is archived and states
  it is unsupported; do not adopt it as a maintained turnkey solution. A reuse
  decision must demonstrate current grammar compatibility, tested resolution,
  no LSP runtime dependencies, acceptable maintenance, and Linux-musl/macOS
  builds. A simpler explicit resolver can win the spike. Do not start a large
  fork or a new general-purpose analysis framework without evidence.

Split the existing monolith behind a shared pipeline:

```text
CLI request -> file discovery / source snapshot
            -> text | identifier | structural | binding-resolution engine
            -> typed edit plan + coverage + diagnostics
            -> conflict / collision / post-edit validation
            -> diff or JSON review -> explicit application
```

Prefer ordinary modules initially: discovery/source, languages, syntax,
resolution, planning, validation, apply, output, CLI. Extract a real library
instead of including the binary's main source. One typed operation enum and one
edit/application path; no stringly-typed dispatch, duplicated per-language
writers, mandatory async runtime, or speculative plugin system. Permit a
specialized semantic backend behind a narrow interface where it is demonstrably
more correct than generic rules.

An invocation-local workspace graph should represent scopes, bindings,
references, imports/exports and relevant configuration. Separate spelling from
identity, namespace from name, and syntactic roles from bindings. Discover
modules and import edges even in files without the old spelling: aliases,
re-exports and implicit references make a literal prefilter unsound for semantic
completeness. Optimize that only with a proved conservative candidate set.

Read configuration as data. Never evaluate Python modules, JS config files,
build scripts, macros, shell code or arbitrary plugins to discover a project.
Model relevant paths, package boundaries, Rust cfg/features, C preprocessor
configurations, TS path mappings, etc.; explicitly report unsupported or missing
configuration. Build graphs fresh on every call and bound resource use without
silently dropping edges.

## Rename correctness obligations

A resolved rename must establish all of the following within its stated model:

1. **Target identity and coverage:** a unique selected declaration/binding,
   recognized syntax, and a complete reference set for supported constructs in
   the analyzed workspace/configuration. Explicitly account for inaccessible,
   ignored, generated, parse-failed and potentially relevant unknown files.
   User selection filters must not silently cut required references out of a
   logical rename. Separate edit roots from analysis roots and report scope.
2. **No capture or accidental unbinding:** validate the destination identifier,
   keyword/escaping rules, collisions, namespaces and shadowing. Analyze both
   renamed references and pre-existing uses of the new name. Introducing a
   declaration must not capture an untouched reference to an outer binding.
3. **Language-specific relationships:** handle or explicitly reject destructuring,
   closures, hoisting, Python global/nonlocal/comprehension scope, overloads,
   traits/interfaces, inheritance, members, implicit receivers, macros,
   conditional compilation, import aliases and re-exports. Do not reuse one
   language's scoping rules merely because two grammars have similar nodes.
4. **Syntax coupled to multiple meanings:** expanding JS object shorthand or
   Rust field shorthand may be necessary to rename a local without changing a
   public field/key. Aliased imports may need to preserve the local name while
   updating the exported name. A one-token replacement is not always a rename.
5. **Post-edit invariants:** reparse and re-resolve the affected analysis domain
   in memory before writing. Renamed references must map to the intended
   renamed binding; untouched references must retain their binding under a
   before/after identity mapping. Validate final simultaneous names for swaps
   and cycles. Successful parsing alone is insufficient evidence.

Unknown resolution that may affect the target blocks automatic symbol writes.
Use a justified dependency slice so unrelated dynamic code does not make every
rename unusable, but record the argument for excluding it. No confidence-score
threshold, `--force`, or silent incomplete-reference mode may bypass semantic
checks. The user can deliberately choose separately labeled identifier/text
editing; that is a different operation, not a downgrade inside rename.

Report boundaries separately from incompleteness: no tool can enumerate unknown
external consumers of a public API. Explain that the declared workspace is the
analysis boundary. Within it, potential relevant dynamic/reflective references
remain unresolved, not magically safe. Residual scans of old spellings in code,
strings, comments and configuration are review aids, never a proof of reference
completeness or permission to auto-edit every occurrence.

Across languages, distinguish **many language backends** from **one symbol
crossing an FFI/API/schema boundary**. Support explicit declarative migration
rules or validated bridges for the latter; never infer shared identity from
matching names. Generated consumers should normally be regenerated from their
source of truth by the user, not silently rewritten.

## Safe plans, batches and filesystem behavior

The shared edit plan records source identity, expected bytes/ranges, replacement,
operation ID, engine/capability, configuration and coverage evidence. Plans are
versioned data, not executable scripts. Include the full relevant read-set and
file-discovery/configuration fingerprint, not only files being edited: a changed
import, added shadowing file or modified manifest can invalidate an old rename.
Applying a saved plan revalidates those prerequisites and rejects stale plans;
do not quietly replan different edits after approval.

Validate every operation before any write. Reject overlapping/conflicting edits,
out-of-bounds ranges, invalid UTF-8 boundaries, invalid replacement syntax,
path traversal, root escapes, non-regular files and unexpected symlink/hardlink
semantics. Preserve unrelated bytes, comments, line endings, BOM and executable
permissions; define metadata/ACL/xattr handling explicitly, rather than silently
losing attributes. Reject unsupported encodings with actionable diagnostics.
Do not run a formatter as part of ordinary rename.

Apply edits against one source snapshot, with per-file offset ordering and
cross-operation dependency checks. Support simultaneous rename maps (including
swaps) only after collision and final-resolution validation. For ordered
structural migrations, define sequencing explicitly and recompute later steps
on the in-memory result; do not rescan already-written files opportunistically.
The optional rule format is versioned, declarative and language-scoped. Reuse
upstream AST pattern/constraint semantics instead of inventing a general-purpose
programming language. Configuration must not run shell hooks or native plugins.

Use same-directory staged writes and atomic **per-file** replacement, with all
preconditions checked before mutation and rechecked as close to replacement as
practical. Do not claim an ordinary filesystem provides an atomic transaction
across many files. Detect concurrent modifications; avoid overwriting edits
made after the snapshot. Document the residual TOCTOU threat model honestly.
Handle injected write/rename/fsync failures and cancellation. Provide rollback
and recovery evidence where supported, never overwrite concurrently changed
files during rollback, and report exactly which files were applied, restored,
or still need recovery. Temporary/recovery files created for an active write
transaction do not constitute a persistent semantic index; no daemon may be
required to clean them up.

Do not require a clean Git worktree and never use `git reset`, `git clean`, or
checkout-based rollback. User modifications are input to the operation, not
something the tool owns. Unsafe state, unsupported semantics or partial apply
must be visible in both exit status and structured results.

## Execution sequence

Work the companion ledger as the source of ticket state. The slices are ordered
by dependencies, not an instruction to finish all syntax breadth before any
real symbol rename:

1. Establish exact behavioral/verification witnesses, a shared fixture runner,
   licensing inventory and a short evidence report (REFAC-1).
2. Choose and test the CLI grammar and machine contract (REFAC-2).
3. Extract the shared library, source snapshot and edit-plan/application core;
   add filesystem and whole-rename review tests (REFAC-3/4).
4. Prove the reuse choices, registry and first structural/identifier modes with
   actual Rust/Python/JS/TS operations (REFAC-5/6).
5. Deliver resolution, collision checks, and useful local plus workspace renames
   for A; do not defer this to after dozens of superficial adapters (REFAC-7/8).
6. Complete B and C through reusable adapter contracts and language-specific
   fixtures; ship syntax and resolution together in manageable slices
   (REFAC-9/10). Complete D and embedded-coordinate handling (REFAC-11).
7. Finish batch migration, saved plans, residual diagnostics and cross-language
   boundary handling on the shared core (REFAC-12).
8. Run the adversarial/differential corpus, portability and no-state audits;
   fix gaps, then optimize real cold-invocation workloads (REFAC-13/14).
9. Independently review when possible, verify documentation against the binary,
   and close with measured capabilities and exact unresolved work (REFAC-15).

REFAC-13's tests begin with the first implementation slice; its final gate is
not permission to defer correctness testing. Parallelize independent adapters
only after the shared contracts are proven. Keep one integration owner and
avoid concurrent writes to the same core files or ledger. Re-read live branch
state before committing and integrate others' work without discarding it.

## Verification and acceptance

Maintain golden input/output/diagnostic fixtures per language, positive and
negative cases, CLI integration tests, and resolution-before/after witnesses.
For every promoted semantic capability include real success cases plus
same-spelling unrelated symbols, nested shadowing, capture in both directions,
imports/aliases/re-exports, and unsupported-boundary refusals where applicable.
A suite that only returns Unsupported is not progress toward a rename engine.

Include escaped/Unicode identifiers, astral text before anchors, CRLF/BOM,
comments and strings, shebangs, mixed-language embeddings, parse recovery,
generated files, multiple roots, missing dependencies, duplicate names, ignored
references, cyclic imports, macro/preprocessor branches, permissions, links,
concurrent writers, stale plans, interrupts and failure injection. Use property
and mutation tests for span mapping, no-change byte preservation, plan
non-overlap, binding invariants and fail-closed behavior. Do not assert generic
AST rewrites are idempotent; test idempotence only for rules whose contract
requires it. Invertible renames may have round-trip tests under explicit
preconditions.

Use native compiler/interpreter/type-checker checks and trusted refactoring
fixtures as **development-only** corroboration where available, never as the
runtime backend. Passing compilation is not proof of unchanged binding or
behavior. Pin corpus revisions and record licensing; separate comparison
availability from the correctness claim. No LSP is needed even in the default
test lane. Missing platform tooling is an unverified gate, not a passing test.
Keep fast deterministic tests in ordinary CI and larger corpus/property/fault
runs in explicit extended targets; do not game coverage with vacuous snapshots.

Prove the runtime contract with the built binary in disposable minimal
environments: empty HOME/XDG directories, no LSPs/runtimes on PATH, network
disabled, no prebuilt index, and read-only source for preview. Inspect filesystem
and child-process/network activity using platform-appropriate tracing or sandbox
controls. Compare outputs across clean repeated invocations and thread counts.
Only explicit application/report artifacts may persist; no server, registration,
trust database, hidden cache or background process. Test native macOS and the
Linux-musl architectures claimed by release packaging; verify bundled grammar
external scanners link correctly. Do not trade away static Linux packaging to
make an adapter easier.

Performance comes after working features and passing accuracy gates. Record
baseline latency/RSS/binary size early, but do not reject a more accurate engine
merely for costing more than regex. Add pinned realistic repos for A-C and mixed
monorepos, including no-match, sparse/dense edits, deep imports, many small
files, large files, batch and parse-error cases. Measure whole-process startup,
discovery, parse, resolve, plan, diff and apply separately; label OS-cache state.
Every measurement starts a fresh process without a semantic index. Report
latency distributions and peak memory, not only a selected warm median.

Compare text with the existing fastermod baseline and structural operations
with ast-grep/srgn only where output semantics match. Semantic benchmarks need
expected identity/edit sets; a tool doing less work is not an equivalent speed
baseline. Optimize through profiling, bounded parallelism, arenas and proven
conservative prefilters. Never reduce language coverage, truncate resolution,
add hidden state or weaken refusals to win a benchmark. Keep better correctness
when it costs time and explain the tradeoff.

## Unattended work loop and completion

For each slice: reproduce current behavior, add a failing meaningful witness,
implement, run focused and relevant workspace checks, inspect the actual diff,
review safety/accuracy, and commit. Update ticket evidence with commands,
results, commit IDs, fixtures and remaining limits. Discover and use installed
project review/orchestration skills where available, without hardcoding model
providers or assuming tools that are not present. An independent reviewer is
valuable; absent one, report self-review accurately.

Continue until the requested deliverable is met or a specific external blocker
prevents further independent work. Do not stop after planning, scaffolding,
parser integration, one successful demo, or the first three languages. Split a
large ticket into concrete sub-tickets rather than silently narrowing it. Park
only the blocked slice, record a reproduction and next action, and continue
other work. Complexity is not by itself an owner decision. Never replace an
unfinished implementation with invented results or a 'done with limitations'
label that hides unmet acceptance criteria.

The final report must include the actual CLI, capability matrix, examples,
verification commands/results, corpus and correctness evidence, absence-of-state
proof, cold-start measurements, dependency/license decisions, commits, and
remaining unsupported constructs. Mark the overall campaign complete only when
the required inventory and operation gates pass. Otherwise report partial
completion and keep specific tickets open. Do not publish a release without
separate permission.

## Primary-source starting points

Checked 2026-11-15; inspect current APIs, licenses and maintenance again during
REFAC-5. These are candidates and evidence, not blanket adoption approvals.

- [ast-grep FAQ](https://ast-grep.github.io/advanced/faq): structural matching does
  not supply scope analysis or type information.
- [ast-grep source](https://github.com/ast-grep/ast-grep) and
  [language reference](https://ast-grep.github.io/reference/languages): inspect
  library boundaries and actual bundled grammars.
- [srgn source](https://github.com/alexpovel/srgn): syntax-scoped replacement
  reference, not an assumed general semantic rename engine.
- [Oxc semantic API](https://docs.rs/oxc_semantic/latest/oxc_semantic/): reusable
  JS/TS scope and symbol analysis; validate its actual resolution domain.
- [GitHub stack-graphs](https://github.com/github/stack-graphs): name-resolution
  implementation and rules; archived and unsupported by GitHub since 2025.
- [Tree-sitter](https://tree-sitter.github.io/tree-sitter/): concrete syntax trees
  and grammar interfaces, not language-independent binding semantics.
