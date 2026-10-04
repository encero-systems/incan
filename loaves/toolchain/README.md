# Toolchain ring

Command binaries and the pinned native-driver Loaf live here. They may depend on every ring; nothing depends on the toolchain ring. The native driver holds the narrow rustc-type adapter required by DD-0004. `workspaces/release` and `workspaces/ide` stay where they are because a TypeScript extension and shell packaging are workspaces, not Loaves.

**Versioning:** The bundle. `Incan 0.6` is a manifest pinning one version of every other ring, the way a Rust toolchain pins rustc, cargo, and std.

| Directory | Purpose |
| --- | --- |
| `incan-cli/` | The `incan` command: clap surface, terminal rendering, exit codes. |
| `incan-lsp/` | Language server over the driver. |
| `incan-rustc-driver/` | Pinned rustc adapter and executable source; its native build awaits an identity-bound seed compiler. |
| `oven-cli/` | The `oven` binary and the `oven`, `lock` and `tools` handlers as a library `incan` mounts under its own spellings; depends on the driver and the frontend until RFC 118 authors `oven` against the Oven API. |

The ring rules live in *Repository layout* in `workspaces/docs-site/docs/contributing/explanation/architecture.md`; the migration is #1478's history.
