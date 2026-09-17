# `oven_rustc`

Ring: **oven**

Direct-rustc planning and execution, the Loaf model, host/target unit graph, build-script and proc-macro host providers, and the Cargo-free wire contract the native route reads.

## Sources

- `loaves/oven/oven_rustc/src/rustc.rs` with `loaf.rs`, `loaf_mirror.rs`, `plan/`, `native_test/` and `native_contract.rs`
- `loaves/oven/oven_rustc/src/fixtures/` holds the compiler-suite and release stdlib fixtures the Loaf envelopes are baked from
- The Oven-side plan selection and composition API extracted under #1266 lives in `loaves/oven/oven_rustc/src/plan.rs` and its `plan/` submodules; command-level preparation calls it from `loaves/compiler/incan_driver/src/build/plan_selection.rs`
- `loaves/compiler/incan_driver/src/backend/project/plan.rs`, `lock_projection.rs`, `mod.rs` remain in the driver pending their dependency inversions

## May depend on

`oven_model`, `oven_store`

Nothing here runs Cargo: the explicit compatibility baker is `oven_cargo_compat` and the interop shims are `oven_interop`, both over this crate. `native_contract.rs` declares the types those crates produce and this crate's native route consumes, so a publisher and a reader hold one type. `generator.rs` does not come here: it renders a project from the checked program and provider facts and belongs to `compiler/incan_driver`. The generated-project stdlib baseline (`async, json, ordinal`) arrives as plan facts from the facet rather than as a generator constant.
