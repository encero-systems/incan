# `incan_emit`

Ring: **compiler**

IR-to-Rust emission with syn/quote, conversions, prettyplease formatting, replacement lowering.

## Moves here from

- `src/backend/ir/emit/`
- `src/backend/ir/conversions.rs`
- `src/backend/ir/codegen.rs`
- `src/backend/replacement/`

## May depend on

`kernel`, `incan_ir`

Pure emission. `src/backend/project/` does NOT come here; it is Oven code (33 imports of `crate::oven`) and moves to the oven ring.

This directory is a layout skeleton. It holds no code yet; `src/` is a placeholder for the conventional crate root.
