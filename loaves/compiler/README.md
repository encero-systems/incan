# Compiler ring

The checked source pipeline, native lowering and runtime. Compiler units depend on the kernel; they do not depend on the toolchain ring. Oven builds these Loaves and supplies their compiled dependencies.

**Versioning:** Release-train cadence, with matching kernel contracts.

| Directory | Purpose |
| --- | --- |
| `incan_frontend/` | Typechecker, semantic analysis, vocabulary expansion, Body IR and checked API metadata. |
| `incan_mir_lowering/` | Incan-authored lowering from checked Body IR to the native plan. |
| `incan_mir_plan/` | Native-plan data and builders shared with lowering and the rustc adapter. |
| `incan_native_runtime/` | Incan-authored runtime used by native programs. |
| `incan_format/` | Source formatter. |
| `incan_semantics_stdlib/` | Standard-library semantics packs used by the compiler. |
| `incan_driver/` | Compilation sessions, module graphs, library publication and inspection. |

The rustc adapter lives in `loaves/toolchain/incan-rustc-driver/`. Layer boundaries are documented in `workspaces/docs-site/docs/contributing/explanation/layering.md`.
