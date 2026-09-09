# Stdlib ring

The Incan standard library as SDK components, one directory per component, each a Loaf holding its Incan source and its Rust runtime bridge side by side.

**Versioning:** release-train cadence per component set. `sdk-components.toml` keeps `compiler-requirement` as a range; the exact-version `incan_stdlib_version_check!` is replaced by a check against the stable `incan-v1` symbol ABI.

| Component | Namespace roots | Runtime crates |
| --- | --- | --- |
| `core/` | `std.features`, `std.prelude`, `std.registry`, `std.result`, `std.reflection`, `std.this`, `std.derives`, `std.traits`, `std.runtime` | mandatory; no runtime crates |
| `interop/` | `std.interop` | no runtime crates |
| `system/` | `std.environ`, `std.io`, `std.tempfile`, `std.fs` | byteorder, encoding_rs, rustix, tempfile |
| `codecs/` | `std.checksum`, `std.encoding` | crc32fast |
| `compression/` | `std.compression` | flate2, zstd, bzip2, xz2, snap |
| `data/` | `std.collections`, `std.graph`, `std.hash`, `std.json`, `std.math`, `std.uuid`, `std.datetime`, `std.regex`, `std.serde` | serde, serde_json, libm, rand, regex, the eight hash crates |
| `async/` | `std.async` | tokio (rt-multi-thread, macros, time, sync, net) |
| `observability/` | `std.logging`, `std.telemetry` | no runtime crates |
| `web/` | `std.web` | axum, inventory, incan_web_macros |
| `testing/` | `std.testing` | no runtime crates |
| `derive/` | — | host proc-macro crates `incan_derive` and `incan_web_macros` |

Today: `crates/incan_stdlib/stdlib/components/` holds the Incan source per component, and `crates/incan_stdlib/src/` holds all runtime bridges in one feature-gated crate. Here the bridge moves next to the source it serves, so runtime-dependency attribution can follow the component (or the symbol) instead of the whole `std.<module>`.

See [`LAYOUT.md`](../LAYOUT.md).
