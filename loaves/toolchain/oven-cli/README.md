# `oven-cli`

Ring: **toolchain**

The `oven` binary and the library both binaries mount: the explicit Oven receipt, bounded-store and native direct-rustc workflow (`oven bake`, `import`, `interop`, `plan`, `store`, `test`, `run`, and the repository's own compiler-suite tooling), lock generation (`lock`), and the toolchain inspection helpers (`tools`). `incan` mounts every one of them under `incan oven …`, `incan lock` and `incan tools`, so scripts, CI and tests keep their spellings; the `oven` binary exposes the Oven-owned families at its root and leaves `tools` — a semantic product, `incan`'s by RFC 118's ownership table — to `incan`.

## Sources

- `loaves/toolchain/oven-cli/src/cli.rs` — the clap types of the three families, shared by both binaries
- `loaves/toolchain/oven-cli/src/commands/oven.rs`, `loaves/toolchain/oven-cli/src/commands/oven/` — the Oven workflow handlers
- `loaves/toolchain/oven-cli/src/commands/lock.rs` — `lock`
- `loaves/toolchain/oven-cli/src/commands/tools.rs` — `tools doctor`, `tools metadata`, `inspect registry`
- `loaves/toolchain/oven-cli/src/main.rs` — the `oven` binary

## Depends on

`oven` (`oven_model`, `oven_store`, `oven_rustc`), `compiler/incan_oven_facet`, and — the RFC 118 backlog — `incan_driver` (the bake and interop of an Incan project, the store defaults, lock resolution) and `incan_frontend` (`tools`). The layout's dependency line for this package is `oven` plus the facet; the two compiler-ring edges are the handlers' existing reaches, moved as they were, and RFC 118 authors the canonical `oven` against the Oven API in v0.7 so they can go. `cli_layering_guardrails` records the frontend reaches and lets them only shrink.

## Distribution

Built by `make build` and CI; not shipped in the release archives until RFC 118's command-surface split lands.
