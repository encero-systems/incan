# Contributing to Incan

Thank you for your interest in contributing to the Incan programming language! This document provides guidelines for contributing to the project.

## Start Here (Docs)

- **Contributor docs (this repo)**: see `workspaces/docs-site/docs/contributing/`
  - [Contributor Docs Index](workspaces/docs-site/docs/contributing/index.md)
  - [Extending the Language](workspaces/docs-site/docs/contributing/how-to/extending_language.md) — when to add builtins vs new syntax
- **Compiler architecture overview**: [Architecture](workspaces/docs-site/docs/contributing/explanation/architecture.md)

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

4. **Install the commit hooks**

   ```bash
   make install-hooks
   ```

   This points `core.hooksPath` at the repository's `.githooks/`. The `commit-msg` hook rejects AI attribution trailers, session trailers, generation footers, and a tool git identity, which several agent harnesses inject by default. Commit messages in this repository are authored by their committer and carry no tool attribution.

## Project Structure

The compiler is organized into a **frontend** (lex/parse/typecheck), a **backend** (lowering + Rust emission), plus CLI and tooling.

For an up-to-date module map, see:

- [Compiler Architecture](workspaces/docs-site/docs/contributing/explanation/architecture.md) (includes a module layout table)

## Key Development Tasks

### Bumping the Version

The toolchain version lives in **one place**, the root `Cargo.toml`'s `[workspace.package] version`; the kernel, compiler and toolchain crates inherit it. Two rings ship on their own line — the Oven (`oven_model`, `oven_store`, `oven_rustc`) and the stdlib runtime (`incan_stdlib`, `incan_derive`, `incan_web_macros`) — and `incan_vocab`, the vocabulary registration contract, has carried its own since before the rings existed. Each of those declares an explicit `version` in its manifest, and the root `[workspace.dependencies]` entry for it repeats that line as a requirement beside its `path`; `scripts/check_ring_versions.py` (part of `make version-gate`) keeps the crates of a ring, and the table, in agreement.

1. Edit the root `Cargo.toml` and update `[workspace.package] version = "..."`. Bump a ring's line in the same change only when that ring actually changed: edit its crates' `version` and the matching `version` in the root table — and, for the stdlib ring, `incan_emit::GENERATED_FOR_STDLIB_VERSION`, the line the compiler generates code for (it does not link the runtime, so it declares the line instead).
2. Verify everything still passes:
   - `cargo test`
   - `make pre-commit-fast` (fast local gate)
   - `make pre-commit` (full gate before pushing / opening PR)
3. Commit the change.

Notes:

- The compiler exposes the toolchain version as `incan::version::INCAN_VERSION`, backed by `env!("CARGO_PKG_VERSION")`, so it updates automatically with the workspace version.
- Generated code carries `incan_std_core::__incan_stdlib_version_check!("<stdlib line the compiler generates for>")`; the linked stdlib must be compatible with it (exactly equal for a prerelease line, same major.minor and no older patch for a release), so a stdlib bump that changes what generated code compiles against is a compatibility event, not a formality.
- Codegen snapshots are version-agnostic (they normalize the codegen header to `v<INCAN_VERSION>` and the stdlib check to `<INCAN_STDLIB_VERSION>`), so version bumps should not churn snapshot files.

### Code Generation Overview

The code generation pipeline is:

```text
Incan AST → AstLowering → IR → IrEmitter (syn/quote) → prettyplease → Rust source
```

The single public entry point is `IrCodegen`:

```rust
use incan_emit::IrCodegen;

let codegen = IrCodegen::new();
let rust_code = codegen.generate(&ast);
```

Key files:

- `loaves/compiler/incan_emit/src/codegen.rs`: **Public entry point** (`IrCodegen`) - use this!
- `loaves/compiler/incan_ir/src/lower/mod.rs`: AST to IR lowering (`AstLowering`)
- `loaves/compiler/incan_emit/src/emit/mod.rs`: IR to Rust emission using syn/quote (`IrEmitter`)
- `loaves/compiler/incan_emit/src/conversions.rs`: Type conversions (string literals, borrows, ownership)
- `loaves/compiler/incan_ir/src/`: IR type definitions in `types.rs`, `expr.rs`, `stmt.rs` and `decl.rs`

### Type Conversions System

The `conversions` module (`loaves/compiler/incan_emit/src/conversions.rs`) provides centralized handling of type conversions and borrow checking during Rust codegen. This is where we handle the mismatch between Incan's simple `str` type and Rust's `&str` vs `String` split for example.

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

See `loaves/compiler/incan_emit/src/conversions.rs` for the conversion policy and its focused regression tests.

### Adding a New Builtin Function

This guidance can be found here:

- See [Extending the Language](workspaces/docs-site/docs/contributing/how-to/extending_language.md) for the current builtin pipeline
- For the builtins emitter implementation, see `loaves/compiler/incan_emit/src/emit/expressions/builtins.rs`

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

See [Extending the Language](workspaces/docs-site/docs/contributing/how-to/extending_language.md) for the up-to-date end-to-end checklist (lexer → parser/AST → typechecker → lowering → IR → emission).

### Running Snapshot Tests

We use `insta` for golden snapshot tests:

```bash
# Run codegen snapshot tests
cargo test -p incan_emit --test codegen_snapshot_tests

# Review and accept changes
cargo insta review
```

Snapshot files are in `loaves/compiler/incan_emit/tests/snapshots/`.

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

The rule every crate in the workspace follows (`AGENTS.md` states it as the first rule of the codebase):

> The compiler should not panic under normal operation. All user-facing errors should be returned
> as `Result` types and handled gracefully.
>
> Exception: Codegen may emit `.unwrap()` and `.expect()` **as literal strings** in generated Rust
> code. This is intentional - runtime errors
> in generated code should panic with clear messages.

### CLI Design

The CLI uses clap with derive macros. Commands return `CliResult<ExitCode>` instead of calling `process::exit` directly. This makes commands testable.

### Prelude Status

The stdlib surface now compiles through the normal pipeline under `loaves/stdlib/`. Source declarations are the primary contract for `std.*` modules, including the prelude-facing trait definitions. Some behavior is still realized by backend lowering or runtime bridges (for example derive-backed Rust traits and host-backed stdlib leaves), but the compiler no longer treats the stdlib as documentation-only stubs.

### Property-Based Testing

We use `proptest` for property-based testing of complex invariants.

Property tests are in `loaves/compiler/incan_format/tests/property_tests.rs` and verify:

- Formatting is idempotent
- Formatting preserves parseability
- Type conversions are deterministic

Run property tests:

```bash
cargo test -p incan_format --test property_tests
```

## Macro Discipline

Macros are powerful but can make code harder to understand. We follow strict guidelines:

### Declarative Macros (`macro_rules!`)

**Policy**: Declarative macros are **not allowed** in the main codebase outside of `loaves/stdlib/derive/incan_derive`.

**Rationale**: `macro_rules!` macros hide control flow and make debugging difficult. Use functions and generics instead.

**Exception**: Derive macros in `loaves/stdlib/derive/incan_derive/` may use `macro_rules!` for internal helpers.

### Procedural Macros (Derive Macros)

**Location**: `loaves/stdlib/derive/incan_derive/`

**Requirements**:

1. **Documentation**: Every derive macro must have rustdoc explaining what it generates
2. **Examples**: Include expansion examples in docs
3. **Testing**: Test with and without the derive
4. **Error messages**: Provide clear compile errors for invalid usage

**Example**: See `loaves/stdlib/derive/incan_derive/src/lib.rs` for current patterns.

### `quote!` Usage in Backend

**Location**: `loaves/compiler/incan_emit/src/emit/mod.rs`, `loaves/compiler/incan_emit/src/conversions.rs`

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

**Location**: `loaves/compiler/incan_emit/src/emit/mod.rs`

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

## Checking documentation paths

Run `make doc-paths` to check concrete repository paths in contributor documentation: the root contributor documents, non-state Markdown under `.agents/`, contributor pages under `workspaces/docs-site/docs/contributing/`, and crate READMEs under `loaves/`. Keep these references aligned with the files and directories that own the behavior.

The checker checks concrete paths in every fenced block, including diagrams and shell examples. Commands and diagrams should name real repository inputs or clearly identified example-project files. Record intentional exceptions in `scripts/check_doc_paths.allow`, one tab-separated document, exact path token, and reason per line. The document field may be `*`; a path exception may use a trailing `/**` for a subtree. Other wildcard exception patterns are unsupported. Prefer a document-specific exact token for an illustrative filename; do not exempt a stale implementation path that should be corrected.
