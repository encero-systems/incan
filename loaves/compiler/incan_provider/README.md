# `incan_provider`

Ring: **compiler**

Provider and SDK contracts (manifest types, component catalog, inventory) and their loaders.

## Current sources

- `loaves/compiler/incan_provider/src/` — the loaders: `inventory`, `requirements`, `sdk_store`, `sdk_build`, `sdk_native`, `vocab_extraction`, `effect_digest`, `lock_semantics`
- `loaves/compiler/incan_provider/src/compiled_sdk.rs`
- `loaves/compiler/incan_provider/src/dependency_resolver.rs` — `requirements` reads it, so it sits here rather than in the driver

`loaves/compiler/incan_frontend/src/library_manifest/` and `loaves/compiler/incan_frontend/src/semantics_registry.rs` went to `incan_frontend` instead, with the provider *contract* (`provider/{plan,sdk,features,error}`): the typechecker reads the plan and the manifest model embeds frontend types in 25 places, so the cut that removes the frontend ↔ library_manifest cycle runs below the frontend, not beside it. This crate re-exports that contract so `incan_provider::ProviderPlan` is one name for one type.

## May depend on

`kernel`, `incan_frontend`, `rust_inspect`, `oven_model`, `oven_store`, `oven_rustc`

`test_support` (feature `test_support`) holds the fixtures the driver's tests share with this crate's.

## SDK publication identity

Source SDK publication uses the SDK seed, source and executable bytes, compiler/codegen version, distribution profile, and the exact retained native receipts. The local language and vocabulary companions participate even though their source roots sit outside the stdlib directory. Cargo manifests, Cargo locks, build scripts, and generated target directories are excluded from this identity.

`SdkNativeInputs` selects admitted archive bytes with `INCAN_SDK_NATIVE_BLOBS`, the pinned index repository with `INCAN_SDK_NATIVE_INDEX`, and the active managed rustc. Its native store is retained independently of provider staging. Preparation compiles the adopted seed, local companions, and component Rust facets through the shared direct-rustc executor. Independent local facets continue after adopted-unit failures; publication requires every selected unit to succeed. Native failures and local facet failures are retained in `closure-report.json`.

`prepare_sdk_provider_inventory_with_native_publisher` stages frozen source authority, the exact inspection graph, native receipt and output catalogs, and component inventory together. The supplied in-process compiler callback owns checked component metadata and executable surfaces. Its payload must exclude Cargo metadata, build scripts, links, and special files; checked name and version must match the component declaration. Source changes during publication refuse the new generation. Completed generations are immutable, and only a complete generation replaces the discovery hint. Cache acquisition repairs an interrupted hint write. A callback failure leaves earlier generations and their discovery hint intact.

Native output coordinates bind store entry, receipt, package selection, relative library path, and output digest. Consumers reacquire the exact entries and validate their native bytes and sources. Rust inspection retains those leases through its workspace lifetime. Automatic discovery can reuse a source-current publication without archive or index discovery.

The automatic preparation entry point currently prepares the native closure and then refuses publication because the driver's checked component callback is not connected. It cannot invoke the former per-component subprocess publisher. Automatic SDK preparation, native bake/test selection, and vocabulary companion publication are therefore not complete. The inspector can consume a published generation's frozen graph when that graph covers the requested registry dependencies; project dependencies outside the seed still require their own authority.
