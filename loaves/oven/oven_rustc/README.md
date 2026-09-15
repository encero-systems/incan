# `oven_rustc`

Ring: **oven**

Direct-rustc planning and execution, host/target unit graph, build-script and proc-macro host providers.

## Sources and remaining moves

- `loaves/oven/oven_rustc/src/rustc.rs` with `loaf.rs`, `loaf_mirror.rs`, `plan/`, `native_test/`, `native_contract.rs`, `interop.rs` and `legacy_cargo/`, one strongly connected component on dev.5
- `loaves/compiler/incan_driver/src/backend/project/plan.rs`, `lock_projection.rs`, `mod.rs` remain in the driver pending their dependency inversions
- The Oven-side plan selection and composition API extracted under #1266 already lives in `loaves/oven/oven_rustc/src/plan.rs` and its `plan/` submodules; command-level preparation calls it from `loaves/compiler/incan_driver/src/build/plan_selection.rs`.

## May depend on

`oven_model`, `oven_store`

`generator.rs` does not come here: it renders a project from the checked program and provider facts and belongs to `compiler/incan_driver`. The generated-project stdlib baseline (`async, json, ordinal`) arrives as plan facts from the facet rather than as a generator constant.

`interop` and `legacy_cargo` are modules here until their edges into `loaf` and `rustc` are cut; `oven_interop` and `oven_cargo_compat` are the crates they become. `loaves/oven/oven_rustc/src/fixtures/` holds the compiler-suite and release stdlib fixtures the Loaf envelopes are baked from.
