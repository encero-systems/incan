# 1. Architecture tour

Follow a source program from its project description to native output.

## Read the pipeline

```mermaid
--8<-- "_snippets/diagrams/compiler_pipeline.mmd"
```

Oven reads the project and describes a unit. The frontend checks it and records Body IR. Incan lowering constructs a native plan, and the Rust adapter passes it to the pinned Rust compiler.

## Find each owner

Start in `workspaces/oven/src/main.incn` for command dispatch. Read `unit.incn` for the unit description. Then inspect `loaves/compiler/incan_frontend`, `loaves/compiler/incan_driver`, `loaves/compiler/incan_mir_lowering` and `loaves/toolchain/incan-rustc-driver`.

Use `incan inspect codegraph PATH --query defs NAME` to locate declarations and `--query callers NAME` for checked call relationships. An unchecked-unit warning means an empty relationship query does not prove no callers exist.

## Follow a runtime boundary

Standard packages under `loaves/stdlib` are ordinary Loaves. Their public contracts and compiled implementations are consumed together. Native runtime behavior belongs in `incan_native_runtime`, rather than in build orchestration.

Read [Architecture](../../explanation/architecture.md) and [Layer boundaries](../../explanation/layering.md) before editing a stage. The next chapter explains how those boundaries constrain a change.
