# Oven ring

The build system: project model, resolution, store, direct-rustc execution, registry access. Depends on **no Incan ring**. Everything Oven needs to know about Incan arrives through the provider interface that `compiler/incan_oven_facet` implements. See *The Oven boundary* in `LAYOUT.md` for the measured edges and their disposition.

**Versioning:** Own line. RFC 118 gives Oven its own command surface; its versions move independently of the compiler.

| Directory | Purpose |
| --- | --- |
| `oven_model/` | loaf.toml, oven.lock, workspace discovery, dependency resolution, lifecycle, toolchain layout. |
| `oven_store/` | Bounded Loaf store, receipts, identities, publication. |
| `oven_rustc/` | Direct-rustc planning and execution, host/target unit graph, build-script and proc-macro host providers. |
| `oven_registry/` | Registry index access, crate acquisition, checksum and signature verification, trust policy. New crate. |
| `oven_interop/` | Native linkage, carriers, interop bundles, `rust::` dependency closure sealing. |
| `oven_cargo_compat/` | Explicit Cargo-compatibility and adoption mode. Never a hidden backend. |

See [`LAYOUT.md`](../LAYOUT.md) for the ring rules and the migration order.
