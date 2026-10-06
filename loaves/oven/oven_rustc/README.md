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

## Stage-zero SDK closure

`sdk_closure::prepare_sdk_seed` accepts the SDK resolution seed, the archive directory, a private output/store directory, the explicit pinned rustc executable, and the local index repository. It reads index facts from commit `452504b51712b6a3ac12a2e1b79ba69fff55e6b6`, verifies archive digests before parsing, and compiles independent debug units through the existing direct executor. Build scripts and Cargo declarations remain inert. Exact fact selection includes toolchain, target, profile, and the complete feature set.

The returned closure retains store leases for its compiled outputs and source roots. Its report distinguishes compilation, reuse, missing-fact refusals, and unavailable dependents. `require_complete` prevents SDK publication while any selected unit is unavailable. Matching cfg, generated-file, and environment facts are supported; link/tool facts require publisher assets that this stage-zero entry point does not yet admit. The driver helper `seal_sdk_closure_inspection_sources` writes the existing direct-inspection source authority only for a complete closure. This entry point is not wired into automatic SDK component preparation, and the legacy inspection declaration reader still requires conversion before consuming these source-only roots.

The `sdk_seed` example takes `LOCK BLOBS OUTPUT RUSTC INDEX` and prints measured JSON. Building that example with Cargo builds the compiler tooling; running it compiles the adopted closure directly with rustc.
