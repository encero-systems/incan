# `incan_ir`

Ring: **compiler**

AST-to-IR lowering and the IR type definitions.

## Moves here from

- `src/backend/ir/lower/`
- `src/backend/ir/{types,expr,stmt,decl}.rs`

## May depend on

`kernel`, `incan_frontend`

The IR type modules, `lower/`, the IR-side numeric adapters and the analyses that read only the IR (`borrow_inference`, the scanners) live here; the IR→manifest type projection came from codegen.
