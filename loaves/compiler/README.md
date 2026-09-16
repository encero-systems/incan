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
| `rust_inspect/` (`incan_inspect/` after the step-5 rename) | Rust signature inspection for `rust::` imports and codegraph export. |
| `incan_oven_facet/` | Implements Oven's provider interface for Incan: stdlib extra crate sources, SDK-provider and library-manifest lock sections, diagnostics mapping, rust_inspect hooks. The one place Oven learns about Incan. |
| `incan_driver/` | The compile session: module graph, parsed modules, build orchestration, generated cache, replacement compatibility. No clap, no terminal I/O. |

See [`LAYOUT.md`](../LAYOUT.md) for the ring rules and the migration order.

`incan_semantics_stdlib/` sits here too: the stdlib semantics packs are compiler implementation, not a kernel contract.

`incan_test_support/` is the integration-test harness the rings' roots share — checkout anchors, the compiler subprocess wired to the harness-selected generated target and provider store, fixture builders and artifact readers. It is a dev-dependency only (`publish = false`) and links ring crates alone, so a root can live in the package it exercises; the parity corpus's own two helpers live beside the driver's roots.
