Keep the pipeline aligned (to avoid language/tooling drift):

- **Syntax crate (`crates/incan_syntax/`)**: lexer → parser → AST → diagnostics
- **Formatter (`src/format/`)**: prints AST back (idempotent; never emits invalid syntax)
- **Semantic core (`crates/incan_core/`)**: canonical vocab / shared semantic helpers (avoid duplicating “meaning” in multiple layers)
- **Compiler (`src/frontend/`, `src/backend/`)**:
    - **typechecker** validates and annotates
    - **lowering** turns AST into IR
    - **emission** generates correct Rust
- **Runtime/stdlib (`crates/incan_stdlib/`, `stdlib/`)**: behavior that can live outside the compiler should live here

*Rule of thumb*: prefer pushing shared meaning “down” into `incan_core`/`incan_syntax`/`incan_stdlib`, and keep the `incan` (root) crate focused on orchestration and pipeline wiring.