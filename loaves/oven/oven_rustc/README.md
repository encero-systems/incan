# `oven_rustc`

Ring: **oven**

Direct-rustc planning and execution, host/target unit graph, build-script and proc-macro host providers.

## Moves here from

- `src/oven/rustc.rs (10.5k lines)`
- `src/backend/project/plan.rs`
- `src/backend/project/generator.rs (project rendering)`

## May depend on

`oven_model`, `oven_store`

The generated-project stdlib baseline (`async, json, ordinal`) becomes plan facts here rather than a generator constant.

This directory is a layout skeleton. It holds no code yet; `src/` is a placeholder for the conventional crate root.
