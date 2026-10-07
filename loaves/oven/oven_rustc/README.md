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

`sdk_closure::prepare_sdk_seed` accepts the SDK resolution seed, the archive directory, a private output/store directory, the explicit pinned rustc executable, and the local index repository. It reads index facts from commit `6ec35e0d7e2d202496e2f7a108bb5111b6a9ff87`, verifies archive digests before parsing, and compiles independent debug units through the existing direct executor. `prepare_closure` is the shared entry point for an explicit index commit, target, and debug/release profile. Build scripts and Cargo declarations remain inert. Exact fact selection includes toolchain, target, profile, and the complete feature set.

The returned closure retains store leases for its compiled outputs and source roots. Its report distinguishes compilation, reuse, missing-fact refusals, and unavailable dependents. `require_complete` checks whether every selected unit is available. Matching cfg, generated-file, and environment facts are supported. Native-link facts use the publisher executor and a digest-matched executable owner supplied through `INCAN_OVEN_LINK_OWNERS`; Oven writes and stores the indexed static archive, then applies receipt-bound `-L` and `-l static=` inputs to rustc. Source catalogs containing inert Cargo metadata are refused by record name. Tool facts still require retained publisher assets not supplied by this entry point.

`inspection_project` exports the compiled subgraph with the same active aliases, host/target edges, features, fact cfg values and environment used by compilation. Source and generated-output paths refer to the retained immutable store, rather than temporary archive extraction. The driver helper `seal_sdk_closure_inspection_sources` publishes this graph, its source authority and the named closure report. The direct inspector validates source containment and edge indices, then loads this graph without reading a Cargo manifest or lock. Source-only Loaf manifests also have a compatibility projection for the existing declaration reader. Automatic SDK component preparation and component omission/import diagnostics still require wiring; publishing the compiled source subgraph alone does not publish a complete SDK inventory.

`compile_local_sdk_facet` extends the retained closure with a local Loaf's Rust facet. It snapshots the declared Rust source directory and Loaf declaration, selects declared dependency aliases from already compiled units, binds all snapshot bytes and dependency outputs to a direct-rustc receipt, and publishes the source and library under an execution lease. Procedural-macro dependencies select host units. The language registry's ten embedded component Loaf declarations retain their relative include layout in that snapshot. Missing or ambiguous dependency bindings fail explicitly; this API never resolves them or reads Cargo metadata.

The `sdk_seed` example takes `LOCK BLOBS OUTPUT RUSTC INDEX [INSPECTION_PROJECT [LOCAL_SDK_ROOT]]` and prints measured JSON. The inspection path receives the frozen graph. Supplying a local SDK checkout additionally attempts `incan_lang`, `incan_derive`, `incan_web_macros`, `incan_vocab`, and the core, async, data, web, and testing runtime facets against the seed. Local failures appear separately as `local_failures`, the available subgraph is retained, and any local failure makes the command fail. These are native-facet measurements, not automatic SDK component publication. Building that example with Cargo builds the compiler tooling; running it compiles the adopted closure and requested facets directly with rustc.
