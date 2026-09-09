# `incan_inspect`

Ring: **compiler**

Rust signature inspection for `rust::` imports and codegraph export.

## Moves here from

- `crates/rust_inspect/`
- `src/rust_inspect/`
- `src/cli/commands/codegraph.rs (export half)`

## May depend on

`kernel`, `rust-analyzer crates`

Loads from the Oven project-JSON projection (RFC 119) instead of `ra_ap_load-cargo`, which retires `cargo_metadata`, `notify`, and the vendored `ra_ap_proc_macro_api` patch.

This directory is a layout skeleton. It holds no code yet; `src/` is a placeholder for the conventional crate root.
