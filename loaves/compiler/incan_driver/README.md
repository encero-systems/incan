# `incan_driver`

Ring: **compiler**

Owns compilation sessions, module graphs, checking and Loaf publication. Oven owns command parsing and build orchestration.

## Current sources

- `loaves/compiler/incan_driver/src/unit.rs` — unit descriptions consumed from Oven
- `loaves/compiler/incan_driver/src/session.rs` — compilation sessions
- `loaves/compiler/incan_frontend/src/parsed_module.rs` — parsed-module data consumed by the driver
- `loaves/compiler/incan_driver/src/loaf_publication.rs` — checked Loaf publication
- `loaves/compiler/incan_frontend/src/compiler_stack.rs` — shared frontend stack configuration
- `loaves/compiler/incan_driver/src/check.rs` — unit checking

## May depend on

The dependency contract is declared in `loaves/compiler/incan_driver/loaf.toml`.

The language server and native compiler share these compilation services.

`loaves/compiler/incan_driver/src/lib.rs` is the crate root. Inspection lives under `loaves/compiler/incan_driver/src/inspect/`; Oven's build orchestration lives under `workspaces/oven/src/`. The former generated-project backend and generated-source cache were removed.
