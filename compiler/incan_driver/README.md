# `incan_driver`

Ring: **compiler**

The compile session: module graph, parsed modules, build orchestration, generated cache, replacement compatibility. No clap, no terminal I/O.

## Moves here from

- `src/cli/commands/build.rs (20.8k lines, the logic half)`
- `src/cli/commands/common.rs (CompilationSession, ParsedModule, SDK discovery)`
- `src/generated_cache.rs`
- `src/replacement_compatibility.rs`
- `src/compiler_stack.rs`

## May depend on

`kernel`, `incan_frontend`, `incan_ir`, `incan_emit`, `incan_provider`, `incan_inspect`, `oven (model + store, as a consumer)`

This is what `incan-lsp` and `incan` both link. Its existence is what makes the LSP compile without the CLI (audit finding 1).

This directory is a layout skeleton. It holds no code yet; `src/` is a placeholder for the conventional crate root.
