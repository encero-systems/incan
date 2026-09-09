# `oven_rustc`

Ring: **oven**

Direct-rustc planning and execution, host/target unit graph, build-script and proc-macro host providers.

## Moves here from

- `src/oven/rustc.rs (10.5k lines)`
- `src/backend/project/plan.rs`, `lock_projection.rs`, `mod.rs`
- the Oven plan API #1266 moves out of `src/cli/commands/build.rs`

## May depend on

`oven_model`, `oven_store`

`generator.rs` does not come here: it renders a project from the checked program and provider facts and belongs to `compiler/incan_driver`. The generated-project stdlib baseline (`async, json, ordinal`) arrives as plan facts from the facet rather than as a generator constant.

This directory is a layout skeleton. It holds no code yet; `src/` is a placeholder for the conventional crate root.
