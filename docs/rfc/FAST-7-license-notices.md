# FAST-7: License notices

Status: draft

## Summary

The tree currently tells two license stories. `LICENSE` and the `fastermod` manifest grant MIT. Two source files still carry Meta's Apache-2.0 header, and `imara-histogram-diff` is marked Apache-2.0 because it was extracted from `imara-diff`.

## Motivation

Apache-2.0 requires the license text and the original notices to stay with a derivative. A release zip and a Git checkout today include `LICENSE` (MIT) and not the Apache-2.0 text. Cargo metadata for `fastermod` says `MIT`, which overclaims if those Apache-covered files are still derivatives.

What is known from this tree:

- `src/main.rs` and `src/terminal.rs` name Meta and Apache-2.0.
- `imara-histogram-diff` documents an extraction from `imara-diff`. Upstream `imara-diff` is Apache-2.0, and this crate's manifest says the same.
- The histogram sources do not repeat a copyright notice.
- No file in the repo is a copy of the Apache-2.0 license.

This review did not byte-compare the sources with upstream `fastmod` or `imara-diff`.

## Design

Until you decide otherwise, treat the Meta headers and the histogram crate as Apache-2.0 obligations.

- Add the Apache-2.0 license text as `LICENSE-APACHE`.
- Point `LICENSE` at both files, and say the MIT grant covers only files that are not already under Apache-2.0.
- Leave the Meta headers in place.
- Add a short copyright and Apache-2.0 notice to the histogram crate sources, naming the `imara-diff` origin the crate docs already state.
- Change `fastermod`'s `license` field only after the choice below. Do not leave it as `MIT` if the binary links the Apache-2.0 crate.

## Alternatives considered

- Relicense the whole repo to MIT and delete the Apache headers. Valid only if you have the rights to every remaining line. Nothing in this review shows that.
- Relicense the whole repo to Apache-2.0 and replace `LICENSE`. Simpler for recipients, and it drops the MIT grant you added in 2026.
- Dual-license new code as `MIT OR Apache-2.0`, and leave extracted files Apache-2.0 only. Accurate, and the manifest has to name both.

## Open questions

- Are `src/main.rs` and `src/terminal.rs` still derivatives of Meta `fastmod`, or have those files been rewritten enough that the header is stale?
- Should new code stay MIT, move to Apache-2.0, or be offered as `MIT OR Apache-2.0`?
- Who is the copyright holder to name on the histogram extraction?

## Decision

Not decided.
