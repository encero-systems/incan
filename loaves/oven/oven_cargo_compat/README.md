# `oven_cargo_compat`

Ring: **oven**

Explicit Cargo-compatibility and adoption mode. Never a hidden backend.

## Sources and remaining moves

- `loaves/oven/oven_rustc/src/legacy_cargo.rs`; its SDK inventory discovery and provider digests come through the facet's provider hook
- `loaves/compiler/incan_driver/src/backend/project/cargo_toml.rs`, pending inversion of its dependencies on driver-owned project generation
- `loaves/compiler/incan_driver/src/backend/project/runner.rs`, with its cfg-gated `rust_inspect` calls inverted into a facet hook so rust-analyzer never enters this ring

## May depend on

`oven_model`

Retires with the runner's unified-resolution fallback once Oven unifies resolution. Kept as a crate so its removal is a directory delete.

This directory is a layout skeleton. It holds no code yet; `src/` is a placeholder for the conventional crate root.
