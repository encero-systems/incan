# Compiler Source (`src/`)

This is the main Incan compiler crate. The compilation pipeline flows:

```text
Source → Lexer → Parser/AST → Typechecker → Lowering (AST→IR) → Emission (IR→Rust)
```

## Directory map

```text
src/
├── main.rs                           # CLI entrypoint
├── lib.rs                            # Library root, re-exports
├── manifest.rs                       # incan.toml project manifest parsing
├── lockfile.rs                       # incan.lock lockfile handling
├── library_manifest.rs               # Library manifest (lib.incan.toml)
├── dependency_resolver.rs            # Dependency resolution for multi-crate builds
├── semantics_registry.rs             # Shared semantic definitions
├── numeric.rs / numeric_adapters.rs  # Numeric type handling
├── version.rs                        # Compiler version constant
│
├── frontend/                         # Lexer, parser, AST, typechecker, symbols
│   ├── typechecker/                  # Type checking and semantic analysis
│   │   ├── check_decl.rs             #   Declaration checking (models, classes, traits, functions)
│   │   ├── check_expr/               #   Expression checking (access, calls, match, etc.)
│   │   ├── check_stmt.rs             #   Statement checking
│   │   ├── collect/                  #   Symbol collection pass (imports, declarations)
│   │   ├── helpers/                  #   Shared typechecker utilities
│   │   ├── const_eval.rs             #   Compile-time constant evaluation
│   │   └── stdlib_loader.rs          #   Stdlib type loading
│   ├── symbols.rs                    # Symbol table
│   ├── resolver.rs                   # Name resolution
│   ├── module.rs                     # Module representation
│   ├── decorator_resolution.rs       # Decorator processing
│   ├── library_exports.rs            # Public export tracking
│   └── surface_semantics.rs          # Surface-level semantic rules
│
├── backend/
│   ├── ir/                           # IR types, lowering, and emission
│   │   ├── codegen.rs                # ** Main entry point ** (IrCodegen) — start here
│   │   ├── lower/                    # AST → IR lowering
│   │   │   ├── decl/                 #   Declaration lowering
│   │   │   ├── expr/                 #   Expression lowering
│   │   │   ├── stmt.rs               #   Statement lowering
│   │   │   └── types.rs              #   Type lowering
│   │   ├── emit/                     # IR → Rust code emission (syn/quote)
│   │   │   ├── program.rs            #   Top-level program emission
│   │   │   ├── decls/                #   Declaration emission
│   │   │   ├── expressions/          #   Expression emission (builtins, calls, etc.)
│   │   │   ├── statements.rs         #   Statement emission
│   │   │   └── types.rs              #   Type emission
│   │   ├── emit_service/             # Emission helpers (builtins, decl, expr, stmt)
│   │   ├── scanners/                 # Feature scanners (serde, decorators, web, etc.)
│   │   ├── conversions.rs            # Centralized type conversions (&str↔String, borrows)
│   │   ├── trait_bound_inference.rs  # Generic trait bound inference
│   │   └── types.rs, expr.rs, stmt.rs, decl.rs  # IR type definitions
│   └── project/                      # Cargo project generation
│       ├── generator.rs              #   Generates Cargo.toml + src/*.rs
│       ├── plan.rs                   #   Build plan computation
│       ├── runner.rs                 #   Shells out to cargo build/run
│       └── cargo_toml.rs             #   Cargo.toml construction
│
├── cli/
│   ├── commands/                     # CLI subcommands
│   │   ├── build.rs                  #   `incan build`
│   │   ├── debug.rs                  #   `incan debug`
│   │   ├── format.rs                 #   `incan fmt`
│   │   ├── init.rs                   #   `incan init`
│   │   ├── lock.rs                   #   `incan lock`
│   │   ├── common.rs                 #   Shared CLI helpers (module collection, error display)
│   │   └── stdlib_loader.rs          #   Stdlib loading for CLI
│   └── test_runner/                  # pytest-style test runner (`incan test`)
│       ├── discovery.rs              #   Test function discovery
│       ├── execution.rs              #   Test execution and harness generation
│       ├── reporter.rs               #   Result formatting
│       └── module_graph.rs           #   Multi-module test graph
│
├── format/                           # Code formatter (`incan fmt`)
│   ├── formatter/                    #   Formatting logic per AST node
│   ├── config.rs                     #   FormatConfig (line length, trailing commas, etc.)
│   └── writer.rs                     #   Output writer
│
├── lsp/                              # Language server (feature-gated behind `--features lsp`)
│   ├── backend.rs                    #   LSP request handlers
│   └── diagnostics.rs                #   Diagnostic conversion
│
└── bin/
    ├── lsp.rs                        # LSP binary entrypoint
    └── generate_vscode_grammar_keywords.rs  # VS Code grammar helper
```

## Key entry points

- **Codegen**: `backend::ir::codegen::IrCodegen` — the single public entry point for all code generation.
- **Type conversions**: `backend::ir::conversions` — centralized `&str`/`String`, borrow, and ownership conversions. Use `determine_conversion()`, never ad-hoc `.to_string()` insertions.
- **CLI commands**: all go through `cli::commands::common::collect_modules()` for module loading and error display.
- **LSP**: feature-gated — build with `cargo build --features lsp` or `make lsp`.
