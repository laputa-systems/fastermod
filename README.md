# fastermod

A fast partial replacement for [codemod](https://github.com/facebookarchive/codemod):
a tool for large-scale codebase refactors that can be partially automated but
still need human review. It is derived from Meta's fastmod.

```sh
fastermod [OPTIONS] <REGEX> <SUBST> [FILE_OR_DIR]...
```

Each match is shown as a diff for you to accept or reject unless `--accept-all`
is given. Run `fastermod --help` for the options (multiline, case-insensitive,
extension and glob filters, fixed strings, hidden and ignored files, and so on).

The workspace also contains `imara-histogram-diff`, the histogram line diff used
to render changes.

## Development

```sh
make build                  # debug build
make test                   # cargo test
make lint                   # clippy --fix
make bench                  # rustybench baseline comparison (needs ../rustybench)
make release                # static release build (build-std, immediate-abort panics)
make install                # release build copied to ~/usr/bin
```

`make release-dynamic`, `make verify-release`, and `make verify-release-dynamic`
are used by the Linux release image (`Dockerfile`) and the release workflow.
`TARGET` selects the target triple; it defaults to the host, with Linux hosts
mapped to musl.
