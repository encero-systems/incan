# `incan_driver`

Ring: **compiler**

The compile session: module graph, parsed modules, build orchestration, generated cache, replacement compatibility. No clap, no terminal I/O.

## Moves here from

- `src/cli/commands/build.rs (20.8k lines, the logic half)`
- `src/cli/commands/common.rs (CompilationSession, ParsedModule, SDK discovery)`
- `src/generated_cache.rs`
- `src/replacement_compatibility.rs`
- `src/compiler_stack.rs`
- `src/backend/project/generator.rs` (renders the generated Rust project from the checked program and provider facts)

## May depend on

`kernel`, `incan_frontend`, `incan_ir`, `incan_emit`, `incan_provider`, `incan_inspect`, `oven (model + store, as a consumer)`

This is what `incan-lsp` and `incan` both link. Its existence is what makes the LSP compile without the CLI (audit finding 1).

`src/lib.rs` is the crate root, with the former `src/driver/` modules directly under `src/`; `src/backend/` (the generated project, the shadow comparison and the `backend::ir` shim over `incan_ir`/`incan_emit`), `src/inspect/`, `src/generated_cache.rs`, `src/replacement_compatibility.rs` and `src/shadow_support.rs` sit beside them. `src/backend/project/{plan,lock_projection,cargo_toml,runner}.rs` are still here: their moves into the Oven ring need inversions of their own.
