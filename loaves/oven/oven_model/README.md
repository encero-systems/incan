# `oven_model`

Ring: **oven**

loaf.toml, oven.lock, workspace discovery, dependency resolution, lifecycle, toolchain layout.

## Moves here from

- `src/manifest.rs`
- `src/workspace.rs`
- `src/lockfile.rs` (generic lock only; SDK-provider and library-manifest sections go to `incan_oven_facet`)
- `src/dependency_resolver.rs` (resolution only; the compiler-diagnostics adapter and stdlib registry lookup go to `incan_oven_facet`)
- `src/project_lifecycle/`
- `src/toolchain_layout.rs`

## May depend on

none

Sole owner of `toml` and `toml_edit`.

This directory is a layout skeleton. It holds no code yet; `src/` is a placeholder for the conventional crate root.
