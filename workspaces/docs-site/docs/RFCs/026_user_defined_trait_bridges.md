# RFC 026: User-Defined Trait Bridges

- **Status**: Draft  
- **Author**: Danny Meijer (@dannymeijer)  
- **Issue**: #152
- **RFC PR**: -
- **Created**: 2026-02-19
- **Related**:
    - [RFC 005] (Rust interop - foundation for `rust::` imports)
    - [RFC 023] (Stdlib web module - where trait bridges were first implemented)
    - [RFC 021] (Field metadata - similar decorator pattern)

## Summary

During implementation of RFC 023 (stdlib web module), the stdlib work necessitated us to implement a trait bridge system
that automatically generates Rust trait implementations for newtypes wrapping external types
(e.g., `Query[T]` wrapping Axum's `AxumQuery<T>` gets automatic `FromRequestParts` delegation).

This system is currently hardcoded for stdlib patterns only. This RFC proposes exposing trait bridges to user code via
a new `@rust.delegate` decorator, enabling users to wrap arbitrary external Rust types (`sqlx`, `reqwest`, `diesel`, etc.)
with automatic trait delegation.

## Scope / Non-Goals

**This RFC covers:**

- Trait delegation for newtypes wrapping external Rust types
- Method subsetting and renaming via decorator parameters
- Associated type specification for traits that require them
- Multi-trait delegation with collision detection
- Automatic async trait handling via rust-analyzer introspection
- **Capability parity between stdlib and user code**: delegations used by stdlib modules must be expressible through
    the same `@rust.delegate` mechanism available to users

**This RFC does NOT cover:**

- **Manual `impl` blocks** - Writing full trait implementations with custom logic (see [Future Extensions](#future-extensions))
- **Custom delegation logic** - Delegation is pure forwarding; no custom code in delegated methods
- **Implementing unimplemented traits** - `rust.delegate` can only delegate traits the wrapped type already implements
- **Auto traits** - `Send`, `Sync`, `Unpin` are inferred by Rust automatically (see [Limitations: Auto Traits](#3-limitations-auto-traits))
- **Conditional delegation** - No `#[cfg]` or type-parameter-based conditional delegation
- **Cross-language bridges** - Only Rust trait delegation (no C FFI, Python protocols, etc.)
- **Runtime trait objects** - Delegation is compile-time only; no `dyn Trait` boxing
- **Permanent stdlib-only delegation paths** - hardcoded bridges are treated as migration scaffolding, not long-term
    architecture

This RFC establishes the **decorator surface** and **delegation semantics**. Implementation details may evolve, but the
user-facing contract (decorator parameters, error messages, generated behavior) is the specification boundary.

## Motivation

### What is a Trait Bridge?

A **trait bridge** solves the newtype transparency problem: when you wrap an external Rust type in a newtype, you lose
access to its traits. Without trait bridges, `Query[T]` would just be an opaque wrapper—you couldn't use it as an Axum
extractor because the `FromRequestParts` trait is implemented on `AxumQuery<T>`, not `Query[T]`.

The trait bridge system generates delegation code automatically. For example, for `axum::Query[T]`:

```rust
// Pseudo-Rust: simplified for illustration (actual codegen would include async_trait desugaring)
impl<T, S> FromRequestParts<S> for Query<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
    AxumQuery<T>: FromRequestParts<S>,
{
    type Rejection = <AxumQuery<T> as FromRequestParts<S>>::Rejection;
    
    async fn from_request_parts(
        parts: &mut Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        AxumQuery::<T>::from_request_parts(parts, state)
            .await
            .map(Self)
    }
}
```

This delegation is:

- **Type-safe** - preserves all generic constraints, associated types, and where clauses
- **Transparent** - unwraps the newtype (`self.0`), delegates to wrapped type, re-wraps result
- **Automatic** - triggered by pattern matching on the wrapped type's path during codegen

**Why we need this**: Incan newtypes provide type safety and semantics (e.g., `Query[SearchParams]` vs raw
`AxumQuery<SearchParams>`), but Rust's trait system doesn't automatically forward trait impls through newtype wrappers.
Trait bridges make newtypes truly transparent.

### Current State

Trait bridges work beautifully for stdlib:

```incan
# stdlib/web/request.incn
from rust::axum::extract @ "0.8" import Query as AxumQuery

type Query[T] = newtype AxumQuery[T]
# Compiler auto-generates FromRequestParts impl via hardcoded bridge
```

Backend pattern matching in `src/backend/ir/emit/decls/structures.rs`:

```rust
for bridge in TRAIT_BRIDGES {
    if wrapped_type_path.contains(bridge.applies_to_type_path) {
        generate_delegation_impl(bridge);
    }
}
```

### The Problem

Users **cannot** define their own delegation patterns:

```incan title="user_lib/database.incn"
from rust::sqlx @ "0.7" import PgPool

type Pool = newtype PgPool
# ❌ No trait delegation - PgPool's traits are lost!
```

Users are forced to either:

1. Write manual Rust wrapper code (defeats the purpose)
2. Use raw `PgPool` directly (no type safety)
3. Wait for stdlib support

### The Solution

Let users define trait bridges inline:

```incan
from rust::sqlx @ "0.7" import PgPool

@rust.delegate(
    trait=sqlx::Executor,
    methods=["execute", "fetch_one", "fetch_all"],
)
type Pool = newtype PgPool
# ✅ Generates Rust impl block: makes Pool usable wherever sqlx::Executor is expected
```

The `@rust.delegate` decorator tells the compiler to generate a Rust `impl sqlx::Executor for Pool` that forwards all
trait methods to the wrapped `PgPool`. This preserves Rust's trait-based polymorphism: code expecting an `Executor` will
accept `Pool` because it implements the trait, just like the wrapped type does.

## Design

### Core Syntax: `@rust.delegate`

The decorator attaches to `newtype` declarations and instructs the compiler to generate trait delegation code.

#### Basic Usage

The simplest form delegates specific methods from a single trait:

```incan
import rust::sqlx

@rust.delegate(
    trait=sqlx::Executor,
    methods=["execute", "fetch_one"],
)
type Pool = newtype rust::sqlx::PgPool
```

**Parameters**:

- **`trait`** - The Rust trait to delegate (must be an imported symbol, not a string). The compiler generates
  `impl sqlx::Executor for Pool` that forwards the specified methods to the wrapped `PgPool`.
- **`methods`** - List of method names to delegate. If omitted, delegates **all** trait methods
  (see [Open Questions: Default Delegation Strategy](#2-default-delegation-strategy)).
  Use explicit lists when you only need a subset of the trait's methods.

#### Multiple Traits

When your newtype needs to implement multiple traits, use the `traits` parameter (_note: plural_):

```incan
@rust.delegate(
    traits=[
        sqlx::Executor,
        sqlx::PgExecutor,
        std::fmt::Debug,
    ],
)
type Pool = newtype PgPool
```

**Parameter**:

- **`traits`** - List of trait symbols to delegate. Generates multiple `impl` blocks (one per trait). All traits must
  be implemented on the wrapped type. If method names collide across traits, the compiler rejects with an error
  (see [Multiple Decorator Handling](#1-multiple-decorator-handling)).

**When to use**: Your newtype needs to preserve multiple unrelated trait implementations from the wrapped type
(e.g., database traits + formatting traits).

#### Method Renaming

Sometimes you want to expose Rust trait methods under different names in Incan. Use a dictionary for `methods`:

```incan
@rust.delegate(
    trait=sqlx::Connection,
    methods={
        "connect": "establish",  # Rust: establish → Incan: connect
        "close": "shutdown",     # Rust: shutdown  → Incan: close
    },
)
type DbConnection = newtype PgConnection
```

**Parameter**:

- **`methods` (dict form)** - Maps Incan method names (keys) to Rust method names (values).
  The compiler generates methods with the Incan names that delegate to the Rust names.

**When to use**: The Rust trait uses naming conventions that conflict with Incan style or when avoiding reserved keywords.

> **Note**: Method renaming affects only the generated Incan-facing methods; the Rust trait is implemented with its
> original method names.

### Advanced: Associated Types

Some Rust traits have **associated types** - type placeholders that must be specified when implementing the trait. These
are different from generic type parameters: they're _output_ types that the implementer chooses, not _input_ types the
caller provides.

**Example: `Iterator` has an associated type `Item`**

```rust
trait Iterator {
    type Item;  // ← Associated type: "what does this iterator yield?"
    fn next(&mut self) -> Option<Self::Item>;
}

// When implementing, you must specify what Item is:
impl Iterator for MyRange {
    type Item = i32;  // ← "This iterator yields i32 values"
    fn next(&mut self) -> Option<i32> { /* ... */ }
}
```

**Why this matters:**

Associated types answer questions like:

- `Iterator::Item` - What does this iterator yield?
- `Future::Output` - What does this future resolve to?
- `FromStr::Err` - What error does parsing return?

They're part of the trait's contract - without specifying them, the implementation is incomplete.

**Why trait bridges need this information:**

When you delegate a trait with associated types, the compiler needs to know what to put in those "holes":

```incan
from my_crate import CustomIterator, MyItem   # custom types defined in user code

@rust.delegate(
    trait=std::iter::Iterator,
    associated_types={
        "Item": MyItem,  # ← Symbolic type reference, validated by compiler
    },
)
type MyIter = newtype CustomIterator
```

**Generated Rust:**

```rust
impl Iterator for MyIter {
    type Item = MyItem;  // ← From your decorator
    
    fn next(&mut self) -> Option<MyItem> {
        self.0.next()  // Delegate to wrapped type
    }
}
```

Without the `associated_types` parameter, the compiler can't generate the `type Item = ...;` line, and Rust will reject
the incomplete impl.

**Parameter:**

- **`associated_types`** - Dict mapping associated type names (strings) to **type symbols** (not strings). The type
  values must be valid Incan or Rust types accessible in the current scope (either defined locally or imported). Each
  entry becomes a `type Name = Type;` declaration in the generated impl block. The wrapped type must implement the same
  trait with compatible associated types.

**Common traits with associated types:**

Not just `Iterator`! Many Rust traits use associated types:

- **`Iterator`** - `Item` (what you iterate over)
- **`Future`** - `Output` (what the future resolves to)
- **`FromStr`** - `Err` (what error parsing returns)
- **`Add`, `Sub`, `Mul`** - `Output` (result type of arithmetic)
- **`Deref`** - `Target` (what the smart pointer points to)
- **`Index`** - `Output` (type returned by indexing)
- **`TryFrom`** - `Error` (error type of conversion)

**When to use:**

Only when delegating traits that define associated types. Most common traits (`Display`, `Debug`, `Clone`, `Send`, `Sync`)
have no associated types and don't need this parameter.

**Associated type inference:**

When `associated_types` is omitted, the compiler attempts to **infer** associated types from the wrapped type's existing
trait impl:

- If the wrapped type implements the trait with concrete associated types, the compiler mirrors those types in the
  delegation impl
- If inference succeeds (wrapped type's impl is visible and concrete), no explicit `associated_types` needed
- If inference fails (wrapped type's impl is generic, conditional, or unavailable), the compiler errors and requires
  explicit specification

**Example**: When inference works

```incan
from rust::std::iter import Iterator
from rust::std::vec import IntoIter  # IntoIter<T> implements Iterator with Item = T

@rust.delegate(trait=Iterator)
type MyIter[T] = newtype IntoIter[T]
# Compiler infers: type Item = T (from IntoIter's impl)
```

**Example**: When explicit specification is required

```incan
from rust::std::iter import Iterator
from my_crate import CustomIter  # Opaque type, impl not visible to Incan

@rust.delegate(
    trait=Iterator,
    associated_types={"Item": int},  # Required: compiler can't infer
)
type MyIter = newtype CustomIter
```

**Rule of thumb**: Try omitting `associated_types` first. If the compiler errors with "missing associated type", add
explicit specification.

**Why symbols instead of strings:**

Like `trait=` parameters, type values are **symbolic references** (they must exist in the namespace).
Built-in types like `int`, `str`, `bool` work without imports. The LSP autocompletes available types as you type.

For example:

```incan
# ✅ Symbols - validated, refactoring-safe
from rust::tokio import JoinHandle
import std::result as result

@rust.delegate(
    trait=std::future::Future,
    associated_types={"Output": result::Result[(), str]},
)
type Task = newtype JoinHandle

# ❌ Strings - unvalidated, breaks silently
@rust.delegate(
    trait=std::future::Future,
    associated_types={"Output": "Result<(), String>"},  # Typo won't be caught
)
type Task = newtype JoinHandle
```

### Advanced: Async Traits

Rust's async traits require special handling for delegation. The decorator automatically handles this through **trait
introspection** — the compiler reads the trait definition and extracts method signatures, generic parameters, and
constraints.

**Example: Delegating `FromRequestParts`**

```incan
import rust::axum::extract as extract

@rust.delegate(trait=extract::FromRequestParts)
type CustomExtractor[T] = newtype ExternalExtractor[T]
```

**What the compiler does automatically:**

1. **Introspects the trait definition** from Rust metadata (via rust-analyzer LSP — see [Rust Trait Introspection](#4-rust-trait-introspection))
2. **Discovers generic parameters**—reads `trait FromRequestParts<S>` and discovers `<S>` automatically
3. **Extracts method signatures**—parameter names, types, return types
4. **Identifies async methods**—methods returning `impl Future<...>`
5. **Preserves trait bounds**—carries over where clauses from the trait definition

**Generated Rust:**

```rust
// Pseudo-Rust: simplified for illustration (actual codegen would include async_trait desugaring)
impl<T, S> axum::extract::FromRequestParts<S> for CustomExtractor<T>
where
    T: DeserializeOwned,
    S: Send + Sync,  // From trait definition's bounds
    ExternalExtractor<T>: axum::extract::FromRequestParts<S>,
{
    type Rejection = <ExternalExtractor<T> as FromRequestParts<S>>::Rejection;
    
    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,   // Introspected from trait
        state: &S,                                // Introspected from trait
    ) -> Result<Self, Self::Rejection> {
        ExternalExtractor::<T>::from_request_parts(parts, state)
            .await
            .map(Self)
    }
}
```

**No manual parameters needed:**

The compiler discovers **all** trait metadata automatically—generic parameters, method signatures, async detection,
bounds, and associated types. You only specify the trait symbol:

```incan
@rust.delegate(trait=extract::FromRequestParts)
type CustomExtractor[T] = newtype ExternalExtractor[T]
```

The compiler reads `trait FromRequestParts<S>` and discovers that `<S>` exists, just like it discovers method parameters
`(parts: &mut Parts, state: &S)`.

**Common patterns:**

```incan
# Database connection
@rust.delegate(trait=sqlx::Executor)
type Pool = newtype PgPool

# Async extractors (compiler discovers <S> from trait definition)
@rust.delegate(trait=extract::FromRequestParts)
type Query[T] = newtype AxumQuery[T]

# Futures (compiler infers Output from wrapped type's impl)
@rust.delegate(trait=std::future::Future)
type Task[T] = newtype JoinHandle[T]  # Infers: type Output = T
```

### Decorator Parameters Reference

<!-- markdownlint-disable MD013 -->

| Parameter          | Type              | Description                                                           | Example                                             |
|--------------------|-------------------|-----------------------------------------------------------------------|-----------------------------------------------------|
| `trait`            | Symbol            | Single trait to delegate                                              | `trait=sqlx::Executor`                              |
| `traits`           | List[Symbol]      | Multiple traits                                                       | `traits=[TraitA, TraitB]`                           |
| `methods`          | List[str] or Dict | Methods to delegate (default: all)                                    | `methods=["execute"]` or `{"connect": "establish"}` |
| `associated_types` | Dict[str, Symbol] | Associated type mappings (when trait requires explicit specification) | `{"Item": int}` or `{"Item": MyType}`               |

<!-- markdownlint-enable MD013 -->

## Comparison: Hardcoded vs User-Defined

### Hardcoded (v0.2 Stdlib Approach)

**Definition** (in `crates/incan_core/src/lang/trait_bridges.rs`):

```rust
TraitBridge {
    applies_to_type_path: "axum::extract::",
    trait_path: "axum::extract::FromRequestParts",
    // ...
}
```

**Usage**:

```incan
type Query[T] = newtype AxumQuery[T]  # Pattern matches automatically
```

**Pros**: Automatic, zero boilerplate  
**Cons**: Only stdlib, users can't extend

### User-Defined (This RFC)

**Definition**:

```incan
@rust.delegate(trait=sqlx::Executor, methods=["execute"])
type Pool = newtype PgPool
```

**Pros**: User-extensible, explicit, grep-able  
**Cons**: More boilerplate for custom cases

## LSP Support

The Language Server Protocol integration is critical for usability. The LSP provides:

### Autocomplete

- **`trait=` parameter** - suggests available imported traits
- **`associated_types` keys** - when trait is set, suggests trait's required associated type names
- **`associated_types` values** - suggests types in scope (imported, built-in, or locally defined)

### Hover Information

Hovering over `trait=` shows the trait signature:

```incan
@rust.delegate(
    trait=std::iter::Iterator,  # ← Hover shows trait definition
    associated_types={"Item": int},
)
type MyIter = newtype CustomIterator
```

Displays:

```rust
trait Iterator {
    type Item;
    fn next(&mut self) -> Option<Self::Item>;
}
```

### Diagnostics

- **Missing required associated types** - "Iterator requires associated type 'Item'"
- **Unknown associated type names** - "Trait Iterator has no associated type 'Output'"
- **Invalid trait symbols** - "Trait 'Foo' not found in scope"
- **Missing trait import** - "Trait must be imported to use in decorator"

### Signature Help

As you type decorator parameters, shows available parameters and their types:

```incan
@rust.delegate(|  # ← Shows: trait=Symbol, traits=List[Symbol], methods=...
```

This makes the decorator self-documenting without referring to external Rust documentation.

## Examples

### Example 1: Database Connection Pool

```incan
"""Custom async database wrapper"""
import rust::sqlx as sqlx
from rust::sqlx @ "0.7" import PgPool

@rust.delegate(
    traits=[
        sqlx::Executor,
        sqlx::PgExecutor,
    ],
)
type Pool = newtype PgPool

async def get_users(pool: Pool) -> List[User]:
    # pool.execute() works because Executor is delegated!
    rows = await pool.fetch_all("SELECT * FROM users")
    return [User.from_row(r) for r in rows]
```

### Example 2: Custom Iterator

```incan
from rust::my_crate @ "1.0" import RangeIter as RustRange

@rust.delegate(
    trait=std::iter::Iterator,
    associated_types={"Item": int},  # Symbolic type reference
)
type Range = newtype RustRange

# Now works in for loops!
for x in Range.new(0, 10):
    println(x)
```

### Example 3: Error Handling

```incan
from rust::anyhow @ "1.0" import Error as AnyhowError

@rust.delegate(
    traits=[
        std::error::Error,
        std::fmt::Display,
        std::fmt::Debug,
    ],
)
type AppError = newtype AnyhowError

def risky() -> Result[Data, AppError]:
    return Err(AppError.msg("Something failed"))  # ? operator works!
```

### Example 4: Method Renaming

```incan
"""
Hypothetical: demonstrating method renaming with a custom HTTP trait.
(Note: reqwest::Client is a struct in Rust; this assumes a fictional HttpClient trait for illustration)
"""
from rust::my_http_lib @ "1.0" import Client as LibClient

@rust.delegate(
    trait=my_http_lib::HttpClient,   # Fictional trait for demonstration
    methods={
        "get": "http_get",           # Expose http_get as get
        "post": "http_post",         # Expose http_post as post
        "execute": "send_request",   # Rename for clarity
    },
)
type HttpClient = newtype LibClient

async def fetch(client: HttpClient, url: str) -> str:
    response = await client.get(url).execute()
    return await response.text()
```

> **Note**: This example uses a hypothetical `HttpClient` trait for illustration. In practice, HTTP client libraries
> like `reqwest` provide structs (not traits), so delegation would apply to traits those structs implement
> (e.g., `Clone`, `Debug`, or `tower::Service`).

### Example 5: Disambiguating Method Collisions

```incan
"""Wrapping a type that implements multiple Executor traits"""
import rust::sqlx as sqlx
import rust::custom_db as custom

@rust.delegate(
    trait=sqlx::Executor,
    methods={
        "execute_sql": "execute",      # Rename to avoid collision
        "fetch_one": "fetch_one",      # No collision, keep original
    }
)
@rust.delegate(
    trait=custom::Executor,
    methods={
        "execute_custom": "execute",   # Rename to avoid collision
        "batch_execute": "batch_execute",
    }
)
type HybridPool = newtype CustomPgPool

async def run_queries(pool: HybridPool):
    # Both executors available under different names
    await pool.execute_sql("SELECT * FROM users")
    await pool.execute_custom(custom_query)
```

## Why `@rust.delegate` Syntax?

### Namespacing

The `@rust.*` prefix clearly marks Rust interop:

- `@rust.delegate` - Trait delegation
- `@rust.unsafe` (future) - Unsafe operations
- `@rust.ffi` (future) - C FFI bindings
- `@rust.inline` (future) - Force inline

vs Incan-native decorators:

- `@derive` - Incan codegen
- `@test` - Test framework
- `@fixture` - Test fixtures

### Symbolic Arguments

Use `trait=sqlx::Executor` (symbol) not `trait="sqlx::Executor"` (string):

**Benefits**:

1. **Typechecker validation** - trait path must resolve
2. **IDE autocomplete** - works on trait names
3. **Refactoring safe** - rename tracking
4. **Import enforcement** - compiler ensures import exists

> **Important**: The trait symbol **must be imported** for the decorator to work. This is by design—it forces users to
> ensure proper Rust imports are in place, making the underlying dependencies explicit and verified.

**Comparison**:

```incan
# ✅ GOOD - Symbol imported and used
import rust::sqlx as sqlx

@rust.delegate(trait=sqlx::Executor)
type Pool = newtype PgPool

# ❌ BAD - String, unvalidated, no import verification
@rust.delegate(trait="sqlx::Executor")
type Pool = newtype PgPool
```

## Design Decisions

### 1. Multiple Decorator Handling

**Decision**: Multiple `@rust.delegate` decorators on the same type ARE allowed, with collision detection.

```incan
# ✅ ALLOWED - no method name collisions
@rust.delegate(trait=sqlx::Executor)
@rust.delegate(trait=std::fmt::Debug)
type Pool = newtype PgPool
```

**Method name collision rule**:

If two decorators delegate methods with the same final Incan name (after renaming), the compiler **must reject**:

```incan
# ❌ ERROR - both expose 'execute'
@rust.delegate(trait=sqlx::Executor, methods=["execute"])
@rust.delegate(trait=custom::Executor, methods=["execute"])
type Pool = newtype PgPool
# Error: Method 'execute' delegated by multiple decorators (sqlx::Executor, custom::Executor)
# Hint: Use method renaming to resolve the conflict
```

**Disambiguation via renaming**:

```incan
# ✅ VALID - renamed to avoid collision
@rust.delegate(trait=sqlx::Executor, methods={"execute_sql": "execute"})
@rust.delegate(trait=custom::Executor, methods={"execute_custom": "execute"})
type Pool = newtype PgPool
```

**Rationale**:

- **Flexibility**: Allows incremental trait addition and per-trait method selection
- **Clarity**: Each decorator states its trait and method subset independently
- **Simplicity**: Collision detection is straightforward (check final method names across all decorators)

**Alternative single-decorator form**:

For convenience, `traits=[...]` in a single decorator is still supported when no renaming is needed:

```incan
# Equivalent to three separate decorators (when no collisions)
@rust.delegate(traits=[TraitA, TraitB, TraitC])
type T = newtype Wrapped
```

**Method name conflicts within a single decorator**:

When using `traits=[...]`, if multiple traits define the same method name, the compiler rejects with an error:

```incan
@rust.delegate(traits=[sqlx::Executor, custom::Executor])
type Pool = newtype PgPool
# Error: Method 'execute' appears in multiple traits: sqlx::Executor, custom::Executor
# Hint: Use separate decorators with method renaming to disambiguate
```

### 2. Default Delegation Strategy

**Decision**: When `methods` is omitted, delegate **all trait methods** by default.

```incan
@rust.delegate(trait=Executor)  # Delegates ALL Executor methods
```

**Rationale**:

- Most common use case is full trait delegation (newtypes as transparent wrappers)
- Explicit `methods=["execute", "fetch"]` available for subsets
- No need for redundant `methods="all"` parameter

**For subsets**, use explicit list:

```incan
@rust.delegate(trait=Executor, methods=["execute", "fetch"])  # Only these two
```

### 3. Limitations: Auto Traits

**Auto traits (`Send`, `Sync`, `Unpin`, `UnwindSafe`, `RefUnwindSafe`) are not supported by `@rust.delegate`.**

These traits are automatically inferred by Rust's compiler based on type composition and cannot be implemented manually.
The newtype automatically inherits these traits if the wrapped type has them—no delegation needed.

**What happens automatically:**

```incan
from rust::std::sync import Arc

# Arc<T> is Send + Sync when T is Send + Sync
type SharedData[T] = newtype Arc[T]

# SharedData[T] automatically gets Send + Sync - no decorator needed!
# Rust infers this based on Arc's auto trait impls
```

**What's rejected:**

```incan
# ❌ ERROR - auto traits cannot be explicitly delegated
@rust.delegate(trait=std::marker::Send)
type MyWrapper = newtype SomeType
# Error: Cannot delegate auto trait 'std::marker::Send'
# Note: Auto traits (Send, Sync, Unpin, etc.) are inferred automatically by Rust
```

**Why this limitation exists:**

Auto traits are special in Rust's type system — they're implemented automatically based on the types a struct contains.
You cannot write `impl Send for MyType` manually. The newtype wrapper inherits auto traits from its wrapped type
automatically, so explicit delegation is both unnecessary and impossible.

**Common auto traits:**

- `Send` - Type can be transferred across thread boundaries
- `Sync` - Type can be shared between threads (via `&T`)
- `Unpin` - Type can be moved even when pinned
- `UnwindSafe` / `RefUnwindSafe` - Type is safe across panic unwinding

If you need to control these traits, you must do so at the type definition level (e.g., wrapping in `!Send` types), not
through delegation.

### 4. Rust Trait Introspection

**How does the compiler access Rust trait definitions?**

**Approach:** Leverage rust-analyzer via LSP

Query rust-analyzer through the Language Server Protocol for trait metadata. This approach piggybacks on Incan's
existing rust-analyzer dependency (required for `rust::` imports per [RFC 005]).

**Why rust-analyzer:**

1. **Already required** - Incan's LSP uses rust-analyzer for `rust::` import resolution, type checking, and autocomplete
2. **Solves hard problems** - Expands proc macros, resolves cargo features, handles GATs and const generics
3. **Stable protocol** - LSP is versioned and stable (unlike rustc internals or rustdoc JSON)
4. **No custom parser maintenance** - rust-analyzer handles Rust syntax evolution; we consume stable APIs
5. **IDE integration** - Same service powers editor features (hover, autocomplete)

**What rust-analyzer provides:**

```rust
// Pseudo-code: trait information from rust-analyzer LSP
TraitInfo {
    name: "Executor",
    generics: [TypeParam("S"), TypeParam("T"), ...],
    methods: [
        Method { name: "execute", is_async: true, params: [...], return_type: ... },
        Method { name: "fetch_one", ... },
    ],
    associated_types: [
        AssocType { name: "Item", bounds: [...], default: None },
    ],
    supertraits: [Executor, Send, Sync],
    where_clauses: [...],
}
```

**Key capabilities:**

1. **Proc macro expansion** - Sees traits generated by `#[async_trait]`, `#[derive]`, etc.
2. **Cargo features resolution** - Knows which features are enabled in current build
3. **Trait impl checking** - Verifies wrapped type actually implements the trait
4. **Cross-crate resolution** - Follows trait definitions through dependencies
5. **GATs and const generics** - Full support for advanced Rust type features

**How it works:**

```rust
// Pseudo-code: LSP query during compilation
let client = LspClient::new("rust-analyzer");
let trait_info = client.query_trait("sqlx::Executor")?;

// Generate delegation impl using trait metadata
for method in trait_info.methods {
    generate_delegated_method(method);
}
```

**Caching:**

- Queried trait metadata cached in `target/incan/trait-cache/`
- Keyed by (crate_name, version, trait_path, **content_hash**)
- Content hash computed from trait definition source (transitively including supertraits)
- Automatically invalidated when trait definition changes (hash mismatch), regardless of cause
- Resilient to edge cases: manual edits, git branch switches, dependency updates
- Reduces LSP queries on incremental builds

**Fallback for CI/offline environments:**

When rust-analyzer is unavailable (CI, offline builds, cross-compilation):

1. Use cached metadata from previous builds (if available)
2. Defer trait validation to rustc during final Rust compilation
3. rustc errors map back to Incan source locations (e.g., "trait `Executor` not implemented for `PgPool`")

This ensures Incan works in all environments while preferring rich metadata when available.

**Introspection capabilities comparison:**

We considered using `syn` for this parsing model, but unfortunately it lacks the features to make that a viable option.
This table shows `syn` compared to the `rust-analyzer` approach:

| Edge Case                  | rust-analyzer | syn-only   |
| -------------------------- | ------------- | ---------- |
| Proc macro traits          | ✅ Yes        | ❌ No      |
| Cargo features             | ✅ Yes        | ❌ No      |
| Trait impl checking        | ✅ Yes        | ❌ No      |
| GATs (Generic Assoc Types) | ✅ Yes        | ⚠️ Partial |
| Const generics             | ✅ Yes        | ⚠️ Partial |
| Target-specific impls      | ✅ Yes        | ❌ No      |
| Supertrait resolution      | ✅ Yes        | ✅ Yes     |
| Async trait detection      | ✅ Yes        | ⚠️ Partial |

**Compiler responsibilities:**

The compiler still handles Incan-specific delegation logic:

1. **Generic substitution** - Maps Incan type parameters to Rust generics
2. **Method conflicts** - Detects name collisions in multi-trait delegation
3. **Associated type inference** - Validates `associated_types=` matches trait definition
4. **Error translation** - Converts rust-analyzer/rustc errors to Incan diagnostics

## Migration Path

### Stdlib Migration (Required for RFC Completion)

Currently, stdlib uses hardcoded trait bridges:

```incan
# stdlib/web/request.incn - current
type Query[T] = newtype AxumQuery[T]  # Auto-delegates via hardcoded pattern
```

Stdlib must converge to explicit decorators for trait delegation parity:

```incan
# stdlib/web/request.incn - with decorator
@rust.delegate(trait=axum::extract::FromRequestParts)
type Query[T] = newtype AxumQuery[T]
```

**Benefits of migration:**

- Makes trait delegation explicit and grep-able
- Same mechanism for stdlib and user code
- Better documentation (hover shows what traits are implemented)

**Transitional allowance:**

- Existing hardcoded bridges may remain temporarily while stdlib modules are migrated
- New trait delegation behavior should be added through `@rust.delegate`, not new hardcoded bridge entries
- Hardcoded bridges are removed (or reduced to compatibility shims) once equivalent decorator-based coverage exists

## Acceptance Criteria

1. **Parity**: Any trait delegation behavior used by `std.web` (or other stdlib modules) is expressible with
    `@rust.delegate` in user code.
2. **No privileged path for new features**: New delegation features are implemented in the decorator pipeline, not via
    stdlib-only hardcoded bridges.
3. **Stdlib convergence**: Core stdlib newtype delegations are represented via explicit decorators (or generated from an
    equivalent decorator model), with hardcoded behavior retained only as temporary compatibility scaffolding.
4. **User validation**: At least one non-stdlib integration example (e.g., `sqlx`, `tower`, or equivalent) demonstrates
    that user libraries can achieve the same delegation outcomes without handwritten Rust wrappers.

## Alternatives Considered

- **Hardcoded bridges only** - Keep trait bridges internal to stdlib.
  Rejected: forces users to wait or write Rust wrappers.
- **Build stdlib in Rust** - Write `std.web` as Rust wrappers instead of Incan.
  Rejected: defeats Incan's purpose as a high-level language.
- **String-based trait names** - Use `trait="sqlx::Executor"` instead of symbols.
  Rejected: no validation, no IDE support, breaks refactoring.

## Success Metrics

1. **Adoption**: 50%+ of external Rust wrapper newtypes use `@rust.delegate` in community packages
2. **Reduction in manual Rust code**: 75% less hand-written delegation glue
3. **Parity achieved**: No net-new stdlib-only trait bridge rules are introduced after RFC adoption
4. **Documentation**: Clear examples in Incan Book chapter on Rust interop
5. **Performance**: Zero runtime overhead (compile-time only)

## References

- RFC 005: Rust Interop (`rust::` import syntax)
- RFC 021: Field Metadata & Aliases (similar decorator pattern)
- Trait bridges implementation: `src/backend/ir/emit/decls/structures.rs`
- Trait bridge registry: `crates/incan_core/src/lang/trait_bridges.rs`

## Future Extensions

### 1. `@rust.unsafe` Decorator

```incan
@rust.unsafe
def raw_pointer_deref(ptr: *const i32) -> i32:
    return *ptr  # Allowed in unsafe block
```

### 2. `@rust.inline` Hint

```incan
@rust.inline(always)
def hot_path(x: int) -> int:
    return x * 2
```

### 3. `@rust.ffi` for C Bindings

```incan
@rust.ffi(lib="mylib", symbol="compute")
def native_compute(x: float) -> float: ...
```

### 4. Full `impl` Blocks

```incan
impl CustomTrait for MyType:
    fn custom_method(self) -> int:
        # Full method body in Incan
        return 42
```

This RFC focuses on delegation; full impl blocks are a separate (larger) feature.

---

## Appendix: Current Trait Bridge Registry

For reference, stdlib's hardcoded bridges:

```rust
// crates/incan_core/src/lang/trait_bridges.rs
pub const TRAIT_BRIDGES: &[TraitBridge] = &[
    // Display: println!("{}", x)
    TraitBridge {
        applies_to_type_path: "",  // Matches all
        trait_path: "std::fmt::Display",
        // ...
    },
    
    // FromRequestParts: Axum extractors
    TraitBridge {
        applies_to_type_path: "axum::extract::",
        trait_path: "axum::extract::FromRequestParts",
        // ...
    },
    
    // Iterator: for x in iter
    TraitBridge {
        applies_to_type_path: "",
        trait_path: "std::iter::Iterator",
        // ...
    },
    
    // IntoResponse: Axum responses
    TraitBridge {
        applies_to_type_path: "axum::response::",
        trait_path: "axum::response::IntoResponse",
        // ...
    },
];
```

This RFC proposes making this extensible via `@rust.delegate`.

--8<-- "_snippets/rfcs_refs.md"
