# 1. Architecture tour

This chapter builds a mental model for how Incan turns `.incn` into an executable, and where that logic lives in the repo.

## The pipeline (conceptual)

```mermaid
--8<-- "_snippets/diagrams/compiler_pipeline.mmd"
```

Keep this picture in your head while reading the code. Most contributing work is “add a feature” **and** ensure each stage
continues to compose.

## The big pieces

At a high level, the compiler/toolchain splits into:

- **Syntax frontend**: lexer + parser + AST + syntax diagnostics
- **Compiler frontend**: module resolution + typechecking
- **Backend**: lowering + Rust emission
- **Project generation**: writes a Cargo project, builds/runs it
- **Tooling**: formatter, LSP, test runner, and other developer-facing workflows

Workspace crates split across stable contracts, compiler/toolchain implementation, and runtime-only implementation. Keep that split in mind before adding a dependency: generated programs may use runtime crates, but compiler code should normally consume stable contract crates and toolchain crates only.

## Where to look in the repository

You can orient yourself with these anchors:

- `crates/incan_syntax/`:
    - shared lexer/parser/AST/diagnostics
    - used by compiler, formatter, and LSP to avoid drift
- `crates/incan_core/`:
    - pure language policy and registries shared across compiler/runtime boundaries
- `crates/incan_semantics_core/` and `crates/incan_semantics_stdlib/`:
    - descriptor contracts plus current stdlib semantics-pack implementation
- `crates/incan_vocab/`:
    - stable library manifest/desugarer contract for import-activated library DSLs
- `crates/rust_inspect/`:
    - staged Rust metadata preparation/cache subsystem for Rust interop
- `crates/incan_stdlib/`, `crates/incan_derive/`, and `crates/incan_web_macros/`:
    - runtime-only support used by generated Rust programs
- `src/frontend/`:
    - module resolution (`module.rs`)
    - typechecker (`typechecker/`)
    - symbol table + scope rules (`symbols.rs`)
- `src/backend/`:
    - IR + lowering (`ir/lower/`)
    - emission (`ir/emit/`) producing Rust code
    - project generation (`project.rs`)
- `src/cli/`:
    - CLI entrypoints and commands (`build`, `run`, `fmt`, `test`)
- `src/lsp/`:
    - language server implementation that reuses frontend stages

If you want the deep version (module layout, key types, entry points), read:

- [Architecture](../../explanation/architecture.md)
- [Layering rules](../../explanation/layering.md)

## Contributor workflow: “touch points”

Most changes you’ll make land in one of these patterns:

- **Builtin or special lowering**: change typechecking + lowering/emission
- **New syntax**: change lexer/parser/AST + formatter + typechecker + lowering/emission
- **Tooling**: reuse the same syntax/frontend layers (formatter and LSP should not drift from the compiler)

## Next

Next chapter: [02. Layering and boundaries](02_layering_and_boundaries.md).
