# RFC 026: User-Defined Trait Bridges

> **Withdrawn.** The `@rust.delegate` mechanism described in this RFC is **not** planned. Trait forwarding and Rust-side contracts for wrapped types are owned by RFC 043: Rust trait implementation from Incan (`impl` on `rusttype`, including body-less forwarding where the backing type already implements the trait; `@rust.derive` for Rust proc-macro derives; compiler-managed `Future` bridging). This document remains an archival record of the problem statement and the rejected decorator-centric design.

- **Status:** Superseded
- **Superseded by:** RFC 043
- **Created:** 2026-02-19
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 043 (**superseding** — Rust trait `impl` from Incan, `@rust.derive`, async bridging)
    - RFC 005 (Rust interop — foundation for `rust::` imports)
    - RFC 023 (Compilable stdlib and `rust.module` binding — derive-based web delegation today)
    - RFC 021 (Field metadata and aliases — similar decorator pattern)
    - RFC 024 (Extensible derive protocol — how `@derive` maps to Rust proc macros today)
    - RFC 041 (First-class Rust interop — `rusttype`, metadata dispatch, coercions; **implemented**)
- **Issue:** #152
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

RFC 023 introduced stdlib patterns where newtypes over external Rust types can gain trait implementations through **`@derive`** wired to the `incan_web_macros` crate and `rust.module(...)` metadata; that path is limited to what the stdlib exposes and does not generalize to arbitrary crates. This RFC proposes **`@rust.delegate`**, a user-facing decorator that generates forwarding `impl` blocks for any imported Rust trait the wrapped type already implements, with explicit method selection, renaming, and associated-type control, so newtypes stay transparent outside stdlib-curated derive hooks.

## Motivation

### What is a Trait Bridge?

A **trait bridge** solves the newtype transparency problem: when you wrap an external Rust type in a newtype, you lose access to its traits unless you add an explicit `impl` that forwards to the inner value. For example, if you introduced an Incan `newtype` around Axum's query extractor type, that wrapper would not implement `FromRequestParts` until the compiler generated a forwarding impl (or you wrote one by hand).

The following illustrates the kind of Rust delegation this RFC aims to automate (pseudo-Rust; real output may use `async_trait` or equivalent desugaring):

```rust
// Pseudo-Rust: simplified for illustration (actual codegen would include async_trait desugaring)
impl<T, S> FromRequestParts<S> for MyQuery<T>
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

- **Type-safe** - preserves generic constraints, associated types, and where clauses carried from the trait definition
- **Transparent** - unwraps the newtype (`self.0`), delegates to the wrapped type, re-wraps results as needed
- **Explicit at the Incan layer** - under this RFC, authors opt in via `@rust.delegate` (and today, for a narrow stdlib path, via `@derive` on specific traits; see [Current State](#current-state))

**Why we need this:** Incan newtypes carry semantic intent, but Rust does not forward arbitrary trait implementations through wrappers. Bridges restore interoperability with Rust APIs that bound on traits.

### Current State

**Stdlib web (`std.web`):** Delegation for some newtypes is implemented today using **`@derive`** with trait names defined in `std.web.macros` and backed by proc macros in the `incan_web_macros` crate. The compiler resolves each derive name to a Rust module path using `rust.module(...)` metadata on the stdlib module and emits `#[derive(...)]` on the tuple struct when lowering and emitting (see RFC 024). For example, `std.web.response` defines a `newtype` over Axum's raw response type with `@derive(IntoResponse)` so the proc macro generates an `IntoResponse` impl that delegates to the inner type.

**`std.web.request`:** `Query` and `Path` are **direct re-exports** of Axum extractor types from `rust::axum::extract`; they are not Incan `newtype` aliases. Those Axum types already implement `FromRequestParts`, so no Incan-side derive or bridge is applied to them in the stdlib sources.

**User code:** Authors cannot today attach arbitrary third-party Rust traits (e.g. `sqlx::Executor`) to their own newtypes through a single documented decorator; extending delegation requires new stdlib-visible derive hooks or handwritten Rust. **`@rust.delegate`** (this RFC) is the proposed general mechanism.

### The Problem

With RFC 041, authors often use **`rusttype`** (or `newtype`) over `PgPool` and can **call methods** in Incan. The pain returns when **Rust** APIs require **`Pool: sqlx::Executor`** (or similar): the generated wrapper type does not implement the trait just because the inner type does.

Users **cannot** today define their own **forwarding** trait impls from Incan without one of:

```incan title="user_lib/database.incn"
from rust::sqlx @ "0.7" import PgPool

type Pool = newtype PgPool
# ❌ No Incan-native forwarding impl for traits the inner type already implements
```

1. Handwritten Rust `impl` (or a proc-macro crate)
2. Sticking with the raw imported type at those boundaries
3. Waiting for stdlib/curated `@derive` support for that specific trait

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

The `@rust.delegate` decorator tells the compiler to generate a Rust `impl sqlx::Executor for Pool` that forwards all trait methods to the wrapped `PgPool`. This preserves Rust's trait-based polymorphism: code expecting an `Executor` will accept `Pool` because it implements the trait, just like the wrapped type does.

## Goals

- Allow authors to **forward** existing Rust trait implementations through Incan `newtype` (and, where applicable, `rusttype`-lowered wrappers) without maintaining a separate proc-macro crate for every trait, when the inner Rust type already implements the trait.
- Support method subsetting, associated type specification, multi-trait delegation with collision rules, and async traits; full rust-analyzer-driven introspection (Design Decisions §4) is a **scaling** goal, not a prerequisite for a useful MVP (see [Delivery phases (if pursued)](#delivery-phases-if-pursued)).
- **Parity (soft):** stdlib should *be able* to express the same forwarding semantics as user code; **mandatory migration** of already-working `@derive(IntoResponse)` / `incan_web_macros` to `@rust.delegate` is **not** a high-value goal on its own—it is churn with little author benefit unless it removes real maintenance cost.

## Strategic note (historical)

This section was written while RFC 026 and RFC 043 were framed as **complementary**. **RFC 043 subsumed this RFC** — see [RFC 043: Rust trait implementation from Incan](../../043_rust_trait_impl_from_incan.md) (§ “Supersedes RFC 026”). The **problem statement** in the Motivation sections remains valid: `rusttype` and `newtype` wrappers do not automatically implement traits implemented by the inner Rust type. The **`@rust.delegate`** design in the rest of this document is **withdrawn**; the adopted direction is **body-less `impl` on `rusttype`**, **`@rust.derive`**, and compiler-managed **`impl Future`**, as specified in RFC 043.

**Alternatives that were weighed against “full” RFC 026** (before consolidation):

- **Curated proc macros per trait** (status quo extended): add more derives or user-publishable proc-macro crates. **Benefit:** no compiler feature. **Cost:** N× crates, no single Incan-native spelling, weak IDE story unless each macro is documented.
- **Generalize RFC 024** so user code can attach **arbitrary** Rust proc-macro derives (not only stdlib `rust.module` traits). **Benefit:** reuse Rust ecosystem patterns. **Cost:** still proc-macro–centric; not symbolic `trait=` in Incan.
- **Supersede RFC 026 in favor of RFC 043** — **outcome:** forwarding and custom trait behavior are unified under `impl` / `@rust.derive` / async bridging in RFC 043 rather than adding `@rust.delegate`.

## Scope / Non-Goals

**This RFC covers:**

- Trait delegation for newtypes wrapping external Rust types
- Method subsetting and renaming via decorator parameters
- Associated type specification for traits that require them
- Multi-trait delegation with collision detection
- Automatic async trait handling via rust-analyzer introspection
- **Capability parity between stdlib and user code**: delegations used by stdlib modules must be expressible through the same `@rust.delegate` mechanism available to users

**This RFC does NOT cover:**

- **Manual `impl` blocks** - Writing full trait implementations with custom logic (see [Future Extensions](#future-extensions))
- **Custom delegation logic** - Delegation is pure forwarding; no custom code in delegated methods
- **Implementing unimplemented traits** - `rust.delegate` can only delegate traits the wrapped type already implements
- **Auto traits** - `Send`, `Sync`, `Unpin` are inferred by Rust automatically (see [Limitations: Auto Traits](#3-limitations-auto-traits))
- **Conditional delegation** - No `#[cfg]` or type-parameter-based conditional delegation
- **Cross-language bridges** - Only Rust trait delegation (no C FFI, Python protocols, etc.)
- **Runtime trait objects** - Delegation is compile-time only; no `dyn Trait` boxing
- **Permanent stdlib-only delegation paths** - stdlib-only derive or registry shortcuts are treated as migration scaffolding, not long-term architecture

This RFC establishes the **decorator surface** and **delegation semantics**. Implementation details may evolve, but the user-facing contract (decorator parameters, error messages, generated behavior) is the specification boundary.

## Design

### Core Syntax: `@rust.delegate`

> **Notation:** This RFC spells decorator arguments with keywords (e.g. `trait=sqlx::Executor`). Other documents may use shorthand such as `@rust.delegate(FromRequestParts)`; that should be read as the same intent as `@rust.delegate(trait=FromRequestParts)` unless a different syntax is explicitly standardized elsewhere (e.g. RFC 043).

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
  (see [Design Decisions: Default Delegation Strategy](#2-default-delegation-strategy)).
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

- **`traits`** - List of trait symbols to delegate. Generates multiple `impl` blocks (one per trait). All traits must be implemented on the wrapped type. If method names collide across traits, the compiler rejects with an error (see [Multiple Decorator Handling](#1-multiple-decorator-handling)).

**When to use**: Your newtype needs to preserve multiple unrelated trait implementations from the wrapped type (e.g., database traits + formatting traits).

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

Some Rust traits have **associated types** — type placeholders that must be specified when implementing the trait. These differ from generic type parameters: they are _output_ types that the implementer chooses, not _input_ types the caller supplies.

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

Without the `associated_types` parameter, the compiler cannot emit the `type Item = ...;` line, and Rust will reject the incomplete impl.

**Parameter:**

- **`associated_types`** — Dict mapping associated type names (strings) to **type symbols** (not strings). Values must be valid Incan or Rust types in scope (local or imported). Each entry becomes a `type Name = Type;` line in the generated impl. The wrapped type must implement the same trait with compatible associated types.

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

Only when delegating traits that define associated types. Most common traits (`Display`, `Debug`, `Clone`, `Send`, `Sync`) have no associated types and do not need this parameter.

**Associated type inference:**

When `associated_types` is omitted, the compiler attempts to **infer** associated types from the wrapped type's existing trait impl:

- If the wrapped type implements the trait with concrete associated types, the compiler mirrors those types in the delegation impl.
- If inference succeeds (the wrapped impl is visible and concrete), no explicit `associated_types` is required.
- If inference fails (generic, conditional, or unavailable impl), the compiler reports an error and requires explicit specification.

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

**Rule of thumb**: Try omitting `associated_types` first. If the compiler errors with "missing associated type", add explicit specification.

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

Rust's async traits require special handling for delegation. The decorator automatically handles this through **trait introspection** — the compiler reads the trait definition and extracts method signatures, generic parameters, and constraints.

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

The compiler discovers **all** trait metadata automatically—generic parameters, method signatures, async detection, bounds, and associated types. You only specify the trait symbol:

```incan
@rust.delegate(trait=extract::FromRequestParts)
type CustomExtractor[T] = newtype ExternalExtractor[T]
```

The compiler reads `trait FromRequestParts<S>` and discovers that `<S>` exists, just like it discovers method parameters `(parts: &mut Parts, state: &S)`.

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

| Parameter          | Type              | Description                                                           | Example                                             |
|--------------------|-------------------|-----------------------------------------------------------------------|-----------------------------------------------------|
| `trait`            | Symbol            | Single trait to delegate                                              | `trait=sqlx::Executor`                              |
| `traits`           | List[Symbol]      | Multiple traits                                                       | `traits=[TraitA, TraitB]`                           |
| `methods`          | List[str] or Dict | Methods to delegate (default: all)                                    | `methods=["execute"]` or `{"connect": "establish"}` |
| `associated_types` | Dict[str, Symbol] | Associated type mappings (when trait requires explicit specification) | `{"Item": int}` or `{"Item": MyType}`               |

## Comparison: Stdlib derive hooks vs `@rust.delegate`

### Stdlib `@derive` + proc macros (today)

**Mechanism:** Stdlib modules such as `std.web.macros` declare traits with `rust.module("incan_web_macros")` (or similar). User or stdlib code places `@derive(IntoResponse)` (or other supported trait names) on a `newtype`. The compiler resolves the trait name to a proc-macro path and emits `#[derive(...)]` on the generated Rust tuple struct; the proc macro expands to forwarding `impl` blocks.

**Example:**

```incan
from std.web.macros import IntoResponse

@derive(IntoResponse)
pub type Response = newtype AxumRawResponse:
    """HTTP response wrapper (see std.web.response in the stdlib)."""
```

**Pros:** Integrates with Rust's derive model for the curated trait set; minimal syntax at use sites.  
**Cons:** Each new trait requires stdlib metadata and a proc macro (or shared generator); user crates cannot add arbitrary traits through this path alone.

### User-defined (this RFC)

**Mechanism:** Authors attach `@rust.delegate` with symbolic trait references and optional method maps; the compiler generates the corresponding `impl` without requiring a new stdlib derive entry for every trait.

**Example:**

```incan
@rust.delegate(trait=sqlx::Executor, methods=["execute"])
type Pool = newtype PgPool
```

**Pros:** User-extensible, explicit, grep-able; same surface for stdlib migration targets.  
**Cons:** More surface area and compiler responsibility than a fixed derive allowlist.

## LSP Support

Language Server Protocol integration is critical for usability and provides the following behaviors:

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

> **Important**: The trait symbol **must be imported** for the decorator to work. This is by design—it forces users to ensure proper Rust imports are in place, making the underlying dependencies explicit and verified.

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

## Alternatives Considered

- **Stdlib-only derive hooks** — Keep delegation limited to curated `@derive` traits and proc macros. **Rejected** for *general* interop: forces users to wait or write Rust wrappers for traits outside that set; **accepted** as a *continuing* option for narrow high-traffic traits (web) where cost/benefit favors macros.
- **Extend RFC 024 only** — Let user crates register proc-macro derives without `@rust.delegate`. **Open:** may reduce need for compiler-generated `impl` if ecosystem standardizes on a few derive crates; **weakness:** still no symbolic trait reference in Incan typechecking.
- **RFC 043 `impl` blocks only** — Authors hand-write forwarding bodies in Incan for every method. **Viable** for small traits; **Rejected** as the *only* story: pure forwarding is boilerplate-heavy and error-prone compared to a one-line delegate declaration.
- **Abandon / supersede this RFC** — Declare forwarding out of scope for the compiler; use `rusttype`, shims, and proc macros. **Viable** if Phase 0 finds no blocked integrator; **cost:** Incan never standardizes “forward this trait through my wrapper.”
- **Build stdlib in Rust** — Write `std.web` as Rust wrappers instead of Incan. **Rejected:** defeats Incan's purpose as a high-level language.
- **String-based trait names** — Use `trait="sqlx::Executor"` instead of symbols. **Rejected:** no validation, no IDE support, breaks refactoring.

## Drawbacks

- **Compiler surface:** Parser, typechecker, IR, emission, and (if pursued) LSP all grow; maintenance competes with RFC 043 and derive-protocol work.
- **Duplication risk:** Forwarding via `@rust.delegate` overlaps with proc-macro derives; two ways to achieve similar Rust output unless one path is clearly preferred per scenario.
- **Async / metadata complexity:** Full trait introspection ties compilation to rust-analyzer (or equivalent); partial MVP avoids that but pushes friction to authors (explicit method lists) or to rustc error messages.
- **Orphan / coherence:** Rust’s orphan rules still constrain which `impl` blocks are legal; this RFC does not grant new coherence powers—only codegen within existing rules.

## Layers affected

Impacts are listed by compiler/tooling layer; this section is not an implementation task list.

- **Parser:** `@rust.delegate` syntax, keyword arguments, lists and maps for `methods` / `associated_types` / `traits`.
- **Typechecker:** Resolve trait and type symbols, enforce collision rules, validate that the wrapped Rust type can implement the delegated traits, associated-type consistency.
- **Lowering / IR:** Persist decorator metadata needed for Rust `impl` generation alongside existing `newtype` and `@derive` lowering.
- **Emission:** Generate forwarding `impl` blocks (and interact with today's `#[derive(...)]` emission for tuple structs where migration applies).
- **LSP:** Completions, hovers, and diagnostics for decorator parameters and trait metadata integration.
- **Formatter:** Stable formatting and round-trip for the new decorator.
- **Stdlib:** Migrate derive-based delegations that must remain semantically equivalent to explicit `@rust.delegate` (or an equivalent generated form) per acceptance criteria; keep temporary derive shims only while migration is in flight.

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

Auto traits are special in Rust's type system — they're implemented automatically based on the types a struct contains. You cannot write `impl Send for MyType` manually. The newtype wrapper inherits auto traits from its wrapped type automatically, so explicit delegation is both unnecessary and impossible.

**Common auto traits:**

- `Send` - Type can be transferred across thread boundaries
- `Sync` - Type can be shared between threads (via `&T`)
- `Unpin` - Type can be moved even when pinned
- `UnwindSafe` / `RefUnwindSafe` - Type is safe across panic unwinding

If you need to control these traits, you must do so at the type definition level (e.g., wrapping in `!Send` types), not through delegation.

### 4. Rust Trait Introspection

This subsection describes the **target** architecture for rich metadata. An MVP may ship **without** wiring the compiler to rust-analyzer for trait bodies (see [Delivery phases (if pursued)](#delivery-phases-if-pursued), Phases 1–3 vs 4).

**How does the compiler access Rust trait definitions?**

**Approach:** Leverage rust-analyzer via LSP.

Query rust-analyzer through the Language Server Protocol for trait metadata. This piggybacks on Incan's existing rust-analyzer dependency (required for `rust::` imports per RFC 005).

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

**How it works (illustrative pseudocode — not a prescribed compiler API):**

```rust
// Pseudo-code: LSP query during compilation
let client = LspClient::new("rust-analyzer");
let trait_info = client.query_trait("sqlx::Executor")?;

// Generate delegation impl using trait metadata
for method in trait_info.methods {
    generate_delegated_method(method);
}
```

**Caching (target architecture):**

- Implementations may persist queried trait metadata under a compiler-managed cache directory (exact path is not part of the user-facing contract).
- Cache keys should include crate identity, version or lock context, trait path, and a **content hash** of the trait definition (transitively including relevant supertraits).
- Entries invalidate when the hash no longer matches (dependency updates, branch switches, local edits).
- The cache is an optimization to reduce LSP traffic on incremental builds, not a semantic requirement.

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

## Delivery phases (if pursued)

This is **not** the formal In-Progress “Implementation Plan” checklist; it is a **critical-path** ordering for when the project chooses to invest. Skip or reorder after the [strategic note](#strategic-note-historical).

### Phase 0 — Decision

- Confirm there is a **concrete** integration (stdlib module or external library) blocked on forwarding, not satisfiable by `rusttype` + small Rust shim or an extra proc-macro crate within a week of work.
- If not, **park** or **supersede** this RFC rather than implementing Design Decisions §4 (LSP/rust-analyzer) first.

### Phase 1 — MVP (highest benefit / smallest slice)

- Parse and typecheck `@rust.delegate(trait=..., methods=[...])` on **`newtype`** (and define whether `rusttype` is in scope for v1).
- Emit a forwarding `impl` for **one** trait, **explicit** method list, **sync** methods first; associated types either explicit in the decorator or copied from a single, known-good pattern.
- Defer: method **renaming** maps, multi-trait collision policy, and “delegate all methods with zero list” default.
- Validate shapes primarily with **rustc** errors mapped to Incan spans; treat rust-analyzer metadata as **optional** optimization, not a gate.

### Phase 2 — Async and associated types

- Async methods: agreed strategy (`async_trait` in emitted code, or metadata-driven signature copy) without requiring full LSP trait AST in the first cut.
- Associated type inference only **after** explicit-path MVP is stable.

### Phase 3 — Ergonomics and collisions

- Method renaming, `traits=[...]` on one decorator, collision diagnostics as specified in Design Decisions.

### Phase 4 — Trait introspection (optional acceleration)

- rust-analyzer / LSP-backed trait metadata, caching as in Design Decisions §4—**only if** Phase 1–3 still leave too much manual listing or rustc noise.

### Phase 5 — Stdlib (lowest priority unless it pays rent)

- Replace `@derive(IntoResponse)` with `@rust.delegate` **only** if it deletes `incan_web_macros` maintenance or fixes a real bug; otherwise keep derives as compatibility shims indefinitely.

## Migration Path

### Stdlib migration (optional; not required for author-facing value)

**Today:** `std.web.request` re-exports Axum `Query` and `Path` directly; no Incan `newtype` or `@rust.delegate` is required for `FromRequestParts` on those types. `std.web.response` uses `@derive(IntoResponse)` on a `newtype` over Axum's raw response type, backed by `incan_web_macros`.

**Target:** Any stdlib pattern that relies on compiler-generated forwarding through newtypes must be expressible as `@rust.delegate` (or mechanically equivalent), so user code and stdlib share one model.

**Example (illustrative — if a future stdlib `newtype` wrapper around an Axum extractor were introduced):**

```incan
@rust.delegate(trait=axum::extract::FromRequestParts)
type Query[T] = newtype AxumQuery[T]:
    ...
```

**Benefits of convergence:**

- Delegation is explicit and grep-able in Incan sources
- Same mechanism for stdlib and user code
- Documentation and LSP can surface delegated traits consistently

**Transitional allowance:**

- Existing `@derive(...)`-based hooks may remain temporarily while stdlib modules migrate
- New delegation behavior should be added through `@rust.delegate`, not by expanding stdlib-only derive allowlists as the long-term approach
- Derive-based shims are removed (or reduced to compatibility-only) once equivalent `@rust.delegate` coverage exists

## Acceptance Criteria

1. **User value:** At least one **non-stdlib** integration demonstrates that a library author can satisfy a **foreign** trait bound on an Incan-defined wrapper using `@rust.delegate` (or documents an intentional subset, e.g. sync-only MVP) without adding a new Rust proc-macro crate solely for forwarding.
2. **No accidental privilege:** New **compiler** features for forwarding live in the `@rust.delegate` pipeline; expanding stdlib-only derive allowlists remains acceptable as a **stopgap** if migration is deferred (contrasts with earlier “no privileged path” absolutism—see [Goals](#goals)).
3. **Stdlib (conditional):** If stdlib migration is undertaken, it must be justified by **measurable** reduction in glue (e.g. fewer lines in `incan_web_macros` or fewer special cases in the compiler), not by symmetry alone.
4. **Honest scope:** The shipped feature set matches what was actually built (MVP vs full Design section); rust-analyzer integration is **not** required for “RFC met” if Phase 4 was explicitly deferred.

## Success Metrics

1. **Adoption:** 50%+ of external Rust wrapper newtypes use `@rust.delegate` in community packages
2. **Reduction in manual Rust code:** 75% less hand-written delegation glue
3. **Privilege discipline:** No *new* stdlib-only compiler shortcuts for forwarding are added **without** a plan to expose the same capability to users—or an explicit decision to keep a macro-only path (see [Delivery phases](#delivery-phases-if-pursued), Phase 5).
4. **Documentation:** Clear examples in Incan Book chapter on Rust interop
5. **Performance:** Zero runtime overhead (compile-time only)

## References

- RFC 005: Rust interop (`rust::` import syntax)
- RFC 021: Field metadata and aliases (similar decorator pattern)
- RFC 024: Extensible derive protocol (`@derive` mapping to Rust proc macros)
- Current tuple-struct emission (including `#[derive(...)]` attachment): compiler emission for structs / newtypes
- `incan_web_macros`: proc macros used by `std.web` for `IntoResponse`, `FromRequestParts`, etc.

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

## Appendix: Delegation as implemented today (pre-`@rust.delegate`)

There is **no** `TRAIT_BRIDGES` table or `trait_bridges.rs` registry in the Incan repository. Prior drafts of this RFC used that sketch to motivate the problem; the **actual** pre-RFC-026 path is:

1. **Stdlib** defines traits such as `IntoResponse` and `FromRequestParts` in `std.web.macros` with `rust.module(...)` pointing at `incan_web_macros`.
2. **Lowering** resolves `@derive(TraitName)` to that Rust module path (via stdlib AST cache / trait metadata).
3. **Emission** attaches `#[derive(qualified::TraitName)]` to the generated Rust tuple struct; **proc macros** in `incan_web_macros` expand to forwarding `impl` blocks.

**`std.web.request`** does not use this derive path for `Query` / `Path`; those names re-export Axum types that already implement Axum traits.

This RFC generalizes the *outcome* (forwarding trait impls for newtypes) with **`@rust.delegate`** so users are not limited to the stdlib's curated `@derive` traits.

--8<-- "_snippets/rfcs_refs.md"
