# REFAC-5 — Reuse spike results

Date: 2026-11-15. Toolchain: `nightly-2026-11-09`. Spike code:
`spikes/reuse-spike/` (detached Cargo workspace, vendored deps, not shipped).

## What was exercised

| Candidate | Version | License | Evidence |
| --- | --- | --- | --- |
| `tree-sitter` + `tree-sitter-{rust,python,javascript,typescript}` | 0.27.2 / 0.24.2 / 0.25.0 / 0.25.0 / 0.23.2 | MIT | Parsed Rust/Python/JS/TS samples with `has_error=false`; enumerated `identifier` nodes with exact byte spans (`main` 3..7, `counter` 20..27). |
| `ast-grep-core` + `ast-grep-language` | 0.51.2 | MIT | Pattern `helper($$$ARGS)` matched exactly one call in Rust source — the `// helper(c)` comment decoy did not match. `replace_all` produced a byte-range `Edit`. Same pattern matched Python. `SupportLang::from_path` maps `main.rs`→Rust, `script.py`→Python. `builtin-parser` feature bundles 26 grammars (Rust, Python, JS/JSX, TS/TSX, Go, C, C++, C#, Java, Kotlin, Swift, PHP, Ruby, Dart, Lua, Bash, HTML, CSS, JSON, YAML, TOML-adjacent, Scala, etc.), each behind its own cargo feature. |
| `oxc_{allocator,ast,parser,semantic,span,syntax}` | 0.158.0 | MIT | `Parser` + `SemanticBuilder` on a JS snippet: 0 diagnostics, 2 scopes, 2 symbols, 2 references; symbol `counter` resolved to declaration bytes 21..28 with 1 resolved reference and `BlockScopedVariable` flag. All in-process; no LSP, no node runtime. |
| `srgn` | 0.14.3 | MIT | Library exists (`ScopedView` over tree-sitter scopes: string/comment/literal masking + actions) but is binary-first, single-maintainer, narrower grammar set. Reviewed source; **not adopted** — its scope-masking idea is noted for our own text/ident scope handling. |
| `stack-graphs` (GitHub) | — | — | **Rejected.** Repository archived; GitHub states it is unsupported. Not a maintained turnkey resolver. |

## Decision

1. **Syntax engine: `ast-grep-core` + `ast-grep-language`.**
   One dependency gives pattern matching with `$$$`/`$VAR` metavariables,
   byte-range edits, language detection from path/extension, and 26
   bundled grammars behind per-language cargo features (compile only what
   ships). It is tree-sitter underneath, so raw CST access remains
   available through `tree-sitter` types where needed. ast-grep explicitly
   does not do scope/type analysis — it is the `--rewrite`/`--rules` and
   syntax-role engine, never the rename engine.

2. **JS/TS binding engine: `oxc_*` 0.158.**
   `oxc_semantic`'s `Scoping` already exposes symbol ids, names, declaration
   spans, scope tree and resolved reference ids — the exact inputs REFAC-7
   needs for local rename. Pure Rust, very actively maintained. Domain is
   JS/TS syntax semantics only: no cross-file type information, no
   whole-program resolution; workspace/module references still need our
   own import/export model on top.

3. **Other languages' syntax layer: direct `tree-sitter` 0.27 +
   `tree-sitter-*` grammar crates.**
   For wave B/C languages outside ast-grep-language's set (or where its
   pattern preprocessing gets in the way), the same tree-sitter runtime
   with per-language grammar crates keeps one CST substrate. Grammar
   crates are tiny C bundles; license must be checked per grammar at
   adoption time (most are MIT; record in the capability registry).

4. **Name resolution for non-JS/TS languages: explicit per-language
   resolver built on the CST.**
   No maintained, stateless, in-process resolver crate exists for Rust,
   Python, Go, etc. stack-graphs is archived; LSP-based resolution is
   forbidden by the campaign. REFAC-7 implements scope/binding rules per
   language on top of the chosen CST — this is the campaign-anticipated
   "simpler explicit resolver" outcome.

## Portability and supply-chain findings

- All chosen crates are MIT, pure-Rust APIs over bundled C grammar code;
  they compile to a static binary with no runtime Node/Python/JVM and no
  daemon. A C toolchain is required at build time (already true for the
  `imara-histogram-diff`/`rustix` build on unix targets; verify musl).
- `rust-version`: tree-sitter 1.90, ast-grep 1.90, oxc 1.97 — all below
  the pinned `nightly-2026-11-09` toolchain.
- `ast-grep-language` has no per-language features: `builtin-parser`
  bundles all 26 grammars (or `napi-lang` for a JS/TS/CSS/HTML subset).
  REFAC-6 ships the full `builtin-parser` set — waves B–D need most of
  those grammars anyway, and compile cost is paid once.
- The repo's crates mirror (`source.stuff`) originally did **not** carry
  these crates; they were published to it via
  `stuff publish cargo --lockfile` (mirroring the spike's Cargo.lock,
  ~252 MB of crate payloads, 2026-11-15). `cargo` now resolves
  ast-grep-*/tree-sitter-*/oxc_* normally; the spike's vendored copy
  remains gitignored reference material.
- Versions are pinned exactly in `spikes/reuse-spike/Cargo.lock`
  (2026-11-15): tree-sitter 0.27.2, ast-grep-* 0.51.2, oxc_* 0.158.0.

## Unresolved semantics (carried into REFAC-6/7)

- ast-grep pattern semantics (`$$$` multi-metavar, `with_strictness`,
  relational rules via `AstGrep` config) need a conformance pass against
  our rule format; RULES-1 tickets own the declarative rule design.
- oxc semantic covers one file at a time; cross-file JS/TS rename needs
  our own module/import model (REFAC-8).
- Identifier *role* classification (def vs ref vs type vs field) is not
  provided by any candidate; it comes from per-language node-kind tables
  built on the CST during REFAC-6.
- Python/Rust/Go/etc. binding resolution is ours to write; the spike
  proves the CST substrate, not the resolver.
