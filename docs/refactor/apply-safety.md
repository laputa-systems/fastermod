# Apply safety policy

How fastermod writes files, what it guarantees, and what it explicitly does
not. This document is the normative companion to the plan/apply code in
`src/plan.rs` and `src/review.rs`.

## Write mechanics

- Source edits go through `Fastermod::save`: a same-directory temporary
  file (`.{name}.tmp.{pid}-{counter}`), `fsync`, then `rename(2)` over the
  target, followed by a best-effort parent-directory `fsync` on Unix.
- Each file replacement is atomic: readers see either the complete old
  file or the complete new file, never a torn write.
- There is **no cross-file transaction**. A multi-file apply can stop
  partway (I/O error, cancellation, permission change) leaving earlier
  files applied and later files untouched. `plan::apply` attempts every
  file, records `PlannedFile::applied` per file, and its error message
  lists exactly which paths were applied and which failed. JSON output
  carries the same evidence via top-level `applied` and per-edit
  `applied` flags plus an `apply-failed` diagnostic.
- Plan application preflight checks plan validation, root containment,
  file-type checks, and staleness (content hash) all run before the first
  source write. A stale, escaped, symlinked or unreadable planned file
  refuses the operation before any source is written.

## Staleness and TOCTOU

- Before writing, each file is re-read and its FNV-1a content hash
  compared to the analysis snapshot. Any change refuses the operation;
  fastermod never overwrites bytes it did not analyze.
- Residual TOCTOU: an attacker or concurrent process can modify a file in
  the window between the staleness read and the `rename`. Ordinary
  editors and builds make accidental hits unlikely but not impossible.
  There is no compare-and-swap primitive on POSIX filesystems that would
  close this without mandatory locking; we document rather than pretend.
- Saved plans (`--plan`/`--apply-plan`) extend the same model:
  apply revalidates hashes and roots and rejects stale plans instead of
  replanning.

## Links and file types

- Discovery never follows symlinks (`ignore` walker default) and
  `save()` re-checks with `symlink_metadata`: a symlink or non-regular
  file is refused, never written through.
- Hardlinked files: the rename-replace strategy swaps only the edited
  directory entry. Other hardlinked names keep the old inode and old
  content. This is deliberate: it never propagates an edit to names the
  operation did not analyze, and it cannot corrupt the shared inode.
- FIFOs, sockets, devices and directories are never matched or written.

## Metadata policy

- The staged file inherits the target's permission bits before rename,
  so the executable bit and mode survive the replace.
- Extended attributes, ACLs and resource forks are **not** preserved:
  rename installs a fresh inode. This is an explicit limitation, not a
  silent one; tools that depend on xattrs/ACLs should treat fastermod
  output as a plain-file rewrite.
- Ownership is not changed; rename preserves the creating user, so
  applying to files owned by another user requires appropriate
  permissions and the result is owned by the invoking user.

## Cancellation and recovery

- SIGINT/SIGTERM mid-apply leaves per-file atomicity intact: completed
  files are fully written, the in-flight file is either old or new, and
  at most one `.{name}.tmp.*` staging file per interrupted write can
  remain in the target directory. These are ordinary dotfiles, safe to
  delete; no daemon or index is involved.
- There is no automatic rollback. Recovery is: inspect the apply report
  (stderr message and JSON `applied` flags), then re-run the operation.
  Staleness checks make re-running safe against the already-written
  files only if the pattern no longer matches them; otherwise restore
  from version control manually. fastermod never runs `git reset`,
  `git clean`, or checkout-based rollback.

## Roots and paths

- Plan application requires every file path to canonicalize inside a
  declared coverage root. `..` components, absolute-path escapes and
  symlink-resolved escapes are rejected before any write.
- The analyzed tree is the boundary; fastermod does not claim to reason
  about files outside the roots it was given.
