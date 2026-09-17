# `oven_interop`

Ring: **oven**

Receipt-bound native interop: the selected compiler and SDK facts that authorize a native bake, shim compilation through direct rustc, carriers and interop bundles, and the execution receipt a normal consumer binds to. The authored declarations and their portable lock projection stay in `oven_model::oven_interop`; this crate is what runs against them.

## Sources

- `loaves/oven/oven_interop/src/lib.rs` — formerly `oven_rustc::interop`, moved whole with its tests

## Depends on

`oven_model`, `oven_store`, and `oven_rustc` — the layout's line named the first two; the shim bake selects a direct-rustc plan for execution and reads the plan's artifact manifest, so the crate sits over `oven_rustc` (measured: three plan items and the Loaf temporary directory), never under it. The two receipt-input keys the execution contract writes live with the model in `oven_model::oven_interop`, so `oven_rustc` strips them from a Loaf identity without depending on this crate; this crate re-exports them.
