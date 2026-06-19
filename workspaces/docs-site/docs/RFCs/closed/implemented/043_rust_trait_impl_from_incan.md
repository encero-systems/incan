# RFC 043: Rust Trait Implementation from Incan

- **Status:** Implemented
- **Created:** 2026-03-25
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 041 (First-class Rust interop authoring)
    - RFC 039 (`race` for awaitable concurrency)
    - RFC 026 (Superseded — archival; `@rust.delegate` withdrawn in favor of this RFC)
    - RFC 024 (Extensible derive protocol)
    - RFC 005 (Rust interop)
    - RFC 023 (Compilable stdlib & Rust module binding)
- **Issue:** https://github.com/encero-systems/incan/issues/200
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** v0.3

## Summary

This RFC lets Incan-owned types satisfy Rust ecosystem trait contracts without exposing Rust's `impl Trait for Type` authoring model as the Incan surface. Authors continue to opt concrete types into capabilities with `with TraitName`; ordinary methods satisfy adopted trait requirements when unambiguous; method-level `for TraitName` disambiguates collisions such as Rust's `Display::fmt` and `Debug::fmt`; and `@rust.derive(...)` forwards Rust derive macros where the Rust ecosystem already provides procedural code generation. The implementation still emits Rust `impl` blocks, derive attributes, and async adapter glue, but those are backend output details rather than the user's source syntax. This RFC supersedes RFC 026: the separate `@rust.delegate` compiler feature is withdrawn in favor of explicit `with` adoption, method-level trait targeting, derive forwarding, and backend-managed bridging for language-level protocols such as RFC 039's `Awaitable[T]`.

## Core model

Read this RFC as one foundation plus five mechanisms:

**Foundation**: after RFC 041, imported Rust items are first-class compiler symbols and Incan types can wrap Rust types via `rusttype`. What is still missing is the ability to make Incan types satisfy Rust trait contracts — the reverse direction of interop.

**Mechanisms**:

1. `with TraitName[Args...]` on a `rusttype`, model, class, enum, or newtype declares that the Incan type adopts the named capability. For imported Rust traits, adoption causes the backend to emit the corresponding Rust trait implementation when the type owns the implementation under Rust coherence rules.
2. Ordinary methods in the type body satisfy adopted trait methods when the method name and signature identify exactly one adopted trait requirement.
3. `def method(...) for TraitName[Args...] -> Return:` targets a method body at a specific adopted trait when several adopted traits could otherwise claim the same method name or signature.
4. `@rust.derive(...)` forwards Rust derive macros to the emitted Rust item, so Incan-authored models, enums, newtypes, and `rusttype` wrappers can participate in Rust derive workflows without handwritten Rust.
5. Language-level protocols remain Incan-first. For async, authors adopt `Awaitable[T]`; the Rust backend may realize that adoption as `impl Future<Output = T>`, but users do not write `impl Future` as the public language model.

## Supersedes RFC 026 (user-defined trait bridges)

RFC 026 captured the real problem that nominal wrappers hide Rust trait implementations the inner type already has. A `newtype` or `rusttype` tuple struct does not automatically implement `Executor`, `FromRequestParts`, `Serialize`, and similar Rust traits. RFC 026 proposed `@rust.delegate`, a compiler-native decorator for generating forwarding implementations.

That decorator-centric design is withdrawn. Maintaining separate "delegate" and "implement" spellings for Rust-side contracts would split tooling, diagnostics, and author mental models. The replacement is a smaller set of mechanisms that align with the rest of Incan:

1. **`with TraitName` adoption** — The type opts into the capability using the same spelling Incan already uses for trait conformance.
2. **Ordinary methods plus optional `for TraitName` targeting** — Method bodies remain in the type body. The `for` qualifier is used only when the compiler needs the author to identify which adopted trait requirement a method satisfies.
3. **Body-less adoption for forwarding** — When a `rusttype` adopts an imported Rust trait and the body provides no required methods for that trait, the implementation may synthesize a forwarding Rust implementation if metadata proves that the backing Rust type already implements the trait.
4. **`@rust.derive(...)`** — Rust derive macros remain the preferred path when the Rust ecosystem already provides derive support for the desired trait.
5. **Backend-managed language protocol bridging** — Builtin Incan protocols such as `Awaitable[T]` can map to Rust traits such as `Future` without making Rust trait names the Incan authoring model.

What this RFC does not adopt is a dedicated `@rust.delegate(trait=..., methods=[...])` surface or Rust-shaped source syntax such as `impl Trait for Type`.

## Motivation

### RFC 041 solved half the interop story

RFC 041 made imported Rust items behave like ordinary Incan symbols: methods resolve, coercions insert, and capability bounds lower to Rust predicates. Users can call Rust APIs from Incan without ceremony.

But Rust APIs are not just functions you call. They are also contracts you satisfy. In Rust, types promise behavior by implementing traits:

- a type that can be formatted for display implements `Display`
- a type that can be debug-formatted implements `Debug`
- a type that can be converted from another type implements `From[T]`
- a type that can be serialized for a Rust framework may derive or implement `Serialize`
- a type that can be awaited in Rust implements `Future`

Today, satisfying those Rust contracts from Incan usually means dropping into handwritten Rust. That is exactly the bridge ceremony RFC 041 set out to eliminate.

### The existing Incan trait model should remain the surface

Incan already teaches trait adoption with `with`:

```incan
model Bucket with Len, Index[int, str]:
    ...
```

That model is Python-friendly: a type declares the protocols it supports, then its body provides the behavior. Importing Rust's `impl Trait for Type` model would create a second conformance spelling for the same author intent. This RFC therefore keeps `with` as the source-level adoption verb and treats Rust `impl` blocks as generated backend output.

### Rust still needs explicit trait targets in ambiguous cases

Some Rust traits share method names. `Display` and `Debug` both require `fmt`. In Python, users would normally think in separate dunder surfaces such as `__str__` and `__repr__`, but arbitrary Rust traits cannot all be mapped to Python-shaped dunders. Incan needs a small disambiguation surface for the cases where two adopted Rust traits ask for the same member name.

Method-level `for Trait` is that surface:

```incan
from rust::std::fmt import Debug, Display, Formatter, FmtError

type UserId = rusttype i64 with Display, Debug:
    def fmt(self, f: Formatter) for Display -> Result[None, FmtError]:
        return f.write_str(f"user_{self.0}")

    def fmt(self, f: Formatter) for Debug -> Result[None, FmtError]:
        return f.write_str(f"UserId({self.0})")
```

The qualifier modifies the method declaration, not the return type. It says "this method body satisfies `Display.fmt`" or "this method body satisfies `Debug.fmt`".

### Async should stay Incan-first

RFC 039 introduces `Awaitable[T]` as the Incan-facing protocol behind `await`. This RFC follows that layering. An author should not write Rust's `Future<Output = T>` surface in Incan source when the language already has `Awaitable[T]`:

```incan
from rust::tokio::task import JoinHandle as TokioJoinHandle

type JoinHandle[T] = rusttype TokioJoinHandle[T] with Awaitable[Result[T, TaskJoinError]]:
    def abort(self) -> None:
        ...
```

The Rust backend may realize this as `impl Future for JoinHandle<T>`, including safe `Pin` projection and output mapping, but that is backend realization of an Incan protocol.

## Goals

- Allow Incan-owned types to satisfy imported Rust traits without handwritten Rust adapter modules.
- Preserve `with TraitName` as the source-level conformance/adoption syntax.
- Add method-level `for TraitName` as an ambiguity resolver for same-name trait members.
- Support whole-trait forwarding for `rusttype` wrappers when the backing Rust type already implements the adopted Rust trait.
- Enable `@rust.derive(...)` to forward Rust derive macros to emitted Rust types.
- Make `Awaitable[T]`, not Rust `Future`, the user-facing async contract while allowing the Rust backend to emit correct `Future` implementations.
- Let the standard library migrate Rust adapter glue into Incan source where the behavior is expressible in Incan.

## Non-Goals

- Inline Rust code blocks in Incan source. This RFC does not add an escape hatch for arbitrary Rust syntax.
- Rust-shaped source syntax such as `impl Trait for Type` or nested `impl Trait:` blocks as the main authoring model.
- Orphan rule circumvention. Incan follows Rust's coherence rules: you can implement a foreign trait for a type you own, or implement your own trait for a foreign type, but not arbitrary foreign-for-foreign combinations.
- Runtime trait objects (`dyn Trait`). All Rust trait implementation generation in this RFC is compile-time only.
- Blanket impls such as `impl<F: FnOnce> RuntimeFuture for F`. These require generic where-clause machinery and remain Rust-only or future-RFC work.
- Trait subset, rename, or partial forwarding controls. Initial forwarding is whole-trait forwarding only.

## Guide-level explanation (how users think about it)

### Implementing a Rust trait on a rusttype

The common case looks like normal Incan trait adoption: wrap a Rust type, say which capability the wrapper supports, and provide the required method.

```incan
from rust::tokio::task import JoinError
from rust::std::convert import From

type TaskJoinError = rusttype str with From[JoinError]:
    @classmethod
    def from(cls, error: JoinError) -> Self:
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

The user writes Incan. The backend emits the Rust glue.

### Disambiguating same-name Rust trait methods

When two adopted traits require methods that collide, attach a `for Trait` qualifier to the method declaration:

```incan
from rust::std::fmt import Debug, Display, Formatter, FmtError

type UserId = rusttype i64 with Display, Debug:
    def fmt(self, f: Formatter) for Display -> Result[None, FmtError]:
        return f.write_str(f"user_{self.0}")

    def fmt(self, f: Formatter) for Debug -> Result[None, FmtError]:
        return f.write_str(f"UserId({self.0})")
```

For non-ambiguous methods, no qualifier is required:

```incan
type TaskId = rusttype str with Display, From[JoinError]:
    def fmt(self, f: Formatter) -> Result[None, FmtError]:
        return f.write_str(self.0)

    @classmethod
    def from(cls, error: JoinError) -> Self:
        return TaskId(error.to_string())
```

### Async bridging through `Awaitable[T]`

For `rusttype` declarations that wrap Rust futures, the Incan-facing adoption is `Awaitable[T]`:

```incan
from rust::tokio::task import JoinHandle as TokioJoinHandle

type JoinHandle[T] = rusttype TokioJoinHandle[T] with Awaitable[Result[T, TaskJoinError]]:
    def abort(self) -> None:
        ...
```

The Rust backend owns this mapping, but the first implementation explicitly gates it until the compiler has enough metadata to prove safe `Pin` projection and output conversion. Without backend-managed bridging, the same adapter still requires handwritten Rust with `Pin`, `Context`, `Poll`, and manual error mapping.

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

`@rust.derive` is distinct from Incan's `@derive`: the former forwards to Rust derive macros, while the latter uses the Incan derive protocol from RFC 024.

### Pure trait forwarding

Sometimes a `rusttype` wrapper should expose the same Rust trait contract as the backing type without changing behavior:

```incan
from rust::sqlx import Executor, PgPool

type Pool = rusttype PgPool with Executor:
    pass
```

This means the wrapper should satisfy `Executor` by forwarding the whole trait contract to the inner `PgPool`, if metadata proves that `PgPool` already implements `Executor` and the generated forwarding respects Rust coherence and receiver rules.

## Reference-level explanation (precise rules)

### Adoption syntax

Rust trait implementation authoring uses Incan's existing `with` adoption clause:

```incan
type Name[Params...] = rusttype RustBacking[Params...] with TraitName[Args...], OtherTrait:
    ...
```

Normative rules:

- Imported Rust traits may appear in `with` adoption clauses for Incan-owned `rusttype`, model, class, enum, and newtype declarations.
- The trait name must resolve to an imported Rust trait, or to an Incan trait that the backend knows how to realize as a Rust trait contract.
- For foreign Rust traits, the implementing type must be owned by the current crate after lowering. The compiler must reject foreign-trait-for-foreign-type combinations before emission when it can prove the violation.
- A method in the type body satisfies an adopted trait method when the method name, receiver shape, parameter types, return type, asyncness, and trait context match exactly one required adopted trait item.
- If more than one adopted trait could claim a method, the method must use a `for TraitName[Args...]` qualifier.
- If a method uses `for TraitName[Args...]`, the named trait must be adopted by the enclosing type, either directly or through a transitive Incan supertrait relationship.
- If the method signature does not match the targeted trait item, the compiler must report a span-precise diagnostic at the method declaration.

### Method-level trait target syntax

The method-level trait target appears between the parameter list and the return arrow:

```incan
def method_name(params...) for TraitName[Args...] -> ReturnType:
    body
```

The qualifier modifies the method declaration. It does not modify the return type.

Examples:

```incan
def fmt(self, f: Formatter) for Display -> Result[None, FmtError]:
    ...

def fmt(self, f: Formatter) for Debug -> Result[None, FmtError]:
    ...
```

Formatter and LSP support must preserve this placement so users can read the declaration as "implement `method_name` for `TraitName`".

### Associated items

Rust traits may require associated types or associated constants. This RFC supports associated types through an explicit declaration in the adopting type body, targeted with `for TraitName` when needed:

```incan
type MyIter[T] = rusttype RustIter[T] with Iterator[T]:
    type Item for Iterator[T] = T

    def next(mut self) -> Option[T]:
        ...
```

Normative rules:

- Associated types may use full Incan type expressions that can be lowered to Rust types.
- Associated type declarations must target an adopted trait when the associated item name is not globally unambiguous.
- If a required associated type is missing and cannot be inferred from Rust interop metadata, the compiler must emit a diagnostic that names the missing associated type and trait.
- Associated type compatibility must be checked against the Rust trait metadata when available. If metadata is unavailable, the compiler may preserve the declaration for rustc validation but should still reject locally obvious type-shape errors.

### Forwarding mode

When a `rusttype` adopts an imported Rust trait and provides no explicit required methods or associated items for that trait, the adoption is a request for whole-trait forwarding:

```incan
type Pool = rusttype PgPool with Executor:
    pass
```

Normative rules:

- Forwarding mode is whole-trait only. Method subset, rename, or partial forwarding controls are out of scope.
- The compiler must verify, when metadata is available, that the backing Rust type already implements the adopted trait.
- If the backing Rust type does not implement the trait, or metadata proves that safe forwarding cannot be generated, the compiler must reject the declaration.
- If metadata is unavailable, the compiler may emit the forwarding implementation and leave final validation to rustc only when the generated code preserves source spans enough to produce actionable diagnostics.

### `@rust.derive` decorator

- `@rust.derive(Name1, Name2, ...)` is valid on `model`, `class`, `enum`, and `newtype` declarations.
- `@rust.derive(...)` on `rusttype` declarations is parsed but rejected until `rusttype` lowers to an owned Rust item; the current alias-based lowering cannot carry Rust derive attributes.
- Each third-party derive macro must resolve through an imported Rust macro path and must be backed by a declared Rust dependency in `incan.toml`.
- The compiler may whitelist built-in Rust derives such as `Clone`, `Copy`, `Debug`, `Default`, `Eq`, `Hash`, `Ord`, `PartialEq`, and `PartialOrd` without requiring a dependency declaration.
- The implementation emits `#[derive(path::Name1, path::Name2, ...)]` on the generated Rust struct or enum.
- `@rust.derive` may coexist with Incan's `@derive` when they target different generated behavior.
- If `@rust.derive` and `with TraitName` would both generate the same Rust trait implementation, the declaration must be rejected as ambiguous.

### `Awaitable[T]` bridging rules

- `Awaitable[T]` is the user-facing async protocol, following RFC 039.
- When a `rusttype` declaration adopts `Awaitable[T]`, the Rust backend may generate `impl Future<Output = T> for Type` only after it can prove the backing future projection and output mapping are safe.
- The generated `poll` method must handle `Pin` projection correctly, preserving Rust pinning guarantees. The implementation must maintain the invariant that `Pin<&mut Type>` corresponds to a safe projection into `Pin<&mut backing_type>`.
- Output type mapping must use declared Incan/Rust conversion edges when the backing type's future output differs from the adopted `Awaitable[T]` result type.
- The first implementation rejects `rusttype ... with Awaitable[T]` with a dedicated diagnostic instead of emitting an unsound or unverifiable `Future` bridge.

### Expected diagnostics

- Trait not found in scope: "Trait `X` is not imported or is not available through Rust interop metadata."
- Rust orphan rule violation: "`X` cannot implement foreign trait `Y`; neither side is owned by this crate."
- Ambiguous trait method: "Method `fmt` could satisfy multiple adopted traits; add `for Display` or `for Debug`."
- Trait target not adopted: "Method target `Display` is not adopted by `UserId`."
- Signature mismatch: "Method `fmt` differs from `Display::fmt`; expected `...`, found `...`."
- Forwarding failure: "Backing type does not implement `Trait` and no method bodies are present."
- Missing associated type: "Trait `Iterator` requires associated type `Item`; add `type Item for Iterator = ...`."
- Derive conflict: "`@rust.derive(Display)` conflicts with explicit `with Display` adoption."

### Interaction with existing features

#### RFC 026 (superseded)

RFC 026 recorded the wrapper-trait visibility problem and a decorator-based `@rust.delegate` design. That feature is not adopted. Pure forwarding is expressed as body-less `with Trait` adoption on `rusttype`; custom behavior uses ordinary methods plus optional method-level `for Trait` targeting.

#### RFC 024 (extensible derive protocol)

RFC 024's `@derive` uses the Incan derive protocol. This RFC's `@rust.derive` forwards to Rust's derive macro system. They serve different ecosystems and may coexist when they do not attempt to generate the same Rust trait implementation.

#### RFC 039 (`Awaitable[T]`)

RFC 039 defines `Awaitable[T]` as the Incan-facing protocol behind `await`. This RFC treats Rust `Future` as a backend realization of that protocol, not as source-level Incan syntax.

#### RFC 041 (first-class Rust interop)

This RFC extends RFC 041's `rusttype`, `rust::` import, and metadata infrastructure. Rust trait adoptions use the same imported-symbol model and must validate against the same interop metadata path when available.

## Design details

### Syntax

New or extended syntax:

- Imported Rust traits may appear in existing `with TraitName[Args...]` adoption clauses.
- Method declarations may include an optional trait target: `def name(params...) for TraitName[Args...] -> ReturnType:`.
- Associated type declarations may include an optional trait target: `type AssocName for TraitName[Args...] = IncanType`.
- `@rust.derive(Name, ...)` is a Rust derive forwarding decorator.

### Semantics

The semantic center of this RFC is:

1. Incan source uses `with` to declare conformance. The Rust backend emits `impl Trait for Type` as necessary.
2. Method bodies remain ordinary Incan methods. Method-level `for Trait` is an ambiguity resolver, not the default conformance syntax.
3. `@rust.derive` is a passthrough: the implementation does not interpret the derive macro, it forwards it to rustc after dependency and conflict validation.
4. `Awaitable[T]` is the async contract. Rust `Future` emission is backend realization.

### Compatibility / migration

Existing code continues to work unchanged:

- Existing `rusttype` declarations without Rust trait adoption remain valid.
- Existing Incan trait adoption with `with` keeps its current meaning.
- Existing Rust adapter modules in `incan_stdlib` can be incrementally migrated to `with` adoption, method-level `for Trait` targeting, and `@rust.derive` where appropriate.
- No migration from `@rust.delegate` is required because that compiler feature was never shipped; RFC 026 is archived as superseded.

## Alternatives considered

- **Inline Rust blocks**
    Some languages allow embedding target-language code directly. This was rejected because it breaks the "write Incan, not Rust" promise and makes formatter, LSP, and diagnostics substantially harder.

- **Rust-shaped `impl Trait:` blocks in Incan source**
    A direct `impl Trait:` syntax maps cleanly to Rust, but it creates a second conformance spelling beside Incan's existing `with TraitName` model. That is worse for Python-oriented users and conflicts with RFC 039's language-first async direction.

- **Nested `for Trait:` blocks**
    Grouping all methods under `for Trait:` blocks solves ambiguity, but it adds ceremony even when method ownership is obvious. Method-level `for Trait` keeps the common class-body shape and only adds targeting where needed.

- **Method-level `@rust.impl(...)` decorators**
    Decorators are familiar to Python users, but they get noisy for several methods and make associated types awkward. They also make trait conformance feel like metadata attached to methods instead of a type-level capability declared with `with`.

- **Automatic trait forwarding for all rusttype wrappers**
    The compiler could auto-forward all trait implementations from the backing Rust type. This was rejected because users would not know which traits their wrapper satisfies, coherence violations would be harder to explain, and authors would lose control over the wrapper's public capability surface.

- **RFC 026's `@rust.delegate` as a parallel compiler feature**
    A dedicated delegation decorator was considered and documented in RFC 026. It was withdrawn in favor of one adoption model: `with Trait` declares conformance, ordinary methods implement behavior, and body-less adoption requests whole-trait forwarding.

## Drawbacks

- The implementation must understand enough of Rust's trait system to validate adopted trait methods, associated items, derives, and forwarding requests.
- Method-level `for Trait` is new syntax and must be taught carefully so users understand that it targets the method implementation, not the return type.
- Safe `Awaitable[T]` to Rust `Future` bridging requires careful handling of pinning guarantees.
- Associated types, default methods, and supertraits in Rust traits create a large surface area for edge cases.
- Rust orphan rules may confuse users who expect every visible trait/type combination to be implementable from Incan.

## Layers affected

- **Language surface**: imported Rust traits must be accepted in `with` adoption clauses; method declarations must support optional `for Trait` targeting; associated type declarations must support optional trait targeting; `@rust.derive(...)` must be supported as specified.
- **Parser / AST**: the parser must represent method-level trait targets and associated type targets without confusing them with return types or ordinary `for` statements.
- **Typechecker / interop validation**: adopted Rust traits, targeted methods, associated types, derive macros, forwarding requests, and `Awaitable[T]` bridges must validate against Rust interop metadata when available.
- **Lowering / emission**: the backend must emit Rust `impl` blocks, associated type items, derive attributes, forwarding methods, and `Future` bridge implementations from the Incan adoption model.
- **Stdlib / runtime (`incan_stdlib`)**: async task, time, sync, channel, conversion, formatting, and framework adapter surfaces should be migratable from handwritten Rust glue to Incan-authored adoption where this RFC makes the behavior expressible.
- **Formatter**: `with` adoption, method-level `for Trait`, associated type targets, and `@rust.derive` must format stably.
- **LSP / tooling**: completions and diagnostics should help users discover required trait methods, disambiguate same-name methods with `for Trait`, and understand Rust metadata mismatch errors.

## Implementation Plan

### Phase 1: Syntax, AST, and formatting

- Extend method declarations to carry an optional trait target parsed from `def name(params...) for Trait -> Return:`.
- Extend newtype/rusttype bodies to carry associated type declarations with optional `for Trait` targets.
- Ensure imported Rust traits already accepted in `with` clauses remain distinguishable from ordinary Incan trait adoptions for later validation.
- Update formatter output for method-level `for Trait`, associated type targets, `rusttype ... with Trait`, and the RFC examples.
- Add parser and formatter tests for unambiguous methods, same-name method targets, associated type targets, and invalid target placement after the return type.

### Phase 2: Semantic validation

- Resolve method-level trait targets against the enclosing type's adopted traits and reject targets that are not adopted.
- Detect same-name trait method ambiguity and require `for Trait` only when the method could satisfy multiple adopted trait requirements.
- Validate method signatures against imported Rust trait metadata when available, including receiver shape, parameters, return type, asyncness, and classmethod/staticmethod form.
- Validate associated type declarations as Incan type expressions that lower to Rust types, and require explicit declarations when metadata cannot infer required associated types.
- Add diagnostics for ambiguous method targets, target-not-adopted, signature mismatch, missing associated type, derive/adoption conflict, and Rust orphan-rule violations where statically knowable.

### Phase 3: Lowering and Rust emission

- Preserve targeted trait methods and associated type declarations through checked metadata and IR lowering.
- Emit Rust `impl Trait for Type` blocks from `with` adoption for Incan-owned types and imported Rust traits.
- Emit custom method bodies into the targeted Rust trait impl and synthesize whole-trait forwarding only when metadata proves the backing Rust type already implements the adopted trait.
- Emit associated type items in the generated Rust impl, lowering Incan type expressions to Rust types.
- Implement `Awaitable[T]` bridging for `rusttype` wrappers as Rust `Future` realization, including output mapping and safe pin projection policy.
- Add codegen snapshot and integration coverage for `From`, `Display`/`Debug` same-name disambiguation, associated type emission, forwarding failure diagnostics, and `Awaitable[T]` bridging.

### Phase 4: Rust derive passthrough and dependency validation

- Validate `@rust.derive(...)` separately from Incan `@derive(...)`.
- Whitelist built-in Rust derives that do not require external dependencies.
- Require third-party derive macros to resolve through imports and declared Rust dependencies.
- Reject derive/adoption combinations that would generate duplicate Rust trait implementations.
- Add tests for built-in derive passthrough, third-party derive dependency validation, and derive/adoption conflict diagnostics.

### Phase 5: Stdlib migration, docs, and release tracking

- Migrate a narrow stdlib adapter surface that currently requires handwritten Rust trait glue into Incan-authored adoption where this RFC makes the behavior expressible.
- Update user-facing trait, Rust interop, async, and stdlib docs to teach `with`, method-level `for Trait`, `@rust.derive`, and `Awaitable[T]` bridging.
- Add a release notes entry for the active development line.
- Bump the active dev version by one increment when implementation code lands.
- Run targeted parser/typechecker/codegen/docs checks during slice work and the repository pre-commit gate after integration.

## Implementation log

### Spec / design

- [x] Resolve source-level syntax around `with` adoption and method-level `for Trait`.
- [x] Resolve async bridging through `Awaitable[T]` rather than source-level `Future`.
- [x] Resolve associated types as Incan type expressions.
- [x] Resolve `@rust.derive` dependency and conflict rules.
- [x] Resolve blanket impls as out of scope.

### Parser / AST / formatter

- [x] AST: add optional method-level trait target.
- [x] AST: add associated type declarations for newtype/rusttype bodies.
- [x] Parser: parse `def name(params...) for Trait -> Return:`.
- [x] Parser: parse `type Assoc for Trait = IncanType`.
- [x] Parser diagnostics: reject `for Trait` after the return type.
- [x] Formatter: print method-level `for Trait` and associated type targets stably.
- [x] Tests: parser and formatter coverage for the new syntax.

### Typechecker / diagnostics

- [x] Resolve targeted methods against enclosing `with` adoptions.
- [x] Detect ambiguous same-name trait methods and require `for Trait`.
- [x] Validate method signatures against trait metadata when available.
- [x] Resolve associated type declarations as Incan type expressions.
- [x] Diagnose missing associated type requirements from Rust trait metadata.
- [x] Validate Rust orphan-rule violations where statically knowable.
- [x] Validate `@rust.derive` dependency and duplicate-impl conflicts.
- [x] Tests: diagnostics for ambiguity, target-not-adopted, signature mismatch, missing associated type, derive conflict, and orphan-rule violations.

### Lowering / emission

- [x] Preserve targeted methods and associated types through checked metadata and IR.
- [x] Emit Rust trait impls from `with` adoption for newtype/rusttype declarations.
- [x] Emit custom targeted methods into Rust trait impls.
- [x] Emit associated type items into non-local Rust trait impls.
- [x] Synthesize whole-trait forwarding for valid body-less rusttype adoption by accepting metadata-proven backing impls and skipping invalid alias impl emission.
- [x] Explicitly gate `Awaitable[T]` to `Future` bridging until safe pin projection and output mapping metadata exist.
- [x] Tests: codegen snapshot coverage for targeted newtype impls.
- [x] Tests: integration coverage for imported Rust trait associated types, forwarding, and async bridge gating.

### Stdlib / docs / release

- [x] Evaluate stdlib Rust adapter migration; no safe adapter migrates yet because `rusttype` derive and `Awaitable` bridges are explicitly gated on alias/pinning constraints.
- [x] Update trait authoring reference docs.
- [x] Update newtype reference docs.
- [x] Update Rust interop docs.
- [x] Update async docs for `Awaitable[T]` bridge gating.
- [x] Update release notes for the active development line.
- [x] Bump active dev version when implementation code lands.

### Verification

- [x] Run focused parser/formatter tests.
- [x] Run focused typechecker tests.
- [x] Run focused codegen/integration tests.
- [x] Run docs build.
- [x] Run repository pre-commit gate.

## Design decisions

- **Source-level conformance uses `with`**: Rust `impl` blocks are backend output, not the main Incan authoring syntax.
- **Method-level targeting uses `for Trait` before the return arrow**: `def name(params...) for Trait -> Return:` targets the method implementation at an adopted trait. Placing `for Trait` after the return type is rejected because it reads like a return-type modifier.
- **Trait targeting is optional until ambiguous**: ordinary methods satisfy adopted trait requirements when the compiler can identify exactly one target. `for Trait` is required when more than one adopted trait could claim the method.
- **Associated types use Incan type expressions**: associated type values are not restricted to imported Rust types; they may be full Incan type expressions that lower to Rust types.
- **Third-party Rust derives require declared dependencies**: built-in Rust derives may be whitelisted, but external derive macros must resolve through imports and declared Rust dependencies.
- **Derive/adoption conflicts are errors**: if `@rust.derive(...)` and explicit `with Trait` adoption would generate the same Rust trait implementation, the compiler must reject the declaration rather than choosing precedence.
- **Blanket impls are out of scope**: generic blanket implementations remain Rust-only or future-RFC work.
- **Diagnostics should show both surfaces**: signature mismatch diagnostics should point at the Incan declaration and include the expected Rust trait item shape when metadata is available.
- **Async bridges through `Awaitable[T]`**: authors adopt `Awaitable[T]`; the Rust backend may emit `Future` implementations as the realization of that Incan protocol.
- **Self receiver types stay Incan-shaped**: users write `self` or `mut self`; the implementation infers the Rust receiver form (`self`, `&self`, `&mut self`, or `Pin<&mut Self>`) from the trait metadata and method context.
