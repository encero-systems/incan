# `oven_cargo_compat`

Ring: **oven**

Explicit Cargo-compatibility and adoption mode. Never a hidden backend.

## Moves here from

- `src/oven/legacy_cargo.rs (13k lines)`
- `src/backend/project/cargo_toml.rs`
- `src/backend/project/runner.rs`

## May depend on

`oven_model`

Retires with the runner's unified-resolution fallback once Oven unifies resolution. Kept as a crate so its removal is a directory delete.

This directory is a layout skeleton. It holds no code yet; `src/` is a placeholder for the conventional crate root.
