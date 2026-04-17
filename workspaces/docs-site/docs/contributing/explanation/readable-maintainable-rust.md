
# Readable & Maintainable Rust: A Practical Guide

This document contains a pragmatic set of principles, patterns, and guardrails for writing Rust that's easy to understand
and safe to evolve. It draws from [The Rust Book](https://doc.rust-lang.org/book/) and the
[Rust API Guidelines](https://rust-lang.github.io/api-guidelines/about.html).

Incan contributors are expected to follow these principles and patterns when writing Rust code.

## Table of Contents

- [What "Readable" Means in Rust](#what-readable-means-in-rust)
- [What "Maintainable" Means in Rust](#what-maintainable-means-in-rust)
- [Pragmatic Checklist](#pragmatic-checklist)
- [Idiomatic Examples](#idiomatic-examples)
    - [Errors: Internal vs External](#1-errors-internal-vs-external)
    - [Public API: Trait-Oriented, Minimal Exposure](#2-public-api-trait-oriented-minimal-exposure)
    - [Ownership Clarity: Borrow When Possible](#3-ownership-clarity-borrow-when-possible)
    - [Macro Discipline](#4-macro-discipline)
- [Project Scaffolding](#project-scaffolding)
    - [lib.rs and main.rs](#librs-and-mainrs)
    - [`rustfmt.toml`](#rustfmttoml)
    - [`Cargo.toml`](#cargotoml)
    - [Typical CI Steps](#typical-ci-steps)
- [Common Pitfalls & Anti-Patterns](#common-pitfalls-anti-patterns)
- [Simple Heuristic](#simple-heuristic)

---

## What "Readable" Means in Rust

Readable Rust is code that a competent Rustacean can **scan and understand quickly**, without excessive file-hopping or
ownership gymnastics.

1. **Idiomatic ownership & borrowing**
   - Prefer borrowing (`&T`, `&mut T`) over cloning; clone only at clear boundaries.
   - Lifetimes are **implicit** when possible; explicit lifetimes are localized and named meaningfully when needed.
   - Avoid `Rc<RefCell<_>>` in single-threaded code and `Arc<Mutex<_>>` in multi-threaded code unless truly necessary.

2. **Type-first design**
   - Use **newtypes** to encode domain invariants (e.g., `UserId`, `NonEmptyStr`).
   - Prefer expressive enums over ad-hoc booleans/strings (e.g., `enum State { Pending, Running, Failed }`).
   - Return **domain-specific results**: `Result<Order, OrderError>` beats `Result<T, String>`.

3. **Clear error handling**
   - Use `Result<T, E>` pervasively; reserve panics for programmer errors (`unreachable!`, `expect` with invariant context).
   - Propagate with `?`; keep error messages actionable; don't lose context when mapping.

4. **Minimal magic**
   - Macros remove boilerplate but don't hide logic or create opaque DSLs.
   - Generics are purposeful; trait bounds are explicit and readable.

5. **Consistent naming & layout**
   - Follow conventions: `snake_case` (functions/vars), `CamelCase` (types/traits), `SCREAMING_SNAKE_CASE` (consts).
   - Module/file sizes are reasonable; split by responsibility, not arbitrary layers.

6. **Local reasoning**
   - Functions are small, pure where possible, with limited side effects and clear inputs/outputs.

---

## What "Maintainable" Means in Rust

Maintainable Rust withstands change with **minimal risk and effort**—thanks to stable seams, guardrails, and predictable
behavior.

1. **Stable, minimal public API**
   - Public types/traits are small and composable; prefer **capability-oriented traits** over "god traits."
   - Keep internals private (`mod` visibility) unless exposure is justified.
   - Avoid leaking concrete types when `impl Trait` or trait objects suffice.

2. **Explicit boundaries & layering**
   - Separate pure domain logic, adapters, and IO. Keep `async` at the edges.
   - One clear responsibility per module; lifecycle ownership is explicit.

3. **Error taxonomy**
   - Categorize errors (user, infra, programming) and indicate recoverability.
   - Prefer stable, typed public errors (e.g., via `thiserror`) and keep error messages actionable and consistent.
   - In this repo: compiler diagnostics are typically built with `miette`, and shared runtime/semantic errors should
        stay aligned (see [Layering Rules](../explanation/layering.md)).

4. **Concurrency discipline**
   - Make `Send + Sync` requirements explicit; guard shared state predictably.
   - Use **structured concurrency** (task groups, cancellation) over "fire-and-forget."
   - Prefer channels/async streams for coordination when appropriate.

5. **Tooling baked in**
   - `rustfmt` and `clippy` are non-negotiable in CI; tune lints to project needs.
   - Dependency hygiene: `cargo-audit`, `cargo-deny`, `cargo-udeps`; pin MSRV.
   - Tests include unit, integration, **doctests**, property-based (`proptest`) for parsers.

6. **Safety & invariants**
   - `unsafe` is rare, isolated, and annotated with **SAFETY:** comments plus tests.
   - Invariants are encoded in types and constructors, not just comments.

7. **Performance with restraint**
   - Measure before optimizing (e.g., `criterion`); avoid premature micro-optimizations.
   - Favor simpler code that compiles fast unless profiling proves otherwise.

---

## Pragmatic Checklist

**If these are true, your Rust is both readable and maintainable:**

- ✅ **Formatting & lints**: `cargo +nightly fmt --all -- --check` and `cargo clippy --deny warnings` pass; lint level tuned
  to project.
- ✅ **Boundaries**: Modules align with responsibilities; public surface area small and documented.
- ✅ **Errors**: Clear `Result<T, E>` with actionable messages; categorized `E`.
- ✅ **Ownership**: Minimal clones; predictable lifetimes; borrowing favored.
- ✅ **Tests**: Unit + integration + doctests exist; critical logic property-tested.
- ✅ **Docs**: `rustdoc` examples compile; `README` shows usage; `SAFETY:` annotations present where needed.
- ✅ **CI**: Audit, deny, udeps, MSRV checks run; semantic versioning for releases.
- ✅ **Async/concurrency**: Structured, cancellable; no hidden global mutable state.

---

## Idiomatic Examples

### 1) Errors: Internal vs External

```rust
// External API error (stable, documented)
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("network error: {0}")]
    Network(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("timeout after {0:?}")]
    Timeout(std::time::Duration),
}

// Internal plumbing should preserve context while avoiding opaque stringly errors.
fn perform_io() -> Result<(), std::io::Error> {
    let _data = std::fs::read("config.json")?;
    // ...
    Ok(())
}
```

### 2) Public API: Trait-Oriented, Minimal Exposure

```rust
pub trait Storage {
    fn put(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError>;
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError>;
}

pub struct S3Storage { /* fields private */ }

impl Storage for S3Storage {
    fn put(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError> {
        // ...
        Ok(())
    }
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        // ...
        Ok(None)
    }
}

// Factory returns impl Trait to avoid leaking concrete type
pub fn storage_from_env() -> impl Storage {
    S3Storage { /* ... */ }
}
```

### 3) Ownership Clarity: Borrow When Possible

```rust
fn normalize(input: &str) -> String {
    input.trim().to_lowercase()
}
```

Avoid:

```rust
fn normalize(input: String) -> String {
    // Forces ownership and can cause unnecessary clones elsewhere
    input.trim().to_lowercase()
}
```

### 4) Macro Discipline

```rust
/// Generates simple enum Display impls.
/// Use sparingly; document the expansion in examples.
#[macro_export]
macro_rules! impl_display {
    ($t:ty, $($v:ident),+ $(,)?) => {
        impl std::fmt::Display for $t {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    $(Self::$v => write!(f, stringify!($v)),)+
                }
            }
        }
    };
}
```

---

## Project Scaffolding

### lib.rs and main.rs

```rust
#![forbid(unsafe_code)]
#![deny(clippy::all, clippy::cargo)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)] // tune to taste
```

### `rustfmt.toml`

```toml
edition = "2024"
max_width = 120
```

### `Cargo.toml`

```toml
[package]
edition = "2024"
rust-version = "1.85" # MSRV pinned (keep in sync with CI)

[dependencies]
thiserror = "1"

[features]
default = []
```

### Typical CI Steps

```bash
cargo +nightly fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all --verbose
cargo audit
cargo deny check
cargo +nightly udeps --all-targets
```

Note: MSRV is enforced via CI running build/test with a pinned toolchain.

---

## Common Pitfalls & Anti-Patterns {#common-pitfalls-anti-patterns}

### Quick-reference

|                  Instead of                   |                 Prefer                 |                                Why                                |
| --------------------------------------------- | -------------------------------------- | ----------------------------------------------------------------- |
| `.unwrap()` / `.expect()` in library code     | `?`, `.context()`, or explicit `match` | Panics crash the process; propagate errors instead                |
| `.clone()` to appease the borrow checker      | Restructure ownership or borrow        | Hides design issues and adds unnecessary allocations              |
| `&String`, `&Vec<T>`, `&Box<T>` in parameters | `&str`, `&[T]`, `&T`                   | More general — accepts owned and borrowed callers alike           |
| `x as u32` (silent truncation)                | `x.try_into()` or `From`/`Into`        | `as` silently wraps/truncates; conversions should be explicit     |
| `use foo::*` (wildcard imports)               | `use foo::{Bar, Baz}`                  | Makes origins clear; avoids surprise breakage on upstream changes |
| `.collect::<Vec<_>>()` just to re-iterate     | Chain iterators directly               | Avoids an unnecessary allocation + copy                           |
| `pub` on everything                           | `pub(crate)` or private by default     | Minimize public surface; promote visibility only when needed      |
| Blocking I/O in `async fn`                    | `tokio::fs`, `spawn_blocking`          | Blocks the executor and starves other tasks                       |
| `Result<T, String>` in public APIs            | A typed error enum (`thiserror`)       | Stringly-typed errors are hard to match on and evolve             |
| `Rc<RefCell<T>>` everywhere                   | Restructure data / ownership           | Usually signals a design that fights the borrow checker           |

### Detailed guidance

#### Panics on recoverable paths

In this repository, do **not** use `.unwrap()` or `.expect()` in compiler code, tests, or examples. Even when an invariant feels obvious, prefer `?`, an explicit `match`, or a helper that turns the failure into a typed error with context. The only accepted exception is emitting `.unwrap()` / `.expect()` as literal strings in generated Rust when that is the intended runtime contract of the compiled program.

```rust
// Bad — panics with no context if the file is missing
let file = File::open(path).unwrap();

// Good — propagates a meaningful error to the caller
let file = File::open(path)
    .map_err(|e| anyhow!("failed to open {}: {e}", path.display()))?;
```

#### Cloning — question every `.clone()`

A `.clone()` is sometimes the right answer (crossing an API boundary, shared ownership), but treat every occurrence
as a review signal. Ask: *can I borrow instead? Can I restructure to remove the simultaneous borrow?*

#### Stringly-typed APIs

Use enums or newtypes instead of raw strings for fixed sets of values:

```rust
// Bad — typo-prone, no exhaustiveness checking
fn set_role(role: &str) { /* ... */ }

// Good — the compiler enforces valid values
enum Role { Admin, User }
fn set_role(role: Role) { /* ... */ }
```

#### Parameter types — accept the most general borrow

```rust
// Bad — forces callers to own a String / Vec
fn process(input: &String) { /* ... */ }
fn filter(items: &Vec<Item>) { /* ... */ }

// Good — accepts &String, &str, string literals, slices, etc.
fn process(input: &str) { /* ... */ }
fn filter(items: &[Item]) { /* ... */ }
```

#### Type casting — prefer safe conversions

`as` silently truncates or wraps. Prefer `TryFrom`/`TryInto` for fallible conversions and `From`/`Into` for
infallible ones:

```rust
// Bad — silently wraps on overflow
let small = big_number as u16;

// Good — panics with a message instead of silently producing garbage
let small: u16 = big_number.try_into().expect("value fits in u16");
```

#### Iterator chains — don't collect just to iterate again

```rust
// Bad — allocates a Vec only to loop over it
let v: Vec<_> = items.iter().filter(|x| x.is_valid()).collect();
for item in &v { process(item); }

// Good — zero extra allocation
for item in items.iter().filter(|x| x.is_valid()) {
    process(item);
}
```

#### Visibility — start private, widen deliberately

Default to private. Use `pub(crate)` for crate-internal sharing. Only use `pub` for your actual public API. This
keeps refactoring safe and communicates intent to future readers.

#### Match arms — factor out shared logic

```rust
// Bad — duplicated setup/cleanup in every arm
match kind {
    A => { setup(); handle_a(); cleanup(); }
    B => { setup(); handle_b(); cleanup(); }
}

// Good — shared logic lives outside the match
setup();
match kind {
    A => handle_a(),
    B => handle_b(),
}
cleanup();
```

#### `#[must_use]` on important return values

If a function returns a value that callers should never silently discard (validation results, builders, etc.), mark
it `#[must_use]`. Rust already enforces this for `Result`, but custom types often miss it.

### Architecture-level pitfalls

- **Overusing generics**: Complex bounds and many type params — prefer trait objects or smaller traits.
- **Lifetime gymnastics**: If lifetimes leak everywhere, restructure ownership or use owned buffers at boundaries.
- **Macro-heavy APIs**: If consumers must "think in macros," reconsider the design.
- **Async everywhere**: Keep `async` at IO boundaries; don't make pure functions `async`.
- **Global mutable state**: Hide state behind interfaces; avoid implicit singletons.

### Clippy

`cargo clippy` is mandatory in CI. Common catches worth internalizing:

- `if let Some(_) = x` — use `x.is_some()` instead
- Manual `Default` impl when `#[derive(Default)]` works
- `.map(|x| foo(x))` — use `.map(foo)` (redundant closure)
- `.map().unwrap_or()` — use `.map_or()`

---

## Simple Heuristic

> If a teammate can fix a bug or add a feature **without asking you to explain ownership or lifetimes**, and **CI**
> tells them when they're wrong — your Rust is both **readable** and **maintainable**.
