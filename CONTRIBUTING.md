# Contributing to Incan

Thank you for your interest in contributing to the Incan programming language! This document provides guidelines for contributing to the project.

## Start Here (Docs)

- **Contributor docs (this repo)**: see `docs/contributing/`
  - [Contributor Docs Index](docs/contributing/README.md)
  - [Extending the Language](docs/contributing/extending_language.md) — when to add builtins vs new syntax
- **Compiler architecture overview**: [docs/architecture.md](docs/architecture.md)

## Getting Started

1. **Clone the repository**

   ```bash
   git clone https://github.com/encero-systems/incan
   cd incan
   ```

2. **Build the project**

   ```bash
   cargo build
   ```

3. **Run the tests**

   ```bash
   cargo test
   ```

## Project Structure

The compiler is organized into a **frontend** (lex/parse/typecheck), a **backend** (lowering + Rust emission), plus CLI and tooling.

For an up-to-date module map, see:

- [Compiler Architecture](docs/architecture.md) (includes a module layout table)

## Key Development Tasks

### Bumping the Version

The workspace uses **Cargo workspace package metadata**, so you only bump versions in **one place**.

1. Edit the root `Cargo.toml` and update:
   - `[workspace.package] version = "..."` (this is the single source of truth)
2. Verify everything still passes:
   - `cargo test`
   - `make pre-commit` (fast local gate)
   - `make pre-commit-full` (before pushing / opening PR)
3. Commit the change.

Notes:

- The compiler exposes the version as `incan::version::INCAN_VERSION`, backed by `env!("CARGO_PKG_VERSION")`, so it updates automatically with the Cargo version.
- Codegen snapshots are version-agnostic (they normalize the codegen header to `v<INCAN_VERSION>`), so version bumps should not churn snapshot files.

### Code Generation Overview

The code generation pipeline is:

```text
Incan AST → AstLowering → IR → IrEmitter (syn/quote) → prettyplease → Rust source
```

The single public entry point is `IrCodegen`:

```rust
use incan::backend::IrCodegen;

let mut codegen = IrCodegen::new();
let rust_code = codegen.generate(&ast);
```

Key files:

- `ir/codegen.rs`: **Public entry point** (`IrCodegen`) - use this!
- `ir/lower.rs`: AST to IR lowering (`AstLowering`)
- `ir/emit.rs`: IR to Rust emission using syn/quote (`IrEmitter`)
- `ir/conversions.rs`: Type conversions (string literals, borrows, ownership)
- `ir/types.rs`, `ir/expr.rs`, `ir/stmt.rs`, `ir/decl.rs`: IR type definitions

### Type Conversions System

The `conversions` module (`src/backend/ir/conversions.rs`) provides centralized handling of type conversions and borrow checking during Rust codegen. This is where we handle the mismatch between Incan's simple `str` type and Rust's `&str` vs `String` split for example.

**When to use conversions:**

The `emit.rs` module automatically applies conversions at 4 key points:

1. **Let bindings** - `let name: str = "Alice"` → `let name: String = "Alice".to_string();`
2. **Return statements** - `return "value"` → `return "value".to_string();`
3. **Function call arguments** - distinguishes Incan functions (owned) vs external Rust functions (borrowed)
4. **Struct field initialization** - `User(name="Alice")` → `User { name: "Alice".to_string() }`

**Don't add ad-hoc conversions** - use `determine_conversion()` from the conversions module:

```rust
use super::conversions::{determine_conversion, ConversionContext};

let conversion = determine_conversion(
    expr,                              // IR expression
    Some(&target_type),                // Expected type
    ConversionContext::IncanFunctionArg  // Usage context
);
let converted = conversion.apply(emitted_tokens);
```

See `src/backend/ir/conversions.rs` for detailed documentation and 23 test cases covering all scenarios.

### Adding a New Builtin Function

This guidance can be found here:

- See [Extending the Language](docs/contributing/extending_language.md) for the current builtin pipeline
- For the builtins emitter implementation, see `src/backend/ir/emit/expressions/builtins.rs`

Example:

```rust
"my_builtin" => {
    if let Some(arg) = args.first() {
        let a = self.emit_expr(arg)?;
        return Ok(quote! { my_rust_impl(#a) });
    }
}
```

### Adding a New Expression Type

See [Extending the Language](docs/contributing/extending_language.md) for the up-to-date end-to-end checklist (lexer → parser/AST → typechecker → lowering → IR → emission).

### Running Snapshot Tests

We use `insta` for golden snapshot tests:

```bash
# Run codegen snapshot tests
cargo test --test codegen_snapshot_tests

# Review and accept changes
cargo insta review
```

Snapshot files are in `tests/snapshots/`.

## Code Style

- **Clippy**: We enforce `deny(clippy::unwrap_used)` in CLI/backend modules
- **Error Handling**: Use `Result` types, avoid panics in production code
- **Documentation**: Add doc comments for public functions
- **Tests**: Add tests for new functionality

## Pull Request Guidelines

1. **Run tests**: `cargo test`
2. **Run clippy**: `cargo clippy`
3. **Format**: `cargo +nightly fmt` (nightly rustfmt is required for comment/doc formatting settings)
4. **Update snapshots** if codegen changed: `cargo insta review`
5. **Write descriptive commit messages**

## Architecture Notes

### Panic Policy

From `src/lib.rs`:

> The compiler should not panic under normal operation. All user-facing errors should be returned
> as `Result` types and handled gracefully.
>
> Exception: Codegen may emit `.unwrap()` and `.expect()` **as literal strings** in generated Rust
> code. This is intentional - runtime errors
> in generated code should panic with clear messages.

### CLI Design

The CLI uses clap with derive macros. Commands return `CliResult<ExitCode>` instead of calling `process::exit` directly. This makes commands testable.

### Prelude Status

The stdlib surface now compiles through the normal pipeline under `crates/incan_stdlib/stdlib/`. Source declarations are the primary contract for `std.*` modules, including the prelude-facing trait definitions. Some behavior is still realized by backend lowering or runtime bridges (for example derive-backed Rust traits and host-backed stdlib leaves), but the compiler no longer treats the stdlib as documentation-only stubs.

### Property-Based Testing

We use `proptest` for property-based testing of complex invariants.

Property tests are in `tests/property_tests.rs` and verify:

- Formatting is idempotent
- Formatting preserves parseability
- Type conversions are deterministic

Run property tests:

```bash
cargo test --test property_tests
```

## Macro Discipline

Macros are powerful but can make code harder to understand. We follow strict guidelines:

### Declarative Macros (`macro_rules!`)

**Policy**: Declarative macros are **not allowed** in the main codebase outside of `crates/incan_derive`.

**Rationale**: `macro_rules!` macros hide control flow and make debugging difficult. Use functions and generics instead.

**Exception**: Derive macros in `crates/incan_derive/` may use `macro_rules!` for internal helpers.

### Procedural Macros (Derive Macros)

**Location**: `crates/incan_derive/`

**Requirements**:

1. **Documentation**: Every derive macro must have rustdoc explaining what it generates
2. **Examples**: Include expansion examples in docs
3. **Testing**: Test with and without the derive
4. **Error messages**: Provide clear compile errors for invalid usage

**Example**: See `crates/incan_derive/src/lib.rs` for current patterns.

### `quote!` Usage in Backend

**Location**: `src/backend/ir/emit.rs`, `src/backend/ir/conversions.rs`

**Guidelines**:

- Use `quote!` for **all** Rust code generation - never string concatenation
- Keep `quote!` blocks small and focused (prefer helper functions)
- Use `#variable` syntax for interpolation, never format strings
- Apply `prettyplease` for final formatting

**Example**:

```rust
// Good
let name = format_ident!("user");
let ty = quote! { String };
quote! {
    pub struct #name {
        name: #ty,
    }
}

// Bad - string concatenation
format!("pub struct {} {{ name: String }}", name)
```

### `syn` Usage

**Location**: `src/backend/ir/emit.rs`

**Guidelines**:

- Use `syn` types (`Type`, `Expr`, `Stmt`) for complex Rust AST construction
- Prefer `parse_quote!` for converting quote blocks to syn types
- Use `ToTokens` trait to convert syn types back to `TokenStream`

**When to use `syn` vs `quote!`**:

- Simple code (< 5 lines): `quote!` is fine
- Complex code (functions, structs with many fields): use `syn` types
- Need to manipulate generated code: use `syn`, then convert to tokens

## Questions?

Open an issue or reach out via the repository's discussion board.

## License

By contributing, you agree that your contributions will be licensed under the Apache 2.0 license.
