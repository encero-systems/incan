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

Formatting describes each file's unit with the `debug` profile and default features, and parses it with the vocabularies its dependencies publish. When an active local dependency declares a vocabulary entry, Oven prepares the dependency graph before parsing so its registered syntax is available on a first format. Formatting uses those descriptors without expanding consumer syntax through a desugarer. When no active local dependency declares a vocabulary entry, formatting does not build dependencies. A file whose unit cannot be described or whose dependency preparation fails is reported, and the remaining files are still formatted.

Formatting accepts public reexport syntax before a module is placed under `src/`; checking and compilation enforce that placement rule.
