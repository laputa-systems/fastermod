# Language coverage

`fastermod --languages` (add `--json` for machine output) is the live
capability matrix: every registered language row carries its extensions,
grammar license, whether `ident`/`rewrite`/binding rename are supported,
per-language limitations, and fixture names. This file records the
inventory-level facts that are not visible from the registry itself.

## Registered languages

44 rows: rust, python, javascript, typescript, tsx, go, c, cpp, java,
csharp, kotlin, swift, php, ruby, dart, lua, bash, scala, elixir,
haskell, zig, r, julia, objective-c, erlang, ocaml, ocaml-interface,
fsharp, fsharp-signature, groovy, fortran, matlab, vb-dotnet, css, html,
json, yaml, hcl, nix, markdown, solidity, scss, toml, postgresql.

- `ident` and `rewrite` are syntactic for every row.
- Binding rename is `workspace` scope for rust, python, javascript,
  typescript/tsx; `local` scope for the rest. Renames fail closed on
  unresolved references, dynamic dispatch, and unknown receivers.
- `objective-c`, `ocaml-interface`, and `fsharp-signature` are
  syntax-only (no binding analyzer).
- `.m` (MATLAB vs Objective-C), `.sql` (PostgreSQL vs other dialects),
  and similar extension ambiguities require `--lang`.

## Evidenced-open: no compatible grammar

These languages have no ABI-compatible tree-sitter grammar and are not
in the registry. They are intentionally absent rather than silently
approximated:

| Language | Reason |
| --- | --- |
| Perl | `tree-sitter-perl` crates pin a conflicting `tree-sitter` links version |
| Clojure | grammar crate pins `tree-sitter` 0.25, incompatible with 0.27 |
| COBOL | only binary-only grammar crate published |
| PowerShell | no grammar in ast-grep-language 0.51.2 or crates.io compatible set |
| SQLite | `willbooster-tree-sitter-sql` is ABI-incompatible (parser panic) |
| MySQL | same crate family; no verified compatible grammar |

Reviving any of these needs a vendored or ABI-matched grammar; until
then files in these languages are skipped with an explicit diagnostic.

## Embedded regions

`src/embed.rs` maps `<script>`/`<style>` blocks in HTML/Vue/Svelte hosts
and fenced code blocks in Markdown to inner grammars. `--ident`,
`--rewrite`, and `--rules` ops edit inside regions while preserving host
bytes and delimiters. Vue/Svelte host grammars themselves are
unavailable (the Vue grammar's vendored scanner collides with the
bundled HTML grammar symbols at link time), so host files are handled
through region extraction only. `--rename` inside embedded regions is
unsupported: binding analysis is per-file and cross-language identity
is never inferred.
