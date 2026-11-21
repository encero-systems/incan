# Compiler ring

The checked source pipeline, native lowering and runtime. Compiler units depend on the kernel; they do not depend on the toolchain ring. Oven builds these Loaves and supplies their compiled dependencies.

**Versioning:** Release-train cadence, with matching kernel contracts.

| Directory | Purpose |
| --- | --- |
| `incan_frontend/` | Typechecker, semantic analysis, vocabulary expansion, Body IR and checked API metadata. |
| `incan_lowering/` | The lowering from checked Body IR to the native plan, in Rust. |
| `incan_mir_plan/` | The Incan binding of the native plan for Incan callers such as Oven. |
| `incan_mir_plan_native/` | The native-plan types and plan records in Rust: keyed function products, layouts, the native interface, plan-function rows and lowering demands. |
| `incan_native_runtime/` | Rust runtime every native program links, built with the native support crates. |
| `incan_format/` | Source formatter. |
| `incan_semantics_stdlib/` | Standard-library semantics packs used by the compiler. |
| `incan_driver/` | Compilation sessions, module graphs, library publication and inspection. |
| `incan_worker_host/` | Incan binding over the Rust worker host: the worker protocols and snapshot descriptors under their reference module paths, for Oven. |
| `incan_worker_host_native/` | Rust host of one compiler worker process, run by the rustc adapter: the worker protocols, the worker lifecycle, and checking activations. |
| `incan_worker_host_bindings/` | Platform sockets, store and trace items the Rust worker host reaches by stable Rust names. |
| `internal_models/` | Incan-authored compiler record contracts: canonical checker records, scoped record reads and the producer interface. |

The rustc adapter lives in `loaves/toolchain/incan-rustc-driver/`. Layer boundaries are documented in `workspaces/docs-site/docs/contributing/explanation/layering.md`.
