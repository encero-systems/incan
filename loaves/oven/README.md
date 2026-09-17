# Oven ring

The build system: project model, resolution, store, direct-rustc execution, interop, registry access. Depends on **no Incan ring**: everything Oven needs to know about Incan arrives through the provider interface that `compiler/incan_oven_facet` implements. That is the rule by dependency; by content the ring still holds Incan facts that moved with the code (the `incan_` runtime-crate prefix in `oven_rustc`, the `incan_std_core` root-extern rule, the toolchain layout tables, embedded `.incn` fixtures), and RFC 118 is where they become provider-supplied. The measured edges and their disposition are recorded on #1480.

**Versioning:** Own line. RFC 118 gives Oven its own command surface; its versions move independently of the compiler.

| Directory | Purpose |
| --- | --- |
| `oven_model/` | loaf.toml, oven.lock, workspace discovery, dependency resolution, lifecycle, toolchain layout. |
| `oven_store/` | Bounded Loaf store, receipts, identities, publication. |
| `oven_rustc/` | Direct-rustc planning and execution, host/target unit graph, build-script and proc-macro host providers. |
| `oven_registry/` | Registry index access, source-Loaf and asset acquisition, checksum and signature verification, trust policy. The contract crate for RFC 125 — a workspace member, deliberately empty while the RFC is a draft. |
| `oven_interop/` | Receipt-bound native interop: shim compilation through direct rustc, carriers, interop bundles, the execution receipt. Over `oven_rustc`; the declarations and lock projection stay in `oven_model`. |
| `oven_cargo_compat/` | Explicit Cargo-compatibility and adoption mode. Never a hidden backend. Layout skeleton. |

The ring rules live in *Repository layout* in `workspaces/docs-site/docs/contributing/explanation/architecture.md`; the migration is #1478's history.
