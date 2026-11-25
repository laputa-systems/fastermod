Prefix: FAST
Next: 14

### FAST-13 · P3 · C ident edits skip macro-braced test files

- **Area:** fastermod · C language coverage
- **Where:** src/syntax.rs
- **Problem:** fastermod 0.0.1 on macOS arm64 skips Asterinas regression C files such as test/initramfs/src/regression/process/sched/sched_attr_getset.c for --ident because FN_TEST/END_TEST macros supply the function braces and the unpreprocessed file parses with syntax errors; the requested rename of static sched_setattr wrappers therefore makes no edit, while fixed-string mode is a workable but less precise fallback.
- **Recommendation:** Support an evidenced-safe syntactic token path for C files whose parse errors are confined to macro-generated structure, or document and test the fixed-string fallback for macro-braced C; acceptance is a fixture shaped like the Asterinas FN_TEST file where --ident either performs only exact identifier-token edits with zero nonidentifier replacements or fails with a documented actionable diagnostic.
- **Fix with:** language-coverage
- **Effort:** M

