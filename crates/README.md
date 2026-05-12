# Incan Standard Library Architecture

This document explains the two-crate standard library structure for Incan-generated code.

## Overview

When Incan compiles your code to Rust, the generated programs depend on two support crates:

```bash
┌──────────────────────────────────────────┐
│  Your Incan Program                      │
│  examples/advanced/derives_and_json.incn │
└──────────────┬───────────────────────────┘
               │ compiles to
               ▼
┌──────────────────────────────────────────┐
│  Generated Rust Code                     │
│  `use incan_stdlib::prelude::*;`         │
│  `use incan_derive::FieldInfo;`          │
│                                          │
│  #[derive(Debug, Clone, FieldInfo)]      │
│  pub struct User { ... }                 │
└──────┬──────────────────────┬────────────┘
       │                      │
       ▼                      ▼
┌──────────────┐      ┌───────────────────┐
│incan_stdlib  │      │  incan_derive     │
│              │      │                   │
│Traits:       │      │ Macros:           │
│• FieldInfo   │      │ • #[derive(...)]  │
│• ToJson      │◄─────┤   implementations │
│• FromJson    │      │                   │
└──────────────┘      └───────────────────┘
```

## Why Two Crates?

### Rust Constraint: Proc Macros Must Be Separate

Rust requires procedural macros (like `#[derive(...)]`) to live in a crate with `proc-macro = true` in `Cargo.toml`. Such crates can **only** export proc macros, not regular Rust code like traits or structs.

This is why we need:

1. **`incan_derive`** - The proc-macro crate that generates implementations
2. **`incan_stdlib`** - The library crate that defines what to implement

### Real-World Pattern

This is the standard Rust pattern. Examples from the ecosystem:

- `serde` + `serde_derive`
- `tokio` + `tokio_macros`
- `diesel` + `diesel_derives`

## What Each Crate Does

### `incan_stdlib` - The Standard Library

**Purpose**: Defines the traits and utilities that Incan programs use

**Contains**:

- `HasFieldInfo` trait - Reflection (field names/types)
- `ToJson`/`FromJson` traits - JSON helpers
- The `prelude` module for convenient imports

**Used by**: All generated Incan programs

**Import in generated code**:

```rust
use incan_stdlib::prelude::*;
```

### `incan_derive` - The Derive Macros

**Purpose**: Generates the boilerplate implementations of stdlib traits

**Contains**:

- `#[derive(FieldInfo)]` - Implements the `HasFieldInfo` trait
- `#[derive(IncanClass)]` - Generates `__class__()` and `__class_name__()` methods
- `#[derive(IncanJson)]` - Generates JSON helper methods

**Used by**: All generated Incan programs with models/classes

**Import in generated code**:

```rust
use incan_derive::FieldInfo;
```

## Generated Code Pattern

When you write this Incan code:

```incan
@derive(Debug, Clone)
model User:
    name: str
    age: int
```

The compiler generates this Rust code:

```rust
use incan_stdlib::prelude::*;  // Get the HasFieldInfo trait + FieldInfo record
use incan_derive::FieldInfo;    // Get the derive macro

#[derive(Debug, Clone, FieldInfo)]
pub struct User {
    pub name: String,
    pub age: i64,
}

// Now `User` implements `HasFieldInfo`:
// User::field_names() => ["name", "age"]
// User::field_types() => ["String", "i64"]
```

## Compiler Integration

The Incan compiler handles all of this automatically:

1. **Parser** - Detects `@derive(...)` decorators in Incan source
2. **Lowering** - Maps decorators to Rust derive attributes
3. **Emit** - Adds both `use incan_stdlib::prelude::*` and `use incan_derive::FieldInfo`
4. **Project Generator** - Adds both crates as dependencies in generated `Cargo.toml`

## For Contributors

### Adding a New Derive Feature

1. **Define the trait** in `crates/incan_stdlib/src/`

   ```rust
   pub trait MyTrait {
       fn my_method(&self) -> String;
   }
   ```

2. **Export in prelude**: `crates/incan_stdlib/src/prelude.rs`

   ```rust
   pub use crate::my_trait::MyTrait;
   ```

3. **Implement the macro** in `crates/incan_derive/src/lib.rs`

   ```rust
   #[proc_macro_derive(MyTrait)]
   pub fn derive_my_trait(input: TokenStream) -> TokenStream {
       // Implementation using syn/quote
   }
   ```

4. **Wire to compiler** in `src/backend/ir/lower.rs`
   - Update `extract_derives()` to recognize the new decorator

5. **Add tests** in `tests/codegen_snapshot_tests.rs`

### Testing

Both crates are tested indirectly through the compiler test suite:

```bash
# Run all tests
cargo test

# Test snapshot generation
cargo test --test codegen_snapshot_tests

# Test a specific example
./target/release/incan run examples/advanced/derives_and_json.incn
```

## Version Synchronization

Both crates share the same version number and are released together:

- `incan_stdlib` v0.1.0
- `incan_derive` v0.1.0

Generated code always depends on matching versions to ensure compatibility.

## Further Reading

- [`crates/incan_stdlib/README.md`](../crates/incan_stdlib/README.md) - stdlib API reference
- [`crates/incan_derive/README.md`](../crates/incan_derive/README.md) - derive macro reference
- [RFC 002: Testing Framework](../docs/RFCs/002-testing-framework.md) - How fixtures/parametrize work
- [RFC 005: Rust Interop](../docs/RFCs/005-rust-interop.md) - Using Rust crates from Incan
