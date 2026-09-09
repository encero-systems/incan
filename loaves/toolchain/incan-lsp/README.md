# `incan-lsp`

Ring: **toolchain**

Language server over the driver.

## Moves here from

- `src/bin/lsp.rs`
- `src/lsp/`

## May depend on

`compiler`

Links `incan_driver`, never `incan`. Owns the tokio and tower-lsp dependencies with a narrowed tokio feature set.

This directory is a layout skeleton. It holds no code yet; `src/` is a placeholder for the conventional crate root.
