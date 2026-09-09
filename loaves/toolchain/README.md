# Toolchain ring

Thin binaries and distribution. May depend on every ring. Nothing depends on it.

**Versioning:** The bundle. `Incan 0.6` is a manifest pinning one version of every other ring, the way a Rust toolchain pins rustc, cargo, and std.

| Directory | Purpose |
| --- | --- |
| `incan-cli/` | The `incan` command: clap surface, terminal rendering, exit codes. |
| `incan-lsp/` | Language server over the driver. |
| `oven-cli/` | The `oven` command surface from RFC 118. |
| `release/` | Archive packaging, release manifest, support workspace, install scripts. |
| `ide/` | Editor integrations. |

See [`LAYOUT.md`](../LAYOUT.md) for the ring rules and the migration order.
