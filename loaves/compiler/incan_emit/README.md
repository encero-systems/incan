# `incan_emit`

Ring: **compiler**

IR-to-Rust emission with syn/quote, conversions, prettyplease formatting and backend selection.

## Current sources

- `loaves/compiler/incan_emit/src/emit/`
- `loaves/compiler/incan_emit/src/conversions.rs`
- `loaves/compiler/incan_emit/src/codegen.rs`

## May depend on

`kernel`, `incan_frontend`, `incan_ir`, `rust_inspect`, and `oven_model` for the manifest and receipt types

Pure emission. Generated-project orchestration remains in `loaves/compiler/incan_driver/src/backend/project/`; its planned Oven extraction is separate from this crate.

Emission, conversions, ownership, the trait-bound inference and the codegen entry point live here with backend selection; `checked_program` holds the tests that drive a checked program through to Rust.
