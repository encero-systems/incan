Keep the pipeline aligned (to avoid language/tooling drift):

- **Syntax crate (`loaves/kernel/incan_syntax/`)**: lexer → parser → AST → diagnostics
- **Formatter (`loaves/compiler/incan_format/`)**: prints AST back (idempotent; never emits invalid syntax)
- **Semantic core (`loaves/kernel/incan_lang/`)**: canonical vocab / shared semantic helpers (avoid duplicating “meaning” in multiple layers)
- **Compiler (`loaves/compiler/`)**:
    - **typechecker** (`incan_frontend/`) validates and annotates
    - **Body IR** records resolved semantic facts
    - **Incan lowering** (`incan_mir_lowering/`) consumes those facts and constructs a native plan
    - **Rust adapter** (`incan-rustc-driver/`) translates the plan to the pinned Rust compiler's MIR
- **Runtime/stdlib (`loaves/stdlib/<component>/{src,rust}/`)**: behavior that can live outside the compiler should live here

Keep shared language policy in kernel packages. Keep the driver (`incan_driver/`) and Oven (`workspaces/oven/`) focused on their unit and orchestration boundaries.
