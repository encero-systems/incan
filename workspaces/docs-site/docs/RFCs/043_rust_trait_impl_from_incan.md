# RFC 043: Rust Trait Implementation from Incan

- **Status:** Draft
- **Created:** 2026-03-25
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 041 (First-class Rust interop authoring)
    - RFC 026 (Superseded — archival; `@rust.delegate` withdrawn in favor of this RFC)
    - RFC 024 (Extensible derive protocol)
    - RFC 005 (Rust interop)
    - RFC 023 (Compilable stdlib & Rust module binding)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/200
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC proposes that Incan authors can declare that a type satisfies a Rust trait contract, and the language implementation produces the corresponding `impl` block in emitted Rust. This closes the gap between RFC 041's "use Rust APIs from Incan" and the reverse need: "make Incan types usable by Rust APIs that require trait bounds." Concretely, it introduces `impl` blocks on `rusttype` declarations, a `@rust.derive` decorator for forwarding Rust derive macros, and implementation-managed async Future bridging for `rusttype` wrappers over `Future`-implementing Rust types. It **supersedes RFC 026**: the separate `@rust.delegate` compiler feature is withdrawn; forwarding and glue reduction are expressed here via body-less `impl` blocks, `@rust.derive`, and (where the ecosystem already provides them) proc macros such as those used by `std.web` and `incan_web_macros`.

## Core model

Read this RFC as one foundation plus three mechanisms:

**Foundation**: after RFC 041, imported Rust items are first-class compiler symbols and Incan types can wrap Rust types via `rusttype`. What is still missing is the ability to make those Incan types satisfy Rust trait contracts — the reverse direction of interop.

**Mechanisms**:

1. `impl` blocks on `rusttype` declarations let authors declare that a type satisfies a Rust trait by providing method bodies in Incan. The implementation produces the corresponding `impl Trait for Type { ... }` in emitted Rust.
2. `@rust.derive(...)` forwards Rust derive macros to the emitted struct, so Incan-authored models and newtypes can participate in Rust-ecosystem derive workflows (`Serialize`, `Deserialize`, `Clone`, etc.) without handwritten Rust.
3. Compiler-managed async bridging auto-generates `impl Future` for `rusttype` declarations that wrap `Future`-implementing Rust types, eliminating manual `Pin`/`Context`/`Poll` glue.

## Supersedes RFC 026 (user-defined trait bridges)

RFC 026 captured the real problem that **nominal wrappers hide Rust trait implementations** the inner type already has (a `newtype` or `rusttype` tuple struct does not automatically implement `Executor`, `FromRequestParts`, `Serialize`, and so on). It proposed **`@rust.delegate`** — a compiler-native decorator to generate forwarding `impl` blocks with optional method subsetting, renaming, and associated-type control.

That decorator-centric design is **withdrawn**. Maintaining two spellings (“delegate” vs “implement”) for Rust-side contracts would split tooling, diagnostics, and author mental models without enough unique power: most cases are already covered by a smaller set of mechanisms when composed deliberately.

**Adopted split of responsibilities (this RFC):**

1. **`@rust.derive(...)`** — Forward Rust proc-macro derives onto the emitted struct or enum (Serde, `Clone`, and third-party derives where authors declare the macro paths). This is the right default when the ecosystem exposes a derive for the trait (mirrors and generalizes the `std.web` + `incan_web_macros` pattern described in RFC 023 / RFC 024).
2. **`impl Trait:` on `rusttype` with Incan method bodies** — Custom trait logic, error mapping, and framework extension points where no derive exists or behavior must differ from the inner type.
3. **Body-less `impl Trait` on `rusttype`** — Pure forwarding when the backing Rust type already implements `Trait` and the wrapper should expose the same contract; the implementation generates the forwarding `impl` (see [Reference-level explanation](#reference-level-explanation-precise-rules)). This is the deliberate replacement for RFC 026’s “list methods and delegate” story, expressed in the same syntax family as custom `impl`s.
4. **Compiler-managed `impl Future`** — Specialized delegation for awaitability without handwritten `poll` glue.

**What we are not doing:** a dedicated `@rust.delegate(trait=..., methods=[...])` surface. If a trait is only satisfiable via a hand-written or third-party proc macro today, authors keep using that macro through `@rust.derive` once the macro is importable, or contribute thin `impl` bodies when derives are inappropriate.

**Historical record:** RFC 026 remains available under `closed/superseded/` for rationale, drawbacks, and phased-delivery notes that informed this consolidation.

## Motivation

### RFC 041 solved half the interop story

RFC 041 made imported Rust items behave like ordinary Incan symbols: methods resolve, coercions insert, capability bounds lower to Rust predicates. Users can **call** Rust APIs from Incan without ceremony.

But Rust APIs are not just functions you call. They are also **contracts you implement**. In Rust, types can promise to behave in certain ways by implementing "traits" - standardized interfaces that define what methods and behaviors a type supports. For example:

- A type that can be converted to a string implements the `Display` trait
- A type that can be used in async operations implements the `Future` trait  
- A type that can execute database queries implements the `Executor` trait
- A type that can be serialized to JSON implements the `Serialize` trait

Today, satisfying any of these contracts from Incan requires dropping into handwritten Rust — exactly the "bridge ceremony" that RFC 041 set out to eliminate.

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

### Concrete example: iteration and async traits

The clearest examples are iterator-like and async traits.

In Rust, implementing `Iterator`, `IntoIterator`, `Future`, or `Stream` often
requires extra helper types, associated type declarations, lifetime-sensitive
receiver forms, and, for async traits, `Pin`/`Context`/`Poll` machinery. Those
details are exactly where wrapper types become painful: the author may know the
intended behavior, but still has to build a parallel adapter layer just to make
the wrapper satisfy the Rust contract.

Under this RFC, the Incan author should be able to express the semantic intent
directly:

```incan
from rust::std::vec import Vec

type MyVec[T] = rusttype Vec[T]:
    impl IntoIterator[T]

    impl Iterator[T]:
        type Item = T
        index: usize

        def next(mut self) -> Option[T]:
            if self.index < len(self.0):
                item = self.0[self.index]
                self.index += 1
                return Some(item)
            return None
```

The point is not that Incan hides complexity by magic. The point is that the
compiler owns the Rust-side ceremony while the author owns the behavioral
intent. The same principle applies to async-facing traits:

- authors should not write `Pin`/`Context`/`Poll` boilerplate just to expose an
  awaitable wrapper;
- authors should not create parallel hand-written adapter structs solely to
  satisfy iteration contracts;
- authors should be able to forward or customize trait behavior using one
  language-native `impl` surface.

This is especially important for traits such as `Future`, `Iterator`, `Stream`,
`Deref`, `From`, and framework extension traits, where the semantic logic is
often simple but the target-language implementation shape is not.

## Goals

- **Trait Implementation**: Allow `rusttype` declarations to implement Rust traits using Incan syntax, either by providing custom method implementations or by forwarding existing behavior from the wrapped Rust type.
- **Automatic Derives**: Enable `@rust.derive(...)` to forward Rust's automatic code generation (like serialization or cloning) to Incan types.
- **Async Support**: Provide automatic `Future` trait implementation for types wrapping Rust async operations.
- **Standard Library Integration**: Let Incan's standard library express trait implementations in Incan rather than requiring separate Rust adapter code.
- **Incan-Native Syntax**: Keep the surface syntax natural to Incan developers while generating correct Rust code.

## Non-Goals

- Inline Rust code blocks in Incan source. This RFC does not add an escape hatch for arbitrary Rust syntax.
- Orphan rule circumvention. Incan follows Rust's coherence rules: you can only implement a foreign trait for a type you own (defined in your crate), or implement your own trait for a foreign type.
- Runtime trait objects (`dyn Trait`). All trait impl generation is compile-time only.
- Generic `impl` blocks with complex where-clauses in the initial version. Start with concrete types; generic impls are a future extension.
- A separate **`@rust.delegate`** compiler decorator. Pure forwarding is expressed with **body-less `impl` on `rusttype`** (and, when applicable, **`@rust.derive`**). See [Supersedes RFC 026](#supersedes-rfc-026-user-defined-trait-bridges).
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

The emitted Rust surface corresponds to:

```rust
impl From<tokio::task::JoinError> for TaskJoinError {
    fn from(error: tokio::task::JoinError) -> Self {
        Self(error.to_string())
    }
}
```

The user writes Incan. The Rust glue is derived from that contract.

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

The `impl Future` block declares the associated type. The implementation then derives the `poll` delegation by mapping the backing type's output through any declared interop edges (here, `JoinError` -> `TaskJoinError` via the `From` impl above).

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

The emitted Rust surface corresponds to:

```rust
#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct CustomerEvent {
    pub customer_id: String,
    pub email: String,
    pub amount: i64,
}
```

`@rust.derive` is distinct from Incan's `@derive`: the former forwards to Rust derive macros, the latter uses the Incan derive protocol from RFC 024.

### Pure trait forwarding

Sometimes a `rusttype` wrapper should expose the same Rust trait contract as the backing type without changing behavior. This is expressed with a body-less `impl` declaration:

```incan
from rust::sqlx import Executor, PgPool

type Pool = rusttype PgPool:
    impl Executor
```

This means the wrapper should satisfy `Executor` by forwarding the entire trait contract to the inner `PgPool`. The point is not novelty. The point is removing handwritten delegation code when the wrapper is only adding nominal or API-surface meaning, not changing behavior.

### Implementing arbitrary Rust traits

Pure forwarding is not the only path. When the wrapper needs different behavior, validation, data transformation, or trait support the backing type does not already provide, authors write method bodies in Incan:

```incan
from rust::std::fmt import Display, Formatter, FmtError

type UserId = rusttype i64:
    impl Display:
        def fmt(self, f: Formatter) -> Result[None, FmtError]:
            return f.write_str(f"user_{self.0}")
```

Here `UserId` implements `Display` with custom formatting logic rather than forwarding the backing type unchanged.

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
- The trait name must resolve to an imported Rust trait (via `rust::` or `std.rust`) and be available through the Rust interop metadata path.
- Method signatures in the `impl` block must match the trait's method signatures as exposed through Rust interop metadata. Implementations must validate parameter types, return types, receiver shapes, and asyncness.
- Associated types declared with `type AssocType = T` must be compatible with the trait's associated type constraints.
- If a trait item has associated types, either:
    - the `impl` block explicitly declares them, or
    - the implementation infers them from the backing Rust type's trait impl via Rust interop metadata.
Otherwise a diagnostic must require explicit declarations.
- `impl Trait` supports two modes:
    1. **Custom behavior** (method bodies present): authors provide explicit method implementations in Incan.
    2. **Pure forwarding** (body-less): the implementation generates a full Rust `impl Trait for Type` by delegating trait items to the backing Rust type. This remains the primary substitute for the withdrawn `@rust.delegate` design.
- Trait subset or rename semantics are out of scope for this RFC. A future extension could add narrower forwarding controls if experience shows the need, but the initial contract is whole-trait forwarding or explicit custom bodies.

- The implementation must produce a complete Rust `impl Trait for Type { ... }` block, with method bodies translated from the Incan source or forwarding behavior synthesized for body-less impls.
- When the backing Rust type already implements the trait and the `impl` block contains **no method bodies** (and any required associated types are specified or inferred), the implementation **must** generate forwarding for each trait item. This is the normative replacement for the withdrawn `@rust.delegate` design from RFC 026.
- If both `@rust.derive` and an `impl Trait` block specify the same trait, the declaration must be rejected as ambiguous.

#### Expected diagnostics for `impl`/forwarding path

- Trait not found in scope: "Trait `X` not imported or not available through Rust interop metadata." 
- `impl` outside `rusttype`: "`impl` blocks are only allowed inside `rusttype` declarations." 
- Signature mismatch: "Method `foo` signature differs from `Trait::foo`; expected `...`, found `...`."
- Forwarding failure: "Backing type does not implement `Trait` and no method bodies are present." 
- Associated type mismatch: "Associated type `Item` is incompatible with trait requirement." 

### `@rust.derive` decorator

- `@rust.derive(Name1, Name2, ...)` is valid on `model`, `class`, `enum`, `newtype`, and `rusttype` declarations (for `rusttype`, the attribute applies to the emitted Rust backing struct the same way as for tuple newtypes).
- Each name must resolve to an imported Rust derive macro (via `rust::` imports or known standard derives like `Clone`, `Debug`) and the derive macro path must be declared in `incan.toml` `[rust-dependencies]` unless it is one of the built-in standard derives approved by the compiler.
- The implementation emits `#[derive(path::Name1, path::Name2, ...)]` on the generated Rust struct or enum.
- `@rust.derive` must not conflict with Incan's `@derive`: using both on the same declaration is valid when they target different derives. Duplicate derives across both systems must be rejected.

### Async Future bridging rules

- When a `rusttype` declaration includes `impl Future:` with an associated `type Output = T`, the implementation generates `impl Future for Type` with a `poll` method that delegates to the backing type.
- The generated `poll` must handle `Pin` projection correctly, preserving Rust pinning guarantees. The implementation must maintain the invariant that `Pin<&mut Type>` corresponds to a safe projection into `Pin<&mut backing_type>`.
- Output type mapping: if the backing type's `Future::Output` differs from the declared `Output`, the compiler must insert the appropriate conversion (using `From` impls, `map`, or `map_err`) in the generated `poll` method.
- The implementation should verify, when Rust interop metadata is available, that the backing type actually implements `Future`. When that metadata is unavailable, the declaration may be accepted and left for rustc to validate.

### Interaction with existing features

#### RFC 026 (superseded)

RFC 026 recorded the wrapper–trait visibility problem and a decorator-based `@rust.delegate` design. That feature is **not** adopted. **Pure forwarding** is expressed with **body-less `impl Trait:`** on `rusttype`; **custom** trait behavior uses the same `impl` syntax with Incan method bodies. Where the ecosystem supplies a derive macro for the trait, **`@rust.derive`** remains the preferred path (including stdlib patterns wired through `incan_web_macros` per RFC 023 / RFC 024).

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
2. The implementation owns the translation: method signatures are validated against Rust interop metadata, bodies are translated through the standard Incan pipeline, and the output is a valid Rust `impl` block.
3. `@rust.derive` is a passthrough: the implementation does not interpret the derive macro, it forwards it to rustc.
4. Async bridging is a specialization of `impl Future` that implementations can optimize by delegating `poll` to the backing type.

### Compatibility / migration

Existing code continues to work unchanged. The new features are purely additive:

- Existing `rusttype` declarations without `impl` blocks remain valid.
- Existing Rust adapter modules in `incan_stdlib` can be incrementally migrated to Incan `impl` blocks (and `@rust.derive` where derives already cover the trait).
- No migration from `@rust.delegate` is required: that compiler feature was never shipped; RFC 026 is archived as superseded.

## Alternatives considered

- **Inline Rust blocks**  
    Some languages (e.g. Mojo, Zig) allow embedding target-language code directly. This was rejected because it breaks the "write Incan, not Rust" promise and makes tooling (formatter, LSP, diagnostics) much harder.

- **Automatic trait forwarding for all rusttype wrappers**  
    Instead of explicit `impl` blocks, the compiler could auto-forward all trait impls from the backing type. This was rejected because it would be unpredictable (users wouldn't know which traits their type satisfies), could cause coherence violations, and removes the author's ability to curate the type's API surface.

- **Method-level `@rust.impl` decorators instead of `impl` blocks**  
    Instead of `impl Trait:` blocks, trait implementation could use method-level decorators like `@rust.impl(FromRequestParts[State])`. This was rejected because associated types become awkward (where would `Future::Output` go?), trait method grouping gets lost (methods for the same trait could be scattered), and body-less forwarding becomes inconsistent (requiring a different syntax like `@rust.impl(Executor)` on the type itself). The `impl` block approach provides clearer semantics for trait implementation as a cohesive unit and aligns better with Rust's syntax.

- **RFC 026’s `@rust.delegate` as a parallel compiler feature**  
    A dedicated delegation decorator was considered and documented in RFC 026. It was **withdrawn** in favor of a single `impl` syntax on `rusttype`: body-less blocks mean forwarding; blocks with bodies mean author-defined behavior. That avoids two competing spellings for Rust trait contracts and keeps diagnostics and tooling unified.

## Drawbacks

- The implementation must understand enough of Rust's trait system to validate `impl` block signatures. This is a significant increase in semantic complexity.
- `Pin` projection for `impl Future` is notoriously tricky in Rust. Generating correct `poll` delegation requires careful handling of pinning guarantees.
- Associated types, default methods, and supertraits in Rust traits create a large surface area for edge cases.
- Orphan rules may confuse users who expect to implement any trait for any type (Rust's coherence rules will reject some combinations that seem natural from Incan's perspective).

## Layers affected

- **Language surface**: `impl TraitName[Args...]:` blocks inside `rusttype` bodies, associated type declarations, and `@rust.derive(...)` decorators must be supported as specified.
- **Interop validation**: `impl` method signatures, associated types, and `@rust.derive` targets must validate against the Rust interop metadata model.
- **Execution handoff**: implementations must produce correct Rust `impl` blocks, associated types, derive attributes, and `Future` projection behavior where this RFC requires them.
- **Stdlib / runtime (`incan_stdlib`)**: async task, time, sync, and channel adapter surfaces should be migratable from handwritten Rust glue to Incan-authored `impl` blocks once the feature lands.
- **Formatter**: `impl` blocks and `@rust.derive` decorators should format stably.
- **LSP / tooling**: completions for trait method signatures inside `impl` blocks and diagnostics for signature mismatches should be supported.

## Design decisions

- **Self receiver types**: Users write `self` in method signatures; the implementation infers the appropriate Rust receiver form (`self`, `&self`, `&mut self`, or `Pin<&mut Self>`) based on the trait's expected signature and the method's context. This keeps lifetime and borrow-shape concerns out of the user-facing Incan surface.

## Unresolved questions

- Should `impl Future` for `rusttype` wrappers be automatic (compiler-managed when the backing type implements `Future`) or always explicit (user must write `impl Future:` with the associated type)?
- How do associated types in `impl` blocks interact with Incan's type system? Should they be full Incan type expressions or restricted to imported Rust types?
- Should `@rust.derive` require the derive macro's crate to be declared in `incan.toml` `[rust-dependencies]`, or should well-known standard derives (`Clone`, `Debug`, `Copy`) be allowed without explicit declarations?
- When both `@rust.derive` and a body-less `impl Trait:` could apply to the same trait on the same `rusttype`, which takes precedence, or should the compiler reject the combination as ambiguous?
- Should blanket impls (e.g. `impl<F: FnOnce> RuntimeFuture for F`) be expressible in a future extension of this RFC, or should they remain Rust-only?
- What diagnostics should the implementation produce when an `impl` block's method signature does not match the Rust trait's expected signature? Should it show the expected Rust signature alongside the Incan mismatch?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
