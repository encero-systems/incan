# Toolchain ring

The language server and pinned rustc adapter live here. They may depend on compiler and kernel Loaves; those rings do not depend on toolchain units. The command line is Oven, authored in Incan under `workspaces/oven/`.

**Versioning:** The toolchain bundle pins matching compiler, runtime and standard-package artifacts.

| Directory | Purpose |
| --- | --- |
| `incan-lsp/` | Language server over checked driver and library metadata. |
| `incan-rustc-driver/` | Pinned compiler adapter and executable; checks units and translates the native plan into rustc MIR. Its check, diagnostic and formatting handlers serve Oven's commands. |

Release packaging and the editor extension remain under `workspaces/release/` and `workspaces/ide/`.

Layer boundaries are documented in `workspaces/docs-site/docs/contributing/explanation/layering.md`.
