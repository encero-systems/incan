# Tooling: Start here

This section covers the Incan tooling experience:

- SDK install and project creation
- CLI (`check`, `build`, `run`, `fmt`, `test`, `inspect`, and `tools`)
- editor setup and LSP

If you’re not sure where you fit, start at [Start here](../start_here/index.md).

## Tutorials (learn)

- [Getting Started](tutorials/getting_started.md)

## How-to guides (do)

- [Editor Setup](how-to/editor_setup.md)
- [LSP](how-to/lsp.md)
- [Formatting with `incan fmt`](how-to/formatting.md)
- [Testing](how-to/testing.md)

## Reference (look up)

Single source of truth pages under Tooling:

- [Install and run](how-to/install_and_run.md) (SDK-first)
- [CLI reference](reference/cli_reference.md) (commands/flags/env vars)
- [Codegraph inspection](reference/codegraph_inspection.md) (compiler-backed source-structure JSONL)

Use the CLI reference for `incan build --report json` and `incan inspect rust --format json`; those commands share the same 0.4 semantic-inspection plane as diagnostics and codegraph export.

## Canonical first-contact flow

```bash
incan new hello --yes
cd hello
incan run
incan test
incan build --release
```

Repository `make` targets remain the contributor path for working on Incan itself.
