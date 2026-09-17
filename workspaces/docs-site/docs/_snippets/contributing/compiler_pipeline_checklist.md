Keep the pipeline aligned (to avoid language/tooling drift):

- **Syntax crate (`loaves/kernel/incan_syntax/`)**: lexer → parser → AST → diagnostics
- **Formatter (`loaves/compiler/incan_format/`)**: prints AST back (idempotent; never emits invalid syntax)
- **Semantic core (`loaves/kernel/incan_lang/`)**: canonical vocab / shared semantic helpers (avoid duplicating “meaning” in multiple layers)
- **Compiler (`loaves/compiler/`)**:
    - **typechecker** (`incan_frontend/`) validates and annotates
    - **lowering** (`incan_ir/`) turns AST into IR
    - **emission** (`incan_emit/`) generates correct Rust
- **Runtime/stdlib (`loaves/stdlib/<component>/{src,rust}/`)**: behavior that can live outside the compiler should live here

*Rule of thumb*: prefer pushing shared meaning “down” into `incan_lang`/`incan_syntax`/the stdlib facets, and keep the driver (`incan_driver/`) and the `incan` command line (`loaves/toolchain/incan-cli/`) focused on orchestration and pipeline wiring.
