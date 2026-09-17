# `incan_driver`

Ring: **compiler**

Owns compilation sessions, module graphs, build orchestration, generated caches and replacement compatibility. The CLI owns clap parsing; some driver diagnostics still print directly to stderr, including declared backend-fallback notices.

## Current sources

- `loaves/compiler/incan_driver/src/build/` — build orchestration extracted from the CLI
- `loaves/compiler/incan_driver/src/session.rs` — compilation sessions
- `loaves/compiler/incan_frontend/src/parsed_module.rs` — parsed-module data consumed by the driver
- `loaves/compiler/incan_driver/src/generated_cache.rs`
- `loaves/compiler/incan_driver/src/replacement_compatibility.rs`
- `loaves/compiler/incan_frontend/src/compiler_stack.rs` — shared frontend stack configuration
- `loaves/compiler/incan_driver/src/backend/project/generator.rs` — generated Rust project rendering

## May depend on

`kernel`, `incan_frontend`, `incan_ir`, `incan_emit`, `incan_provider`, `rust_inspect`, `incan_oven_facet`, `oven (model, store and rustc, as a consumer)`

This is what `incan-lsp` and `incan` both link. Its existence is what makes the LSP compile without the CLI (audit finding 1).

`loaves/compiler/incan_driver/src/lib.rs` is the crate root. Build orchestration lives directly under the crate's source directory beside `backend/`, `inspect/`, `generated_cache.rs`, `replacement_compatibility.rs` and `shadow_support.rs`. The generated-project modules remain in `loaves/compiler/incan_driver/src/backend/project/`; moving their planning, lock projection, Cargo manifest and runner responsibilities into the Oven ring still requires dependency inversions.
