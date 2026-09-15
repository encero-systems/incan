# `oven_model`

Ring: **oven**

loaf.toml, oven.lock, workspace discovery, dependency resolution, lifecycle, toolchain layout.

## Sources and remaining moves

- `loaves/oven/oven_model/src/manifest.rs`
- `loaves/oven/oven_model/src/workspace.rs`
- `loaves/oven/oven_model/src/lock.rs` (generic lock model; compiler semantics are supplied by `incan_provider::lock_semantics`)
- `loaves/compiler/incan_provider/src/dependency_resolver.rs` remains in the compiler ring because it consumes Incan imports, diagnostics and provider metadata
- `loaves/oven/oven_model/src/project_lifecycle/`
- `loaves/oven/oven_model/src/toolchain_layout.rs`

## May depend on

none

Sole owner of `toml` and `toml_edit`.

The manifest, workspace, lifecycle and toolchain-layout modules and the interop declarations live here; `lock.rs` is the `oven.lock` model — parse, write, fingerprints, the publication lock; the compiler's `provider::lock_semantics` fills its semantic state.
