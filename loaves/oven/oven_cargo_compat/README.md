# `oven_cargo_compat`

Ring: **oven**

Explicit Cargo-compatibility and adoption mode: the hidden `legacy_cargo` baker that is the one place Cargo runs, the compiler-suite publication it drives, and the Loaf bake that prepares a generated project's closure through it. Never a hidden backend — it sits over `oven_rustc`, calling direct rustc's planning and Loaf model, and nothing in `oven_rustc` reaches back.

## Sources

- `loaves/oven/oven_cargo_compat/src/lib.rs` and its modules (`cargo_json`, `cargo_process`, `compiler_suite_catalog`, `compiler_suite_targets`, `inspection_sources`, `lock`, `registry_sources`, `sdk_staging`, `workspace_authority`) — formerly `oven_rustc::legacy_cargo`, moved whole with its tests
- `loaves/oven/oven_cargo_compat/src/loaf_bake.rs` — the bake half of what was `oven_rustc::loaf`: the generated project's closure through the publisher, the sealed registry lock, the merged inspection sources, the generated-root externs
- `loaves/oven/oven_cargo_compat/src/loaf_bake/vocab_support.rs` — the bounded Cargo run and the vocab-support helpers it bakes and copies into the envelope

## Depends on

`oven_model`, `oven_store`, `oven_rustc`

The inversion that made this a crate: the Cargo-free wire contract the native route reads (the payload, suite, shard, foundation and toolchain-data types, their schema constants, the provider-compilation evidence key, the inspection-source handoff) is declared in `oven_rustc::native_contract` and re-exported here, so a publisher and the consumer that reads its output hold one type; `rustc_commit_hash` lives with the toolchain probes in `oven_rustc::rustc`; and `OvenLoafError::Publisher` carries the publisher's rendered failure, with the `From` impl here, so `oven_rustc`'s Loaf model names no Cargo. `loaves/compiler/incan_driver/src/backend/project/{cargo_toml,runner}.rs` stay in the driver: they render a project from the checked program and provider facts, which is compiler work, not Cargo's.

Retires with the runner's unified-resolution fallback once Oven unifies resolution (RFC 118/119). Kept as a crate so its removal is a directory delete.
