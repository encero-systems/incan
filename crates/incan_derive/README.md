# incan_derive

Derive macros for the Incan programming language standard library.

This crate provides procedural macros that generate boilerplate implementations for Incan language features. These macros are automatically used by the Incan compiler when you use decorators like `@derive(...)` in your Incan code.

## Purpose

When you write:

```incan
@derive(Debug, Clone, FieldInfo)
model User:
    name: str
    age: int
```

The Incan compiler translates this to Rust with appropriate derive macros:

```rust
#[derive(Debug, Clone, FieldInfo)]
pub struct User {
    pub name: String,
    pub age: i64,
}
```

This crate provides those macros.

## Available Macros

### `#[derive(FieldInfo)]`

Implements the `incan_stdlib::HasFieldInfo` trait, enabling compile-time reflection:

```rust
use incan_stdlib::HasFieldInfo;
use incan_derive::FieldInfo;

#[derive(FieldInfo)]
struct Person {
    name: String,
    age: i64,
}

// Generated implementation provides:
assert_eq!(Person::field_names(), vec!["name", "age"]);
assert_eq!(Person::field_types(), vec!["String", "i64"]);
```

**Incan equivalent**: `@derive(FieldInfo)` or automatic for all models/classes

### `#[derive(IncanClass)]`

Generates Python-style dunder methods for class name reflection:

```rust
#[derive(IncanClass)]
struct Config {
    host: String,
    port: i64,
}

let config = Config { host: "localhost".into(), port: 8080 };
assert_eq!(config.__class__(), "Config");
```

**Incan equivalent**: Automatic for all classes with methods

### `#[derive(IncanJson)]`

Generates convenience methods for JSON serialization (requires `serde` derives):

```rust
fn demo() -> Result<(), Box<dyn std::error::Error>> {
#[derive(Serialize, Deserialize, IncanJson)]
struct ApiResponse {
    status: i64,
    message: String,
}

let response = ApiResponse { status: 200, message: "OK".into() };
let json = response.to_json();           // Compact JSON string
let pretty = response.to_json_pretty();  // Pretty-printed JSON
let parsed = ApiResponse::from_json(&json)?;
# let _ = pretty;
# let _ = parsed;
# Ok(())
}
```

**Incan equivalent**: `from std.serde import json` plus `@derive(json)`

### `#[derive(IncanReflect)]`

Alias for `IncanClass` - same functionality, clearer name in some contexts.

## Architecture Notes

### Why a Separate Proc-Macro Crate?

Rust requires procedural macros to live in a crate with `proc-macro = true` in `Cargo.toml`. Such crates can **only** export proc macros, not regular code. This is why we have:

- **`incan_derive`** (this crate) - The macro definitions
- **`incan_stdlib`** - The trait definitions the macros implement

### Code Generation Strategy

These macros use `quote!` and `syn` to generate clean, idiomatic Rust code. They're designed to produce zero-cost abstractions that compile away to the same code you'd write by hand.

### Interaction with Incan Compiler

The compiler automatically:

1. Detects `@derive(...)` decorators in Incan source
2. Maps them to Rust derive attributes
3. Adds prerequisite derives (e.g., `Eq` requires `PartialEq`)
4. Ensures both `incan_stdlib` and `incan_derive` are in scope

You should never need to use these macros directly - the compiler handles it.

## Development

### Testing

Derive macros are tested indirectly through the Incan compiler test suite. See the main workspace `tests/` directory for examples.

### Adding New Macros

When adding a new derive macro:

1. Define the trait in `incan_stdlib`
2. Implement the proc macro here
3. Update the compiler's `lower.rs` to emit the derive
4. Add snapshot tests in `tests/codegen_snapshot_tests.rs`

## Version Compatibility

This crate must stay in sync with `incan_stdlib`. They share version numbers and are released together.

## License

Apache 2.0
