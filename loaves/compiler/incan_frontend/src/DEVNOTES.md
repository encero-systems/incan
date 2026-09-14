# Incan Compiler Frontend - Developer Notes

## Overview

The Incan compiler frontend consists of several components that transform source code into a typed AST ready for code generation:

```text
┌─────────────────────────────────────────────────────────────────┐
│                  COMPILER FRONTEND PIPELINE                     │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│   ┌────────┐    ┌───────┐    ┌────────┐    ┌─────────────┐      │
│   │ Source │───▶│ Lexer │───▶│ Parser │───▶│ TypeChecker │      │
│   │  .incn │    │       │    │        │    │             │      │
│   └────────┘    └───┬───┘    └───┬────┘    └──────┬──────┘      │
│                     │            │                │             │
│                     ▼            ▼                ▼             │
│                   Tokens        AST           Typed AST         │
│                                                   │             │
│   ┌───────────────────────────────────────────────┼─────────┐   │
│   │              Shared Infrastructure            │         │   │
│   │  ┌─────────────┐              ┌───────────────▼──────┐  │   │
│   │  │ SymbolTable │◀────────────▶│     Diagnostics      │  │   │
│   │  │  (scopes)   │              │  (errors/warnings)   │  │   │
│   │  └─────────────┘              └──────────────────────┘  │   │
│   └─────────────────────────────────────────────────────────┘   │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

## Module Structure

- Syntax is provided by the shared `incan_syntax` crate:
  - `crates/incan_syntax/src/lexer/*` - Tokenization module
  - `crates/incan_syntax/src/parser.rs` - Recursive descent parser producing an AST
  - `crates/incan_syntax/src/ast.rs` - Abstract Syntax Tree node definitions
  - `crates/incan_syntax/src/diagnostics.rs` - Syntax/parse error reporting
- `symbols.rs` - Symbol table and scope management
- `typechecker/` - Type checking and validation (two-pass, split by responsibility)
- `diagnostics.rs` - Compiler diagnostics (type errors, warnings) with Python-friendly messages

## Key Design Decisions

### Indentation-Based Syntax

Like Python, Incan uses indentation for blocks. The lexer tracks indent levels and emits `INDENT` and `DEDENT` tokens. Incan uses 2-space or 4-space indentation by default (tabs are treated as 4 spaces).

### Rust-Style Imports

Imports use `::` as path separator:

```incan
import polars::prelude as pl
import incan::http as http
import python "requests" as pyreq   # Python interop
```

### Pattern Matching

Both Python-style `case` and Rust-style `=>` are supported:

```incan
match opt:
  case Some(x):   # Python style
    return x
  None => 0       # Rust shorthand style
```

### Error Handling

The `?` operator propagates `Result` errors:

- Only works on `Result[T, E]` types
- Function must return compatible `Result[_, E]`
- Type checker enforces error type compatibility

### Mutability

- Bindings are immutable by default
- Use `mut` for mutable bindings: `mut x = 1`
- Methods use `mut self` for mutable receiver

## Testing

Run tests with:

```bash
cargo test
```

> Note: you can also use the make command to run tests: `make test`

Test fixtures are in `tests/fixtures/`:

- `valid/` - Files that should compile successfully
- `invalid/` - Files that should produce errors

## Usage

From CLI:

```bash
# Tokenize only
cargo run -- --lex file.incn

# Parse only  
cargo run -- --parse file.incn

# Full type check
cargo run -- --check file.incn
cargo run -- file.incn
```

Programmatically:

```rust
use incan::frontend::{lexer, parser, typechecker};

let source = "def add(a: int, b: int) -> int:\n  return a + b";
let tokens = lexer::lex(source)?;
let ast = parser::parse(&tokens)?;
typechecker::check(&ast)?;
```

## Error Messages

The diagnostics module provides Python-friendly error messages with:

- Source location (file:line:col)
- Highlighted source context
- Helpful hints and notes

Example:

```bash
error[type]: Cannot mutate 'x' - binding is immutable
  --> file.incn:5:3
   |
 5 |   x = 2
   |   ^^^
   = hint: Declare with 'mut' to allow mutation: mut x = ...
```

## Common Error Patterns

| Error | Cause | Fix |
| ----- | ----- | --- |
| Unknown symbol | Undefined variable | Define or import it |
| Type mismatch | Wrong type | Fix type or add conversion |
| Cannot use '?' | Not a Result | Use Result type or handle explicitly |
| Mutation without mut | Immutable binding | Add `mut` keyword |
| Non-exhaustive match | Missing patterns | Add missing cases or wildcard |

## Future Work

- [ ] Better type inference for generics
- [ ] More sophisticated match exhaustiveness checking
- [ ] Derive macro expansion
- [ ] Trait method resolution with inheritance
- [ ] Better f-string expression parsing
