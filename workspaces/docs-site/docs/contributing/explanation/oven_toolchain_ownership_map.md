# Oven and compiler ownership

Oven is authored in Incan under `workspaces/oven`. It owns manifest intake, feature selection, local dependency preparation, unit descriptions, retained project owners, executable reuse, graph retrieval, MCP and toolchain archives.

The Rust frontend owns parsing, checking and Body IR. Incan lowering owns translation of checked bodies into native plans. The Rust compiler adapter owns the pinned rustc integration and Rust metadata extraction. The Incan runtime and standard packages own runtime behavior.

These boundaries keep each fact in one place: orchestration does not infer language semantics, lowering does not recheck types, and the native adapter does not become another authoring layer.

## Installed-toolchain ownership

The selected home contains the compiler closure and standard-package artifacts. A local Rust installation supplies the pinned sysroot. Toolchain installation verifies compiler identity and transported payload digests before publishing a replacement. Publication transfers admission only when the verified filesystem generations survive the directory rename; the first command reads the completed installation admission instead of hashing the shipped closure again.

Packed registry closures carry the native identities and exact dependency entries already established by their producer. Consumers read those facts rather than rediscovering them through rustc. Installation prepares the retained native metadata for legacy bundles that do not carry it.

Project-generated output belongs under the project's `target/incan/`; standard-package output belongs under the toolchain home. Reuse is an optimization over these explicit inputs, not another semantic route.

See [Architecture](architecture.md) and [Layer boundaries](layering.md).
