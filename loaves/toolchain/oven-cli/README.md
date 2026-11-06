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

## Adopted closure compilation

`incan oven compile-closure --pin INDEX --index-commit COMMIT --roots GRAPH --profile debug|release --out RESULT --lock RESOLUTION` compiles an `incan.oven.loaf-resolution/2` document through the same executor as the SDK seed. Omit `--lock` to resolve every root together with the Incan stage-1 driver against committed index lines at `--index-commit`; set `INCAN_SOURCE_ROOT` to the compiler source checkout containing that driver (the current directory is the default). Each schema 2 unit records `edges`: entries with `dependency_key`, `loaf`, exact `version`, and host/target `domain`. Compilation follows those bindings without repeating requirement selection. Schema 1 locks remain accepted when each active dependency uniquely matches a locked unit; ambiguous legacy edges name the dependency and require re-resolution. The result retains the resolved lock and names unavailable roots in `refused`. `GRAPH` contains `roots` in the index dependency spelling, plus `target` and `host`. This implementation requires both triples to equal the selected rustc host. Every index fact and generated-file read uses the full supplied commit, never the index worktree. Supply digest-addressed archives with `INCAN_OVEN_BLOBS` or `--blobs`; `--rustc` selects an explicit compiler instead of the active compiler.

`RESULT` has schema `incan.oven.closure/1`, the original `lock`, an `externs` object keyed by normalized root aliases (each carrying `path` and compiled `unit` identity), a `units` list with every compiled unit (`loaf`, `version`, `domain`, `features`, `profile`, receipt `unit` identity and store `entry` identity), and `refused` records carrying `loaf`, `version`, `profile`, `features`, and `reason`. Independent branches continue after a refusal. An invocation with refusals still writes the result and succeeds; consumers must inspect `refused`. Repeated requests reuse verified store outputs below the result's parent directory.

The `--lock` alternative is currently required. A guarded bake of the resolver project refuses because its Rust dependency `blake2` requires a sealed Oven inspection source authority. The Incan engine source dispatches `incan.oven.loaf-closure-request/1`, but this checkout has no baked engine available for the host to invoke. The command does not resolve in Rust or fall back to Cargo.

Native-link records use the existing publisher executor: each declared object is compiled once, the exact object set is verified, and Oven writes the indexed archive itself. `INCAN_OVEN_LINK_OWNERS` supplies executable-owner roots; their declared closures must reproduce the fact's owner identity. Archives and compiler inputs are store-bound, and consuming rustc receives native search paths and static library flags. Records whose source catalogs require reading inert Cargo manifests or locks are refused by name; the current `lzma-sys` `include--` catalog contains `Cargo.toml` and therefore cannot execute under this boundary.

## Distribution

Built by `make build` and CI; not shipped in the release archives until RFC 118's command-surface split lands.
