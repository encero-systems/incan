# `incan_provider`

Ring: **compiler**

Provider and SDK contracts (manifest types, component catalog, inventory) and their loaders.

## Current sources

- `loaves/compiler/incan_provider/src/` — the loaders: `inventory`, `requirements`, `sdk_store`, `sdk_build`, `vocab_extraction`, `effect_digest`, `lock_semantics`
- `loaves/compiler/incan_provider/src/compiled_sdk.rs`
- `loaves/compiler/incan_provider/src/dependency_resolver.rs` — `requirements` reads it, so it sits here rather than in the driver

`loaves/compiler/incan_frontend/src/library_manifest/` and `loaves/compiler/incan_frontend/src/semantics_registry.rs` went to `incan_frontend` instead, with the provider *contract* (`provider/{plan,sdk,features,error}`): the typechecker reads the plan and the manifest model embeds frontend types in 25 places, so the cut that removes the frontend ↔ library_manifest cycle runs below the frontend, not beside it. This crate re-exports that contract so `incan_provider::ProviderPlan` is one name for one type.

## May depend on

`kernel`, `incan_frontend`, `rust_inspect`, `oven_model`, `oven_store`, `oven_rustc`

`test_support` (feature `test_support`) holds the fixtures the driver's tests share with this crate's.

## SDK publication identity

Development checkouts are recognized by the workspace and compiler-ring layout, including the emitter crate and the associated stdlib root. Their provider-store identity combines compiler effects, publication configuration, compiler/codegen version, distribution profile and the resolved workspace lock. Installed layouts use source and executable bytes instead.

The effect inputs include the stdlib's Rust vocab companion and its vocab contract dependency because they contribute published metadata and executable desugarers. Publication configuration includes stdlib TOML files, standalone Cargo locks and the Cargo manifests owning the Rust effect roots. These configuration inputs are conservatively byte-exact: even a formatting-only manifest edit can miss the cache. Generated target directories are excluded. Ordinary source comments retain the effect digest's semantic treatment.

The v5 store key partitions earlier entries once. Existing entries remain immutable; a changed catalog publishes under a new identity, and an unchanged catalog reuses its existing inventory. This key is separate from the compiler-suite foundation key and does not replace artifact integrity checks.
