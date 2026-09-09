# `incan_provider`

Ring: **compiler**

Provider and SDK contracts (manifest types, component catalog, inventory) and their loaders.

## Moves here from

- `src/provider/`
- `src/library_manifest/`
- `src/compiled_sdk.rs`
- `src/semantics_registry.rs`
- `crates/incan_semantics_stdlib/`

## May depend on

`kernel`

Split internally into `contract` (types the frontend may import) and `load` (filesystem and store access). This is the cut that removes the frontend <-> library_manifest cycle.

This directory is a layout skeleton. It holds no code yet; `src/` is a placeholder for the conventional crate root.
