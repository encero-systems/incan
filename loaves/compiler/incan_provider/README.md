# `incan_provider`

Ring: **compiler**

Provider and SDK contracts (manifest types, component catalog, inventory) and their loaders.

## Moves here from

- `src/provider/` — the loaders: `inventory`, `requirements`, `sdk_store`, `sdk_build`, `vocab_extraction`, `effect_digest`, `lock_semantics`
- `src/compiled_sdk.rs`
- `src/dependency_resolver.rs` — `requirements` reads it, so it sits here rather than in the driver

`src/library_manifest/` and `src/semantics_registry.rs` went to `incan_frontend` instead, with the provider *contract* (`provider/{plan,sdk,features,error}`): the typechecker reads the plan and the manifest model embeds frontend types in 25 places, so the cut that removes the frontend ↔ library_manifest cycle runs below the frontend, not beside it. This crate re-exports that contract so `incan_provider::ProviderPlan` is one name for one type.

## May depend on

`kernel`, `incan_frontend`, `oven_model`, `oven_store`, `oven_rustc`

`test_support` (feature `test_support`) holds the fixtures the driver's tests share with this crate's.
