# RFC 043: Rust Trait Implementation from Incan

- **Status:** Draft
- **Created:** 2026-03-25
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 041 (First-class Rust interop authoring)
    - RFC 026 (User-defined trait bridges)
    - RFC 024 (Extensible derive protocol)
    - RFC 005 (Rust interop)
    - RFC 023 (Compilable stdlib & Rust module binding)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC proposes that Incan authors can declare that a type satisfies a Rust trait contract, and the compiler generates the corresponding `impl` block in the emitted Rust code. This closes the gap between RFC 041's "use Rust APIs from Incan" and the reverse need: "make Incan types usable by Rust APIs that require trait bounds." Concretely, it introduces `impl` blocks on `rusttype` declarations, a `@rust.derive` decorator for forwarding Rust derive macros, and compiler-managed async Future bridging for `rusttype` wrappers over `Future`-implementing Rust types.

## Core model

Read this RFC as one foundation plus three mechanisms:

**Foundation**: after RFC 041, imported Rust items are first-class compiler symbols and Incan types can wrap Rust types via `rusttype`. What is still missing is the ability to make those Incan types satisfy Rust trait contracts — the reverse direction of interop.

**Mechanisms**:

1. `impl` blocks on `rusttype` declarations let authors declare that a type satisfies a Rust trait by providing method bodies in Incan. The compiler generates the corresponding `impl Trait for Type { ... }` in emitted Rust.
2. `@rust.derive(...)` forwards Rust derive macros to the emitted struct, so Incan-authored models and newtypes can participate in Rust-ecosystem derive workflows (`Serialize`, `Deserialize`, `Clone`, etc.) without handwritten Rust.
3. Compiler-managed async bridging auto-generates `impl Future` for `rusttype` declarations that wrap `Future`-implementing Rust types, eliminating manual `Pin`/`Context`/`Poll` glue.

## Motivation

### RFC 041 solved half the interop story

RFC 041 made imported Rust items behave like ordinary Incan symbols: methods resolve, coercions insert, capability bounds lower to Rust predicates. Users can **call** Rust APIs from Incan without ceremony.

But Rust APIs are not just functions you call. They are also **contracts you implement**. DataFusion expects `impl ExecutionPlan`. Axum expects `impl FromRequestParts`. Tokio expects `impl Future`. Serde expects `impl Serialize`. Today, satisfying any of these contracts from Incan requires dropping into handwritten Rust — exactly the "bridge ceremony" that RFC 041 set out to eliminate.

### The stdlib proves the gap is real

The `std.async` stdlib is the clearest example. `JoinHandle` needs `impl Future` with a `poll` method that delegates to Tokio's handle. `TaskJoinError` needs `impl From<tokio::task::JoinError>`. `RuntimeFuture` and `RuntimeFnOnce` need blanket impls. Today these require handwritten Rust adapter modules that Incan library authors cannot write, modify, or understand without leaving the language.

The `std.async.sync` and `std.async.channel` modules carry hundreds of lines of Rust adapter code for the same reason: Incan types need to satisfy Rust trait contracts, and there is no way to express that from Incan source.

### Library authors hit this wall immediately

The moment a library author wants to:

- wrap a Rust type and make it `await`-able (needs `impl Future`)
- convert between error types across a Rust boundary (needs `impl From`)
- make an Incan model serializable for a Rust framework (needs `#[derive(Serialize)]`)
- implement a Rust extension point (needs `impl SomeTrait`)

they must create a parallel Rust adapter crate, maintain it separately, and wire it through `rust::` imports. This is the exact "parallel adapter layer" pattern that RFC 041's motivation section identifies as the problem.

### The goal is clear

Incan users should not have to write Rust to use Rust. RFC 041 achieved this for **consuming** Rust APIs. This RFC achieves it for **satisfying** Rust API contracts.

## Goals

- Let `rusttype` declarations include `impl` blocks that generate Rust `impl Trait for Type` code.
- Let `@rust.derive(...)` forward Rust derive macros to emitted structs and enums.
- Provide compiler-managed `impl Future` bridging for `rusttype` wrappers over `Future`-implementing Rust types.
- Enable the stdlib to express its current Rust adapter surface in Incan, reducing the Rust glue footprint.
- Keep the Incan surface Incan-shaped: authors write Incan method signatures and bodies, not raw Rust syntax.

## Non-Goals

- Inline Rust code blocks in Incan source. This RFC does not add an escape hatch for arbitrary Rust syntax.
- Orphan rule circumvention. Incan follows Rust's coherence rules: you can only implement a foreign trait for a type you own (defined in your crate), or implement your own trait for a foreign type.
- Runtime trait objects (`dyn Trait`). All trait impl generation is compile-time only.
- Generic `impl` blocks with complex where-clauses in the initial version. Start with concrete types; generic impls are a future extension.
- Replacing RFC 026's `@rust.delegate` mechanism. Delegation (forwarding an existing impl through a newtype) and implementation (writing new trait methods) are complementary; both may coexist.
- Blanket impls (`impl<F: FnOnce> RuntimeFuture for F`). These require generics over trait bounds and are deferred to a follow-up or extension of this RFC.

## Guide-level explanation (how users think about it)

### Implementing a Rust trait on a rusttype

The most common case: you wrap a Rust type and need to satisfy a trait contract.

```incan
from rust::tokio::task import JoinHandle as TokioJoinHandle
from rust::tokio::task import JoinError

type TaskJoinError = rusttype str:
    impl From[JoinError]:
        def from(error: JoinError) -> TaskJoinError:
            return TaskJoinError(error.to_string())
```

The compiler generates:

```rust
impl From<tokio::task::JoinError> for TaskJoinError {
    fn from(error: tokio::task::JoinError) -> Self {
        Self(error.to_string())
    }
}
```

The user writes Incan. The compiler writes Rust.

### Async Future bridging

For `rusttype` declarations that wrap a Rust type implementing `Future`, the compiler can auto-generate the `impl Future` delegation:

```incan
from rust::tokio::task import JoinHandle as TokioJoinHandle

type JoinHandle[T] = rusttype TokioJoinHandle[T]:
    impl Future:
        type Output = Result[T, TaskJoinError]

    def abort(self) -> None:
        ...
```

The `impl Future` block declares the associated type. The compiler generates the `poll` method by delegating to the backing type's `poll` implementation, mapping the output type through any declared interop edges (here, `JoinError` -> `TaskJoinError` via the `From` impl above).

Without this mechanism, the same code requires ~25 lines of handwritten Rust with `Pin`, `Context`, `Poll`, and manual error mapping.

### Forwarding Rust derive macros

For Incan models that need to participate in Rust derive workflows:

```incan
from rust::serde import Serialize, Deserialize

@rust.derive(Serialize, Deserialize, Clone)
model CustomerEvent:
    customer_id: str
    email: str
    amount: int
```

The compiler emits:

```rust
#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct CustomerEvent {
    pub customer_id: String,
    pub email: String,
    pub amount: i64,
}
```

`@rust.derive` is distinct from Incan's `@derive`: the former forwards to Rust derive macros, the latter uses the Incan derive protocol from RFC 024.

### Implementing arbitrary Rust traits

For extension points in Rust frameworks:

```incan
from rust::std::fmt import Display, Formatter, FmtError

type UserId = rusttype i64:
    impl Display:
        def fmt(self, f: Formatter) -> Result[None, FmtError]:
            return f.write_str(f"user_{self.0}")
```

The compiler generates `impl std::fmt::Display for UserId { ... }` with the method body lowered from the Incan source.

## Reference-level explanation (precise rules)

### `impl` block syntax

An `impl` block may only appear inside a `rusttype` declaration body:

```incan
type Name[Params...] = rusttype RustBacking[Params...]:
    impl TraitName[Args...]:
        type AssocType = IncanType
        def method_name(params...) -> ReturnType:
            body
```

Normative rules:

- `impl` blocks must appear inside `rusttype` declarations only. Ordinary `newtype` and `model` declarations must not contain `impl` blocks; the compiler must reject them with a clear diagnostic.
- The trait name must resolve to an imported Rust trait (via `rust::` or `std.rust`).
- Method signatures in the `impl` block must match the trait's method signatures as reported by Rust metadata. The compiler must validate parameter types, return types, and receiver shapes.
- Associated types declared with `type AssocType = T` must be compatible with the trait's associated type constraints.
- The compiler must generate a complete `impl Trait for Type { ... }` block in the emitted Rust, with method bodies lowered from the Incan source.
- When the backing Rust type already implements the trait and the `impl` block contains no method bodies, the compiler should delegate to the backing type's implementation (this overlaps with RFC 026's delegation but is expressed through `impl` syntax rather than a decorator).

### `@rust.derive` decorator

- `@rust.derive(Name1, Name2, ...)` is valid on `model`, `class`, `enum`, and `newtype` declarations.
- Each name must resolve to an imported Rust derive macro (via `rust::` imports or known standard derives like `Clone`, `Debug`).
- The compiler emits `#[derive(path::Name1, path::Name2, ...)]` on the generated Rust struct/enum.
- `@rust.derive` must not conflict with Incan's `@derive`: using both on the same declaration is valid when they target different derives. The compiler must reject duplicates that appear in both.
- Rust derive macros that require additional crate dependencies must have those dependencies declared in `incan.toml` `[rust-dependencies]`.

### Async Future bridging rules

- When a `rusttype` declaration includes `impl Future:` with an associated `type Output = T`, the compiler generates `impl Future for Type` with a `poll` method that delegates to the backing type.
- The generated `poll` must handle `Pin` projection correctly: the compiler must emit `Pin::new(&mut self.0).poll(cx)` (or the appropriate projection for the backing type's pinning requirements).
- Output type mapping: if the backing type's `Future::Output` differs from the declared `Output`, the compiler must insert the appropriate conversion (using `From` impls, `map`, or `map_err`) in the generated `poll` method.
- The compiler should verify (via Rust metadata when available) that the backing type actually implements `Future`. When metadata is unavailable, the compiler must accept the declaration and let rustc validate it.

### Interaction with existing features

#### RFC 026 (trait bridges / `@rust.delegate`)

RFC 026's `@rust.delegate` delegates an **existing** trait implementation through a newtype wrapper. This RFC's `impl` blocks **create new** trait implementations with Incan-authored method bodies. They are complementary:

- Use `@rust.delegate(FromRequestParts)` when the backing type already implements the trait and you want pure forwarding.
- Use `impl From[JoinError]: ...` when you need custom logic in the trait methods.

#### RFC 024 (extensible derive protocol)

RFC 024's `@derive` uses the Incan derive protocol (trait adoption, method injection). This RFC's `@rust.derive` forwards to Rust's `#[derive]` macro system. They serve different ecosystems and may coexist on the same type.

#### RFC 041 (first-class Rust interop)

This RFC is a direct extension of RFC 041. It uses the same `rusttype`, `rust::` import, and metadata infrastructure. `impl` blocks on `rusttype` are syntactically an extension of the `rusttype` body that RFC 041 defined for `interop:` edges and rebindings.

## Design details

### Syntax

New syntax additions:

- `impl TraitName[Args...]:` block inside `rusttype` body (indented, same level as `interop:`)
- `type AssocType = IncanType` inside an `impl` block (associated type declaration)
- `@rust.derive(Name, ...)` decorator on type declarations

### Semantics

The semantic center of this RFC is:

1. `impl` blocks on `rusttype` are Incan-authored, Rust-emitted trait implementations.
2. The compiler owns the translation: method signatures are validated against Rust metadata, bodies are lowered through the standard Incan pipeline, and the output is a valid Rust `impl` block.
3. `@rust.derive` is a passthrough: the compiler does not interpret the derive macro, it forwards it to rustc.
4. Async bridging is a specialization of `impl Future` that the compiler can optimize by delegating `poll` to the backing type.

### Compatibility / migration

Existing code continues to work unchanged. The new features are purely additive:

- Existing `rusttype` declarations without `impl` blocks remain valid.
- Existing Rust adapter modules in `incan_stdlib` can be incrementally migrated to Incan `impl` blocks.
- `@rust.delegate` from RFC 026 remains available for pure forwarding cases.

## Alternatives considered

- **Inline Rust blocks**  
    Some languages (e.g. Mojo, Zig) allow embedding target-language code directly. This was rejected because it breaks the "write Incan, not Rust" promise and makes tooling (formatter, LSP, diagnostics) much harder.

- **Automatic trait forwarding for all rusttype wrappers**  
    Instead of explicit `impl` blocks, the compiler could auto-forward all trait impls from the backing type. This was rejected because it would be unpredictable (users wouldn't know which traits their type satisfies), could cause coherence violations, and removes the author's ability to curate the type's API surface.

- **Extending `@rust.delegate` to cover custom implementations**
    RFC 026's decorator could be extended with method-body syntax. This was rejected because `@rust.delegate` has clear "pure forwarding" semantics; mixing in custom logic would confuse the mental model. `impl` blocks are a more natural syntax for "I am writing the implementation."

## Drawbacks

- The compiler must understand enough of Rust's trait system to validate `impl` block signatures. This is a significant increase in semantic complexity.
- `Pin` projection for `impl Future` is notoriously tricky in Rust. Generating correct `poll` delegation requires careful handling of pinning guarantees.
- Associated types, default methods, and supertraits in Rust traits create a large surface area for edge cases.
- Orphan rules may confuse users who expect to implement any trait for any type (Rust's coherence rules will reject some combinations that seem natural from Incan's perspective).

## Layers affected

- **Parser / AST**: `impl TraitName[Args...]:` blocks inside `rusttype` bodies; `type AssocType = T` declarations; `@rust.derive(...)` decorator parsing.
- **Typechecker / Symbol resolution**: validate `impl` method signatures against Rust trait metadata; check associated type compatibility; validate `@rust.derive` targets resolve to known Rust derive macros.
- **IR Lowering**: generate `impl`-block IR nodes from `impl` declarations; lower method bodies through the standard expression pipeline.
- **Emission**: emit `impl Trait for Type { ... }` blocks with correct method signatures, associated types, and `Pin` projection for `Future`; emit `#[derive(...)]` attributes for `@rust.derive`.
- **Stdlib / Runtime (`incan_stdlib`)**: migrate async task, time, sync, and channel Rust adapter modules to Incan `impl` blocks once the feature lands; shrink or remove handwritten Rust glue.
- **Formatter**: format `impl` blocks and `@rust.derive` decorators stably.
- **LSP / Tooling**: completions for trait method signatures inside `impl` blocks; diagnostics for signature mismatches.

## Unresolved questions

- Should `impl Future` for `rusttype` wrappers be automatic (compiler-managed when the backing type implements `Future`) or always explicit (user must write `impl Future:` with the associated type)?
- How do associated types in `impl` blocks interact with Incan's type system? Should they be full Incan type expressions or restricted to imported Rust types?
- Should `@rust.derive` require the derive macro's crate to be declared in `incan.toml` `[rust-dependencies]`, or should well-known standard derives (`Clone`, `Debug`, `Copy`) be allowed without explicit declarations?
- How does this interact with RFC 026's `@rust.delegate` when both are applied to the same trait on the same type? Should one take precedence, or should the compiler reject the combination?
- Should blanket impls (e.g. `impl<F: FnOnce> RuntimeFuture for F`) be expressible in a future extension of this RFC, or should they remain Rust-only?
- What diagnostics should the compiler produce when an `impl` block's method signature does not match the Rust trait's expected signature? Should it show the expected Rust signature alongside the Incan mismatch?
- How should `self` receiver types (`self`, `&self`, `&mut self`, `Pin<&mut Self>`) be expressed in Incan syntax inside `impl` blocks?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
