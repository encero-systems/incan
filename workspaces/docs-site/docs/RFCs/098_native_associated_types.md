# RFC 098: Native associated types for traits

- **Status:** Draft
- **Created:** 2026-05-15
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 023 (compilable stdlib and Rust module binding)
    - RFC 042 (traits are always abstract)
    - RFC 043 (Rust trait implementation from Incan)
    - RFC 054 (explicit call-site generic arguments)
    - RFC 065 (`std.hash`)
    - RFC 088 (iterator adapter surface)
    - RFC 099 (generic trait-targeted methods)
    - RFC 101 (`std.collections.OrdinalMap`)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/580
- **Blocks:** https://github.com/dannys-code-corner/incan/issues/596 (v0.5 RFC 101 trait-system bridge removal)
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC introduces native associated types for Incan traits: a trait may declare named type members that each implementation fills in, and trait methods, generic functions, and type annotations may refer to those members through checked type projections such as `Self.Item`, `H.Digest`, or `P.Value`. Associated types let libraries model Rust-shaped capability families without multiplying traits or helper functions for every implementer-selected type, while keeping runtime values, generic type parameters, and trait-owned type members separate in the language model.

## Core model

1. **Associated types are trait-owned type members:** `type Item` inside a trait declares that every implementation chooses a concrete type for `Item`.
2. **Implementations bind the member:** a class, enum, newtype, or rusttype adopting the trait must provide a matching type declaration for each required associated type.
3. **Trait methods may project the member:** signatures may use `Self.Item` to refer to the implementing type's chosen item type.
4. **Generic code may project through bounds:** a generic parameter `H with StreamingHash` may use `H.Digest` in type positions when `StreamingHash` declares `Digest`.
5. **Projection is checked statically:** every associated-type projection must resolve to exactly one trait-owned type member or produce a diagnostic.
6. **Adjacent features are explicit design decisions:** associated type members sit next to generic associated types, associated constants, specialization, defaults, and trait-object dispatch; this RFC must explain those concepts and decide their relationship deliberately rather than excluding them by assumption.

## Motivation

Incan's trait system can already express method requirements, generic bounds, supertraits, and Rust trait adoption. It cannot yet express a trait where each implementer chooses a type that other methods and generic helpers can refer to. That gap pushes library authors toward awkward designs: duplicated helper functions for each return type, parallel traits that differ only by final value shape, or stringly/runtime dispatch where a static type relationship should exist.

The `std.hash` surface is a concrete example. A streaming hasher has one common operation, `update(mut self, chunk: bytes)`, but different algorithms finalize to different digest shapes: fixed digest bytes, caller-sized SHAKE bytes, `u32`, `u64`, or `u128`. Without associated types, the shared reader-draining logic can be factored only up to the `update` operation; finalization still requires thin wrappers for each digest family. That is a reasonable current implementation, but it is not the best language model.

The same need appears in iterator-like abstractions, parser and codec APIs, resource readers, adapters, graph traversals, and Rust interop. RFC 023 explicitly deferred operations such as iterator item propagation because they require associated-type inference. RFC 042 left associated types as future trait-system work. RFC 043 handles associated types for Rust trait adoption, but native Incan traits need the same concept as a first-class language feature rather than an interop-only escape hatch.

The end-state should be simple: if a capability has an implementer-selected type member, the trait should say so directly, and generic code should be able to use that type member without inventing extra type parameters or wrapper traits.

## Goals

- Allow native Incan traits to declare associated types.
- Allow adopting types to bind associated types with ordinary Incan type expressions.
- Allow trait method signatures to reference associated types through `Self`.
- Allow generic functions and methods to project associated types from bounded type parameters.
- Define ambiguity, missing-binding, and signature-compatibility diagnostics.
- Define how associated types interact with supertraits and same-name associated type members.
- Align native associated types with the Rust trait adoption model from RFC 043 without making the feature Rust-only.
- Provide enough expressive power for streaming hash digest values, iterator item propagation, parser value types, and adapter APIs.
- Preserve associated type declarations, bindings, projections, trait-qualified targets, and resolution provenance in checked metadata, generated documentation, LSP views, and package manifests.
- Identify adjacent trait-system features such as higher-rank associated types, specialization, associated constants, defaults, and trait objects so they can be accepted into this RFC or deferred by explicit design decision.

## Non-Goals

- This RFC does not replace generic trait parameters. Traits may still use ordinary type parameters when the caller chooses the type.
- This RFC does not require every existing generic trait to migrate to associated types.
- This RFC does not change runtime object layout; associated types are compile-time type relationships.
- This RFC does not define an implementation ticket for each compiler subsystem.

## Guide-level explanation

A trait can declare an associated type with a `type` item in the trait body. The name is part of the trait's vocabulary, not a special keyword:

```incan
pub trait PullIterator:
    type Item

    def next(mut self) -> Option[Self.Item]: ...
```

`Item` is not a field and not a runtime value. It is a type member. The trait says every pull iterator yields some item type, but the trait does not choose that type itself.

An implementation binds that member:

```incan
pub class Lines with PullIterator:
    type Item = str

    def next(mut self) -> Option[str]:
        ...
```

Another implementation may choose a different type for the same associated member:

```incan
pub class ByteChunks with PullIterator:
    type Item = bytes

    def next(mut self) -> Option[bytes]:
        ...
```

Generic code can use the implementer's chosen type:

```incan
def next_or_default[I with PullIterator](
    mut input: I,
    default: I.Item,
) -> I.Item:
    match input.next():
        Some(value) => return value
        None => return default
```

The same mechanism works for `std.hash`, but the associated type name should match that domain:

```incan
pub trait StreamingHash:
    type Digest

    def update(mut self, chunk: bytes) -> None: ...

    def finalize(mut self) -> Result[Self.Digest, HashError]: ...
```

`Sha256Hasher` could bind `Digest` to `bytes`, while `Xxh64Hasher` could bind it to `u64`:

```incan
pub class Xxh64Hasher with StreamingHash:
    type Digest = u64

    def update(mut self, chunk: bytes) -> None:
        ...

    def finalize(mut self) -> Result[u64, HashError]:
        ...
```

A shared reader helper can then return the hasher's digest type without caring whether it is bytes, `u64`, or another concrete representation:

```incan
def feed_reader[H with StreamingHash, R with BinaryReader](
    mut hasher: H,
    mut input: R,
    chunk_size: int,
) -> Result[H.Digest, HashError]:
    loop:
        let chunk = input.read_bytes(chunk_size).map_err((err) => HashError(kind="io", algorithm="", detail=err.detail))?
        if len(chunk) == 0:
            break
        hasher.update(chunk)

    return hasher.finalize()
```

That helper returns `Result[bytes, HashError]` for a byte-digest hasher and `Result[u64, HashError]` for `Xxh64Hasher` without separate helper overloads.

Associated types are useful when the implementer chooses the type. Ordinary generic parameters are still the right model when the caller chooses the type. For example, `Callable[T, U]` is still a generic trait because the call site controls parameter and return types through a concrete callable signature.

When a type adopts multiple traits with the same associated type name, the implementation can target the declaration:

```incan
pub class DualValue with TextParser and BinaryParser:
    type Value for TextParser = str
    type Value for BinaryParser = bytes
```

Most code should not need targeted syntax. It is for ambiguity, not for the common path.

## Reference-level explanation

### Declarations and bindings

Associated types enter the model at the trait boundary. A trait body may declare a type member with `type Name`, where `Name` must be a type identifier and must not collide with another associated type declared directly by the same trait. The declaration creates a required member of the trait contract; it does not choose the concrete type and it does not create a runtime value. Every concrete adoption of a trait with required associated types must bind each required member before the type is considered to implement the trait, and an omitted binding must produce a diagnostic that names the adopting type, the trait, and the missing associated type.

An adopting type binds an associated type with `type Name = TypeExpression` when the associated type name is unambiguous among the adopted trait and its supertraits. If multiple adopted traits or supertraits expose the same associated type name, the adoption must use the targeted form `type Name for TraitExpression = TypeExpression` so the binding's owner is explicit. The `TypeExpression` may use ordinary Incan type syntax, including the adopting type's generic parameters, and must be checked like any other type annotation. If the type expression cannot be resolved, cannot be lowered for the target backend, or violates a trait-imposed constraint, the compiler must reject the adoption.

Trait-owned capability families from RFC 099 may also provide associated type bindings when the binding is determined by language metadata rather than an authored type body. For example, a storage-backed integer capability can bind `Storage` from the newtype's checked storage carrier, and a deterministic key capability can expose representation metadata for language scalar families. These bindings must enter the same implementation metadata as ordinary authored bindings; generic projection must not care whether the binding came from a type body or from a proven trait-owned capability family.

### Projection and resolution

After an associated type is declared and bound, source code refers to the chosen type through projection syntax. Trait methods may use `Self.Name` to refer to an associated type declared by the trait or by exactly one of its supertraits. Generic functions, methods, and type aliases may use `T.Name` when `T` is a type parameter or concrete type whose active bounds or implementation metadata expose exactly one associated type named `Name`. Associated-type projection is valid only in type positions; it must not be treated as value-field access.

Simple projection must resolve to exactly one associated type. When the simple form is ambiguous, source may use the trait-qualified projection form `T.Name for TraitExpression`, and the named `TraitExpression` must be one of the traits known to be implemented by `T` after generic substitutions and supertrait expansion. A projection from a concrete type must resolve through that type's known implementation, while a projection from a generic type parameter must resolve through the parameter's active bounds. If the compiler cannot prove the projection from those bounds, it must reject the code rather than treating the projection as `Unknown`.

### Compatibility and inheritance

Associated type bindings participate in method signature compatibility. If a trait requires `def finalize(mut self) -> Result[Self.Digest, HashError]`, then an implementation with `type Digest = u64` may implement `finalize` as returning `Result[u64, HashError]`; returning `Result[bytes, HashError]` for that implementation must be rejected. The binding is therefore not a separate annotation that merely documents the implementation. It is part of the signature contract used to compare the implementation against the trait.

Supertraits may declare associated types, and subtraits inherit those requirements. A subtrait may refer to inherited associated types through `Self.Name` when the name is unambiguous, but it must not silently redeclare an inherited associated type with the same name unless a later design explicitly permits refinement or equality constraints. Trait cycles involving associated types must use the same diagnostic model as supertrait cycles: any cycle that prevents associated type resolution must produce a diagnostic rather than causing infinite expansion.

Associated types have no runtime representation by themselves. They affect type checking, method compatibility, generic substitution, diagnostics, and generated backend signatures.

### Metadata and inspection contract

Associated type behavior must be inspectable as checked semantic data, not only as backend output. Checked API metadata, package manifests, generated documentation, semantic inspection, and LSP surfaces must be able to expose at least:

- associated type declarations owned by a trait, including source identity and supertrait provenance;
- associated type bindings supplied by concrete adoptions, targeted bindings, Rust interop metadata, or RFC 099 trait-owned capability families;
- projections used in public signatures and generic bounds, including the trait identity that made the projection legal;
- ambiguity or degraded-state markers when a projection cannot be resolved in tolerant tooling modes;
- signature-compatibility relationships that explain how a concrete method return or parameter type satisfies a trait requirement through an associated type binding.

Generated documentation should show associated types as part of the public trait contract, not as hidden implementation detail. LSP hover and go-to-definition should let a reader move between a projection such as `H.Digest`, the trait-owned declaration of `Digest`, and the concrete binding when the binding is known. Diagnostics and `incan explain`-style tooling should report the same identities that metadata consumers see so there is one explanation for why a projection typechecks or fails.

## Design details

### Syntax

The trait declaration syntax is intentionally close to existing type declarations:

```incan
pub trait Parser:
    type Value

    def parse(self, input: str) -> Result[Self.Value, ParseError]: ...
```

The implementation binding syntax is:

```incan
pub class JsonParser with Parser:
    type Value = JsonValue
```

The targeted binding syntax mirrors RFC 043's associated type target form:

```incan
pub class Adapter with LeftTrait and RightTrait:
    type Value for LeftTrait = str
    type Value for RightTrait = bytes
```

Projection syntax in type positions is:

```incan
Self.Item
H.Digest
P.Value for Parser
```

The simple `H.Digest` form is preferred when it is unambiguous. The trait-qualified form is reserved for cases where multiple bounds expose the same name.

### Adjacent trait-system features

Associated types are one trait-system feature, not the whole trait-system roadmap, but they sit close to several other features that need to be named rather than accidentally accepted or excluded. Generic associated types extend the idea by giving the associated type its own parameters: a normal trait says `type Item`, while a lending or borrowed-view trait might eventually want `type Item[Borrow]` and projections such as `Self.Item[SomeBorrow]`. That is more powerful because the implementation no longer chooses one concrete type member; it chooses a family of related types. Associated constants move in a different direction: they are trait-owned value members, such as `const DIGEST_SIZE: int` on a digest trait or `const RANK: int` on a matrix trait, and they require value-level rules for constant evaluation and backend representation rather than type projection alone. Trait-owned capability families, as described by RFC 099, are adjacent because they may supply associated type bindings from language metadata without reopening an existing type declaration.

Defaults and dispatch-related features are adjacent for different reasons. Default associated types let a trait provide a fallback binding, such as `type Error = Never`, which can remove boilerplate but also changes ambiguity, semver, and inheritance behavior. Specialization would let a more specific implementation or method body override a generic one, raising coherence questions that should not enter through the side door of associated types. Trait-object dispatch for projected types, such as a hypothetical `Box[StreamingHash with Digest = bytes]`, is likewise not just surface syntax: it needs object-safety rules, method-availability rules, mutability rules, and a way to keep projected types known after dynamic dispatch.

This RFC's north star is native trait-owned type members. Any of the adjacent features above may belong near that north star if they are required for a coherent design, but none of them should be pulled in by implication. Each needs an explicit decision about whether it is part of RFC 098 or belongs in a follow-up RFC.

### Choosing associated types versus generic parameters

Associated types should be used when the implementer chooses the type and generic consumers need to recover it. Generic type parameters should be used when the caller chooses the type.

This distinction matters because the two designs express different contracts:

```incan
pub trait Decoder[T]:
    def decode(self, input: bytes) -> Result[T, DecodeError]: ...
```

`Decoder[T]` means a caller can ask for a decoder of a particular `T`.

```incan
pub trait Decoder:
    type Value

    def decode(self, input: bytes) -> Result[Self.Value, DecodeError]: ...
```

`Decoder` with `Value` means the decoder implementation decides what it produces.

### Interaction with supertraits

Associated type requirements flow through supertraits. If `ByteDigestHasher` extends `StreamingHash`, any type that implements `ByteDigestHasher` also satisfies the `StreamingHash` associated type requirements.

```incan
pub trait StreamingHash:
    type Digest

pub trait ByteDigestHasher with StreamingHash:
    def finalize(mut self) -> Result[Self.Digest, HashError]: ...
```

If a subtrait needs a stronger semantic promise about an inherited associated type, it should express that promise through method signatures or a separate associated-type constraint mechanism. This RFC does not introduce refinement syntax.

### Interaction with Rust interop

RFC 043 already defines associated type declarations for Rust trait adoption. This RFC generalizes the concept to native Incan traits and should align syntax and diagnostics with that existing surface.

When an Incan type implements an imported Rust trait with associated types, the compiler should preserve RFC 043 behavior. When an Incan trait with associated types lowers to Rust, the generated Rust trait should use Rust associated types where possible. If a target backend cannot represent native associated types, it must reject the affected code with a clear diagnostic.

### Interaction with `Self`

`Self.Name` in a trait body refers to the associated type selected by the eventual implementation. `Self.Name` in an implementation body refers to the associated type binding for the implementing type and trait context.

An associated type projection must not be confused with field access. `Self.Item`, `Self.Digest`, and `H.Digest` in type position are type projections. `self.value` in expression position remains ordinary value access.

### Diagnostics

The compiler should provide focused diagnostics for:

- missing associated type binding
- unknown associated type name
- ambiguous associated type binding
- ambiguous associated type projection
- projection from a type without the required trait bound
- implementation method signature incompatible with the associated type binding
- associated type declaration cycle through supertraits or aliases

Diagnostics should avoid suggesting Rust syntax as the primary fix unless the code is inside an explicit Rust interop context.

### Compatibility

This RFC is additive. Existing traits without associated types continue to work unchanged. Existing Rust trait associated type support from RFC 043 remains valid. Existing generic traits should not be migrated solely for style; migration is justified when associated types remove duplicated shape-specific traits or make generic consumers more precise.

## Alternatives considered

The closest alternative is to keep using ordinary generic trait parameters for every implementer-specific type family. That remains the right model when the caller chooses the type, but it is a poor fit when the implementation owns the choice. It forces APIs to carry generic arguments that are not real inputs to the capability, and it makes generic consumers restate type information the implementation should already provide.

Shape-specific trait splitting solves some small cases but does not scale as a language model. Traits such as `ByteDigestHasher`, `Hash64DigestHasher`, and `Hash128DigestHasher` can be clear when the surface is tiny, yet they prevent one generic helper from returning the implementer's selected type and they multiply every adapter or helper by result shape. Union returns have the opposite problem: `Union[bytes, u64, u128]` puts every possible result into one signature, but it moves a static relationship into runtime branching and forces callers to narrow values even when the concrete implementation is already known.

Overloading or multimethod dispatch would also miss the core relationship. Incan does not currently expose a general overload model, and overloads would still not give other signatures a stable name for the type selected by the implementation. Treating associated types as a Rust-only interop detail fails for the same reason: stdlib and user-authored Incan traits need this modeling power even when no external Rust trait is involved.

## Drawbacks

Associated types add another dimension to trait reasoning. Users must learn the difference between a generic parameter chosen by the caller and an associated type chosen by the implementer, and that distinction has to stay visible in examples, diagnostics, and API design guidance. Projection syntax can also become noisy for types that implement several traits with the same associated type name; the simple form should remain common, but the language still needs a precise targeted form for ambiguous cases.

The implementation cost is also real. The typechecker must resolve projections through bounds, supertraits, concrete implementations, and generic substitutions, which makes ambiguity diagnostics and cycle detection more important. Backend emission must preserve associated type relationships instead of erasing them into unknown or cloned concrete types, raising the bar for generated Rust quality and for non-Rust backend parity.

## Implementation architecture

Associated types should be represented as first-class trait members in checked metadata, not as synthetic methods or stringly annotations. Trait adoption should carry resolved associated type bindings alongside method implementations. Generic bound resolution should expose associated type projections from active bounds and supertrait closures. Backend lowering should preserve projected types until enough concrete information is available to emit target-language associated type items or substituted concrete types.

This section is non-normative. The contract is the language behavior above; compiler internals may choose a different representation if they preserve the same checked semantics and diagnostics.

## Layers affected

- **Parser / AST**: trait bodies and adopting type bodies must accept associated type declarations and targeted associated type bindings. Type annotations must accept associated type projections in type positions.
- **Typechecker / Symbol resolution**: trait metadata must record associated type declarations, adoption metadata and RFC 099 trait-owned capability metadata must record bindings, projections must resolve through active bounds and concrete implementations, and signature compatibility must substitute associated type bindings.
- **IR Lowering**: checked associated type projections must remain visible enough for backend type lowering and diagnostics rather than degrading to unknown types.
- **Emission**: Rust emission should lower native associated types to Rust associated type items where possible and substitute concrete bindings in generated method signatures as required.
- **Stdlib / Runtime (`incan_stdlib`)**: stdlib traits such as streaming hashers, iterators, parsers, codecs, and adapters may use associated types to remove duplicated shape-specific abstractions.
- **Formatter**: associated type declarations, targeted bindings, and projection types must format stably.
- **Library manifests / checked API metadata**: public trait metadata must preserve associated type declarations, bindings, projections, trait-qualified targets, and degraded-state markers for downstream package consumers and generated documentation.
- **LSP / Tooling**: completions, hover, go-to-definition, diagnostics, semantic inspection, and explanation commands should expose associated type members, concrete bindings, provenance, and ambiguity reasons.

## Unresolved questions

- Should this RFC include trait-qualified projection syntax exactly as `T.Item for Trait`, or should the language choose a more explicit projection form before implementation?
- Should associated type names follow the same capitalization convention as nominal types, or should the formatter/linter enforce a Rust-like `UpperCamelCase` convention only as style?
- Should generic associated types be part of RFC 098's north star, or should they be a separate RFC after non-generic associated type projection is settled?
- Should associated constants be designed alongside associated types, especially for stdlib traits with fixed sizes or capabilities, or should value-level trait members stay separate?
- Should default associated types be specified here, or should defaults be a separate design from required associated type bindings?
- Should specialization or overlapping implementation behavior be considered in this RFC, or should associated types deliberately preserve today's trait coherence model?
- Should trait-object dispatch with fixed associated type bindings be specified here, or should dynamic dispatch for associated-type traits remain a later trait-system RFC?
- Should associated type equality constraints be introduced separately, or can APIs in this RFC rely on projection types and narrower traits without equality syntax?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
