# Toolchain ring

Thin binaries only. May depend on every ring. Nothing depends on it. This is the one ring that is not Loaf-shaped in the sense of holding source to bake; `workspaces/release` and `workspaces/ide` stay where they are because a TypeScript extension and shell packaging are workspaces, not Loaves.

**Versioning:** The bundle. `Incan 0.6` is a manifest pinning one version of every other ring, the way a Rust toolchain pins rustc, cargo, and std.

| Directory | Purpose |
| --- | --- |
| `incan-cli/` | The `incan` command: clap surface, terminal rendering, exit codes. |
| `incan-lsp/` | Language server over the driver. |
| `oven-cli/` | The `oven` command surface from RFC 118. |

See [`LAYOUT.md`](../LAYOUT.md) for the ring rules and the migration order.
