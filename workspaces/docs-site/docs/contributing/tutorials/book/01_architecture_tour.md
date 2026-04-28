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

## Where to look in the repository

You can orient yourself with these anchors:

- `crates/incan_syntax/`:
    - shared lexer/parser/AST/diagnostics
    - used by compiler, formatter, and LSP to avoid drift
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

## Contributor workflow: “touch points”

Most changes you’ll make land in one of these patterns:

- **Builtin or special lowering**: change typechecking + lowering/emission
- **New syntax**: change lexer/parser/AST + formatter + typechecker + lowering/emission
- **Tooling**: reuse the same syntax/frontend layers (formatter and LSP should not drift from the compiler)

## Next

Next chapter: [02. Layering and boundaries](02_layering_and_boundaries.md).
