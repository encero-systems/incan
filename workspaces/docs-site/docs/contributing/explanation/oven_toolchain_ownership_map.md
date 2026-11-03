# Oven and compiler ownership

Oven is authored in Incan under `workspaces/oven`. It owns manifest intake, feature selection, local dependency preparation, unit descriptions, retained project owners, executable reuse, graph retrieval, MCP and toolchain archives.

The Rust frontend owns parsing, checking and Body IR. Incan lowering owns translation of checked bodies into native plans. The Rust compiler adapter owns the pinned rustc integration and Rust metadata extraction. The Incan runtime and standard packages own runtime behavior.

These boundaries keep each fact in one place: orchestration does not infer language semantics, lowering does not recheck types, and the native adapter does not become another authoring layer.

## Installed-toolchain ownership

The selected home contains the compiler closure and standard-package artifacts. A local Rust installation supplies the pinned sysroot. Toolchain installation verifies compiler identity and transported payload digests before publishing a replacement. Publication transfers admission only when the verified filesystem generations survive the directory rename; the first command reads the completed installation admission instead of hashing the shipped closure again.

Packed registry closures carry the native identities and exact dependency entries already established by their producer. Consumers read those facts rather than rediscovering them through rustc. Installation prepares the retained native metadata for legacy bundles that do not carry it.

The installation also admits one versioned typed bakery page containing the compiler description, normalized standard-package intent, manifest identities, producer records and native linking descriptions. Ordinary commands decode that selected page once per process instead of reading and parsing those original documents repeatedly. Filesystem generations guard the installation's membership and each admitted input, including in-place rewrites; changed inputs are re-admitted, while damage to the selected bakery revision refuses. An explicit standard-library source override remains editable input. These generation checks still perform metadata queries and do not establish the build engine's zero-stat capture target.

The installation page and admitted compiler identity share the home catalog. An explicit work-bakery override does not rebuild, copy or re-admit those installed facts. Both routes read the same selected installation revision, with its integrity and filesystem generation checks, independently of which store holds the project's compiled units.

A short command or resident preparation captures that checked page once under its snapshot lease. Its callbacks borrow the captured facts without repeating selection or rebuilding the document projection. A resident process ends the capture after successful or failed preparation, so the next request reconciles installation generations again; direct library readers continue to reconcile each selection.

Installation compiles and executes a built-in qualification program through the ordinary run command to verify the installed compiler, runtime and loader. Its temporary project is removed, while the shared development runtime and installed manifest facts remain available to the first user project.

Project-generated output belongs under the project's `target/incan/`; standard-package output belongs under the toolchain home. Reuse is an optimization over these explicit inputs, not another semantic route.

See [Architecture](architecture.md) and [Layer boundaries](layering.md).
