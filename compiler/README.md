# Compiler ring

The pipeline from checked AST to emitted Rust, plus the session and provider machinery both binaries share. Depends on kernel only. Never on oven, toolchain, or stdlib runtime crates.

**Versioning:** Release-train cadence. Requires a kernel version range.

| Directory | Purpose |
| --- | --- |
| `incan_frontend/` | Typechecker, semantic analysis, vocab desugar pass, body IR, API metadata. |
| `incan_ir/` | AST-to-IR lowering and the IR type definitions. |
| `incan_emit/` | IR-to-Rust emission with syn/quote, conversions, prettyplease formatting, replacement lowering. |
| `incan_format/` | Source formatter. |
| `incan_provider/` | Provider and SDK contracts (manifest types, component catalog, inventory) and their loaders. |
| `incan_inspect/` | Rust signature inspection for `rust::` imports and codegraph export. |
| `incan_driver/` | The compile session: module graph, parsed modules, build orchestration, generated cache, replacement compatibility. No clap, no terminal I/O. |

See [`LAYOUT.md`](../LAYOUT.md) for the ring rules and the migration order.
