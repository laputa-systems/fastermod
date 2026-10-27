# fastermod

`fastermod` applies regular-expression substitutions across files and shows each
proposed change for review. It is a partial replacement for
[codemod](https://github.com/facebookarchive/codemod).

Supported on macOS and Linux. Windows is not supported.

## Install

Install from the repository with Cargo:

```sh
cargo install --git https://github.com/laputa-systems/fastermod fastermod
```

When the manual release workflow runs, pre-release binaries are listed on the
[GitHub Releases page](https://github.com/laputa-systems/fastermod/releases).

## Use

```sh
fastermod 'OldName' 'NewName' src/
```

The first argument is a regular expression; the second is its replacement.
`^` and `$` always match line boundaries. `-m` only makes `.` match a newline.
Without a path, `fastermod` searches the current directory. By default it skips
hidden and ignored files. Accepted replacements are written to the matched file.

Use `-S` or `--smartcase` to rename text while keeping its casing:

```sh
fastermod -S tansu2 tansu ~/d/tansu
```

When `REGEX` has no uppercase letters, `--smartcase` matches without regard to
case. It changes `tansu2`, `TANSU2`, and `Tansu2` to `tansu`, `TANSU`, and
`Tansu`. Mixed-case matches use `SUBST` as written. If `REGEX` has uppercase
letters, matching stays case-sensitive unless you also use `--ignore-case`.

For each match, fastermod displays a diff and asks what to do. Press Enter or
`y` to apply the change, `n` to skip it, `e` to edit the file, `A` to accept all
remaining matches, `E` to accept and edit, or `q` to quit. Editing uses
`$EDITOR`, defaulting to `vim`.

For unattended changes, add `--accept-all`. `--print-changed-files` prints
changed paths; it is useful with `--accept-all` in scripts. Use `-F` or
`--fixed-strings` when the search text should be treated literally, and
`--help` for case, multiline, extension, glob, hidden-file, and ignore options.

## Help and license

Report problems on the [issue tracker](https://github.com/laputa-systems/fastermod/issues).
See [LICENSE](LICENSE) for the repository's license information.
