# Format Incan source

Run the formatter on files or directory trees:

```bash
incan fmt src tests
incan fmt --check src tests
```

Use `--check` in CI: it leaves source unchanged and returns nonzero when a file would change. Use `--` before a path that starts with `-`.

Directories are searched recursively, skipping hidden directories, `target` and `node_modules`. A file that cannot be parsed is reported; fix its syntax before formatting it.

## Check the result

Run check mode after formatting. Formatting preserves parseable literal spellings, including string escapes and multiline strings. The canonical style uses four-space indentation and a 120-character target. Long lines are best-effort rather than an unconditional maximum.

The [code style reference](../../language/reference/code_style.md) defines source rules. Repository Rust uses `make fmt`; Incan uses `incan fmt`.

## Format a source with dependencies

Keep the matching package manifests available under the selected toolchain. Formatting reads a file's unit vocabulary; it does not build missing companion crates or substitute dependency source for absent compiled manifests.
