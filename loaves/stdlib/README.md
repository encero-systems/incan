# Stdlib ring

The Incan standard library as SDK components, one directory per component, each a Loaf holding its Incan source and its Rust runtime bridge side by side.

**Versioning:** release-train cadence per component set; every facet and both derive crates carry the ring's version line, checked by `scripts/check_ring_versions.py`. `sdk-components.toml` keeps `compiler-requirement` as a range, and `incan_std_core::__incan_stdlib_version_check!` accepts any stdlib the compiler's line is compatible with under the semver rule (a prerelease demands equality). A check against the stable `incan-v1` symbol ABI is the destination.

| Component | Namespace roots | Rust facet | Runtime crates |
| --- | --- | --- | --- |
| `core/` | `std.features`, `std.prelude`, `std.registry`, `std.result`, `std.reflection`, `std.this`, `std.derives`, `std.traits`, `std.runtime` | `incan_std_core` (mandatory: reflection, frozen constants, numerics, strings, collections, storage, validation, the version check) | — |
| `interop/` | `std.interop` | — | — |
| `system/` | `std.environ`, `std.io`, `std.tempfile`, `std.fs` | — | byteorder, encoding_rs, rustix, tempfile |
| `codecs/` | `std.checksum`, `std.encoding` | — | crc32fast |
| `compression/` | `std.compression` | — | flate2, zstd, bzip2, xz2, snap |
| `data/` | `std.collections`, `std.graph`, `std.hash`, `std.json`, `std.toml`, `std.math`, `std.uuid`, `std.datetime`, `std.regex`, `std.serde` | `incan_std_data` (JSON traits and value, the `std.serde` facade, the ordinal-map key helpers) | serde, serde_json, toml, toml_edit, serde_path_to_error, xxhash-rust, libm, rand, regex, the eight hash crates |
| `async/` | `std.async` | `incan_std_async` (tasks, timers, races, channels, synchronization) | tokio (rt-multi-thread, macros, time, sync, net) |
| `observability/` | `std.logging`, `std.telemetry` | — | — |
| `web/` | `std.web` | `incan_std_web` (route registry, `App`, responses) | axum, inventory, incan_web_macros |
| `testing/` | `std.testing` | `incan_std_testing` (marker and assertion host functions) | — |
| `derive/` | — | host proc-macro crates `incan_derive` and `incan_web_macros` | — |

Each component directory holds its Incan sources under `src/` — the `.incn` modules for the namespace roots `sdk-components.toml` assigns it, resolved by the compiler through that catalog — and, where the component has one, its Rust facet under `rust/`: a crate named `incan_std_<component>` that the `.incn` sources reach with `from rust::incan_std_<component>::…` and that generated code links by path. A `loaf.toml` declares the facet with `[rust.source] root = "rust"`, as RFC 119 spells a mixed root. The facet sits next to the source it serves, so runtime-dependency attribution follows the component instead of the whole `std.<module>`: a program that never imports `std.collections` links no hasher.

## Why the derive macros are separate crates

Rust requires procedural macros to live in a crate with `proc-macro = true`, and such a crate can export only macros — no traits, no structs. So the traits generated code implements (`HasFieldInfo`, `ToJson`, `FromJson`, …) live in the facets, and the macros that implement them (`#[derive(FieldInfo)]`, `#[derive(IncanJson)]`, `#[route(...)]`) live in `derive/incan_derive` and `derive/incan_web_macros`. It is the `serde` + `serde_derive` pattern. Generated code names both sides by path; the compiler's project generator adds every facet the program links and the derive crates to the generated `Cargo.toml`, and the ring version gate keeps the facets and the macro crates on one version line.

To add a derive-backed feature: define the trait in the owning facet, implement the macro in `derive/incan_derive/src/lib.rs`, teach lowering to recognize the decorator, and add a codegen snapshot test.

The ring rules live in *Repository layout* in `workspaces/docs-site/docs/contributing/explanation/architecture.md`.
