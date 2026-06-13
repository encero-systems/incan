# RFC 099: Generic trait-targeted methods

- **Status:** Draft
- **Created:** 2026-05-15
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 009 (sized integers)
    - RFC 017 (validated newtypes with implicit coercion)
    - RFC 025 (multiple generic trait instantiations)
    - RFC 042 (traits are always abstract)
    - RFC 043 (Rust trait implementation from Incan)
    - RFC 056 (`std.io`)
    - RFC 091 (constrained integer newtype storage carriers)
    - RFC 098 (native associated types for traits)
    - RFC 101 (`std.collections.OrdinalMap`)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/581
- **Blocks:** https://github.com/dannys-code-corner/incan/issues/596 (v0.5 RFC 101 trait-system bridge removal)
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC introduces generic trait-targeted methods: methods declared inside the owning type body may carry their own type parameters, target a trait instantiation with `for Trait[...]`, and add `where` constraints that make the method define a checked family of trait adoptions rather than one concrete adoption at a time. It also defines how trait-owned capability families can make an existing deterministic type family satisfy a trait without adding Rust-shaped external implementation blocks. The primary motivation is numeric and storage-carrier dispatch: `std.io.BytesIO` should be able to define one `write[T ... for BinaryWrite[T] ...]` method family for exact-width integers and another for storage-backed integer newtypes, while traits such as `OrdinalKey` should be able to own deterministic key conformance for language scalar families.

## Core model

1. **Behavior lives on the owning type:** when a class, model, enum, newtype, or rusttype owns behavior, generic trait-targeted methods are written in that declaration body.
2. **The method header names the trait target:** `for TraitExpression` inside the method generic header says the method satisfies that trait instantiation, not merely that it is an inherent overload.
3. **Generic constraints are part of the adoption family:** bounds such as `T with StorageBackedInteger` and `where T.Storage with FixedWidthInteger` decide which static type arguments the adoption family covers.
4. **No Rust `impl` syntax leaks into Incan:** the backend may emit Rust `impl` blocks, but source uses class bodies, `with`, `for`, and `where`.
5. **Storage dispatch is explicit:** a storage-backed newtype may delegate representation work through `T.Storage`, but the source-level type remains the nominal newtype and no ambient primitive conversion is created.
6. **Overlaps must be diagnosed:** if two generic trait-targeted methods could satisfy the same trait instantiation for the same receiver type and type arguments, the compiler must reject the ambiguity unless another accepted RFC defines specialization.
7. **Traits may own capability families:** when a trait defines a canonical contract over a finite or registry-backed language type family, the trait can expose that conformance through checked capability metadata instead of reopening each existing type declaration.

## Motivation

RFC 009 made exact-width numeric types first-class. RFC 056 then used trait-backed numeric I/O so `BytesIO` could read and write `u8`, `i8`, `u16`, `i16`, `u32`, `i32`, `u64`, `i64`, `u128`, `i128`, `f32`, and `f64` without exposing a public matrix of method names. That design is right, but the current language still pressures implementations toward repeated one-type-at-a-time adoptions, hardcoded builtin dispatch, or backend-specific escape hatches. The language has the vocabulary for generic bounds and method-level trait targeting, but it lacks a source form for saying "this class method satisfies a whole generic family of trait instantiations."

RFC 091 adds a second pressure point. A constrained integer newtype such as `Month = newtype int[ge=1, le=12, storage=u8]` is logically a `Month`, not a `u8`, but representation-oriented traits such as binary writing often want to use the compact storage carrier once validation has already happened. The feature needed here is not "treat `Month` as a `u8`." It is a checked way for a method family to say that storage-backed values can participate in a trait when their storage carrier participates in the corresponding representation trait.

Incan should solve owned behavior in the class-body model users already know. Python's class model teaches that a class bundles data and functionality, and Incan's class docs follow that direction: classes are behavior-first, methods are the primary API, and `extends` is reserved for inheritance and method reuse. Adding an external `impl` or `extend Type with Trait` block would solve the backend problem while making the authoring surface less Incan. The language should instead let `BytesIO` teach existing `BytesIO` values the generic trait behavior by putting the method on `BytesIO` itself.

RFC 101 exposed the complementary case. `std.collections.OrdinalKey` owns a deterministic key contract for existing language scalar families such as `str`, `bytes`, exact-width integers, decimal, UUID, dates, and stable value enums. Reopening each type declaration is the wrong mental model, and external `impl` blocks would import Rust's coherence surface into Incan source. The trait should own the canonical conformance family, while the compiler proves membership from language registries, associated type metadata, and the trait contract.

## Goals

- Allow methods in concrete type bodies to declare method-level generic parameters and target a generic trait instantiation in the same method header.
- Allow generic trait-targeted methods to establish a family of trait adoptions for the enclosing type.
- Add method-local `where` constraints for projected associated types and concrete receiver capability checks needed by those adoption families.
- Support numeric family dispatch for exact-width integer and float helper traits without requiring a hand-authored method per primitive type.
- Support storage-carrier dispatch for constrained integer newtypes without weakening RFC 091's logical-type versus storage-carrier boundary.
- Support trait-owned capability families for deterministic registry-backed contracts such as stable ordinal keys.
- Preserve the existing `with TraitName` and method-level `for TraitName` mental model from RFC 043.
- Reject overlapping adoption families clearly rather than adding specialization by accident.
- Preserve trait adoption families, where-constraint proofs, trait-owned capability-family membership, selected substitutions, and overlap diagnostics in checked metadata, generated docs, LSP views, and package manifests.

## Non-Goals

- This RFC does not add Rust-shaped `impl Trait for Type` source syntax.
- This RFC does not add external class reopening or monkey-patching as a general language feature.
- This RFC does not make `extends` mean trait adoption or method injection; `extends` remains class inheritance and behavior reuse.
- This RFC does not change the representation of bare `int`, which remains the RFC 009 alias for `i64`.
- This RFC does not make storage-backed newtypes subtype or implicitly convert to their storage carrier.
- This RFC does not introduce trait specialization or overlapping implementation precedence.
- This RFC does not require generic associated types.
- This RFC does not add arbitrary package monkey-patching for existing types. Trait-owned capability families are canonical contracts owned by the trait and checked by the compiler, not per-package alternate implementations.

## Guide-level explanation

A class that owns behavior can define a generic trait-targeted method directly in its body. The method's generic header lists the method type parameter, the trait target, and any extra constraints that determine when the adoption family exists:

```incan
class BytesIO:
    def write[T with FixedWidthNumeric for BinaryWrite[T]](
        mut self,
        value: T,
        endian: Endian,
    ) -> Result[None, IoError]:
        ...
```

This reads as: `BytesIO` supports `BinaryWrite[T]` for every `T` that satisfies `FixedWidthNumeric`, and this method is the `write` method for that trait target. Users still call it like an ordinary method:

```incan
buf = BytesIO()
value: u32 = 42
buf.write(value, Endian.Little)?
```

The same shape handles storage-backed integer newtypes without pretending the newtype is its storage carrier. A storage-backed type exposes a storage associated type through RFC 098-style projection, and the method family can require that storage type to be writable:

```incan
type Month = newtype int[ge=1, le=12, storage=u8]

class BytesIO:
    def write[
        T with StorageBackedInteger
        for BinaryWrite[T]
        where T.Storage with FixedWidthInteger
        where BytesIO with BinaryWrite[T.Storage]
    ](
        mut self,
        value: T,
        endian: Endian,
    ) -> Result[None, IoError]:
        storage: T.Storage = value.to_storage()
        return self.write(storage, endian)
```

The declaration does not say `Month` is a `u8`. It says `BytesIO` can write a `Month` because `Month` has a storage carrier, that carrier is a fixed-width integer, and `BytesIO` already knows how to write that carrier. The explicit projection keeps the representation decision visible in source while preserving the domain type at the API boundary.

The feature is also useful outside `std.io`. A codec, hasher, serializer, graph store, or binary protocol writer can write one method family for a trait capability instead of copying the same method for every numeric width or storage-backed domain type:

```incan
class FrameWriter:
    def put[
        T with StorageBackedInteger
        for FrameEncode[T]
        where T.Storage with FrameEncode
    ](mut self, value: T) -> Result[None, FrameError]:
        return self.put(value.to_storage())
```

Plain class inheritance stays separate:

```incan
class CountingBytesIO extends BytesIO:
    bytes_written: int
```

`extends` creates a new class with inherited behavior. Generic trait-targeted methods add checked behavior to the class that contains the method declaration.

## Reference-level explanation

### Method header form

A concrete type body may contain a generic trait-targeted method with the form `def name[TypeParams for TraitExpression where Constraints...](params...) -> ReturnType:`. The `TypeParams` portion follows the existing method generic parameter rules. The optional `for TraitExpression` inside the generic header targets the method at a trait instantiation that may refer to the method's type parameters. The optional `where` clauses introduce additional constraints used to decide whether the trait-targeted method applies for a particular substitution.

The `for TraitExpression` form inside the generic header is distinct from the existing concrete method target form `def name(params...) for TraitExpression -> ReturnType:`. The existing form remains valid when the trait target does not need method-local type parameters. A method must not use both target forms at once. If the trait target mentions a method-local type parameter, the target must appear inside the generic header so that the parameter is in scope for the target.

### Adoption-family semantics

A generic trait-targeted method in a concrete type body establishes a trait adoption family for the enclosing type. For each substitution of the method type parameters that satisfies the method's bounds and `where` constraints, the enclosing type is considered to adopt the targeted trait instantiation, and the method body satisfies the corresponding trait method. The compiler must check the method signature against the targeted trait method after substituting the method type parameters, associated type projections, and constraints.

The enclosing type does not need to list every covered trait instantiation in its declaration header. For example, `BytesIO` should not have to spell `with BinaryWrite[u8], BinaryWrite[i8], BinaryWrite[u16], ...` when a single generic trait-targeted method covers the exact-width numeric family. The adoption family is still explicit because it is attached to a method that names `for BinaryWrite[T]` and declares the constraints on `T`.

### Trait-owned capability families

Some traits define a canonical contract for an existing language type family rather than behavior owned by one class body. `OrdinalKey` is the motivating example: the trait owns the deterministic key encoding contract, and `str`, `bytes`, exact-width integers, decimal, UUID, civil date/time values, and stable scalar value enums satisfy that contract when their language-defined representation has a deterministic encoding.

The compiler must model these as checked trait conformance facts, not as ad hoc name checks in each caller. A trait-owned capability family has:

- a fully qualified trait identity;
- a finite or registry-backed type-family predicate;
- required methods, associated type bindings, or constants supplied by the trait contract and language metadata;
- deterministic overlap rules so a type has at most one visible conformance for the same trait instantiation.

Source should not spell Rust-style external implementation blocks for this. The trait owns the intent, the compiler proves that a concrete type is in the supported family, and backend lowering emits whatever target representation is required. If the proof depends on associated types, the associated type bindings must be exposed through the same implementation metadata described by RFC 098.

### Where constraints

A `where` constraint in a method generic header may express trait conformance for a type parameter, an associated type projection, or a concrete type expression. Examples include `where T.Storage with FixedWidthInteger` and `where BytesIO with BinaryWrite[T.Storage]`. A `where` constraint must be checked statically and must not be treated as a runtime guard. If the compiler cannot prove a `where` constraint for a candidate substitution, that substitution does not satisfy the adoption family.

This RFC defines method-local `where` constraints only for generic trait-targeted methods. A later RFC may generalize `where` clauses to functions, type aliases, class declarations, and trait declarations, but this RFC does not require that larger surface.

### Storage-backed integer dispatch

Storage-backed constrained integer newtypes may expose a trait-owned `StorageBackedInteger` capability. That capability should include an associated type named `Storage`, whose binding is the exact-width integer storage carrier declared by `storage=<integer-carrier>`, and a value-level method that lets already-validated values expose their storage representation for representation-oriented work. A type such as `type Month = newtype int[ge=1, le=12, storage=u8]` therefore satisfies `StorageBackedInteger` with `type Storage = u8`.

Using `T.Storage` in a generic trait-targeted method must not create ambient primitive conversion. A `Month` remains a `Month`, a `u8` remains a `u8`, and a method that delegates through `value.to_storage()` is explicitly choosing a representation path for an already-validated value. Reconstructing a storage-backed newtype from storage data must continue to use the checked newtype construction path so storage carrier values that are representable but semantically invalid still fail validation.

### Coherence and ambiguity

The compiler must reject overlapping generic trait-targeted methods when two methods could satisfy the same trait instantiation for the same enclosing type and substituted type arguments. This RFC does not introduce specialization, priority ordering, or "more specific wins" selection. If a future RFC adds specialization, it may relax this rule with explicit precedence semantics, but the default behavior must be ambiguity rejection.

Call resolution must continue to treat trait-targeted methods as trait behavior, not arbitrary overloads. A direct method call may use the targeted method when existing trait-method resolution can identify the relevant trait instantiation from argument types, expected result type, or an explicit target. If no target can be proven, the compiler must report an ambiguity or missing-method diagnostic rather than trying every method with a matching name as a runtime overload set.

### Metadata and inspection contract

Generic trait-targeted methods must produce inspectable trait adoption facts. Checked API metadata, package manifests, generated documentation, semantic inspection, and LSP surfaces must be able to expose at least:

- the enclosing type that owns the method family;
- the targeted trait instantiation, including method-local type parameters;
- bounds and `where` constraints that decide whether a substitution belongs to the adoption family;
- associated type projections and storage-carrier relationships used by the family;
- trait-owned capability-family predicates and registry-backed membership sources;
- overlap-rejection facts when two adoption families would satisfy the same trait target;
- the concrete substitution selected for a resolved method call when enough type information is available.

These facts are part of the public semantic contract. A package consumer, generated-doc reader, LSP hover, or explanation command should be able to answer why `BytesIO` satisfies `BinaryWrite[Month]`, which storage carrier made that possible, and which constraint would fail if the same call were attempted for a non-storage-backed type. The metadata must preserve the distinction between authored adoption families, trait-owned capability families, and backend lowering choices so generated Rust does not become the only way to understand the behavior.

## Design details

### Syntax

The compact one-line form is:

```incan
def write[T with StorageBackedInteger for BinaryWrite[T] where T.Storage with FixedWidthInteger](...) -> Result[None, IoError]:
    ...
```

The multiline form is preferred when the header contains more than one constraint:

```incan
def write[
    T with StorageBackedInteger
    for BinaryWrite[T]
    where T.Storage with FixedWidthInteger
    where BytesIO with BinaryWrite[T.Storage]
](
    mut self,
    value: T,
    endian: Endian,
) -> Result[None, IoError]:
    ...
```

The `for` clause belongs inside the generic header because the target trait instantiation may use method type parameters. The `where` clauses also belong in the generic header because they constrain the adoption family, not the runtime body. No colon appears inside the bracket list; `:` remains the block introducer after the return type.

### Numeric family traits

The standard library or builtin trait surface should define numeric family traits over the RFC 009 registry rather than forcing each numeric helper surface to discover widths by string matching. This RFC uses illustrative names such as `FixedWidthInteger`, `FixedWidthFloat`, `FixedWidthNumeric`, and `StorageBackedInteger`; the final names should align with the existing stdlib trait naming style before this RFC moves out of Draft.

The important contract is registry-backed membership. If a trait claims to represent fixed-width integers, its membership must come from the numeric registry introduced by RFC 009, not from duplicated lists in each library. Aliases such as `byte` and `integer` should resolve to their canonical numeric identities before family membership is checked, so alias spelling does not create extra adoption families.

The same registry-backed model applies outside numeric I/O. A trait such as `OrdinalKey` may define a deterministic key family over language scalar registries and exact stdlib scalar metadata. That does not make `str` or `u32` reopenable; it means the trait contract and registry facts together establish one canonical conformance.

### Interaction with `std.io`

RFC 056 deliberately made numeric reads and writes trait-backed rather than a public matrix of method names. Generic trait-targeted methods are the missing authoring surface for that design. `BytesIO` can define one method family for fixed-width numeric writes, one for fixed-width numeric reads where the result type is expected, and one for storage-backed integer newtypes that delegate through their storage carriers.

`int` remains an alias for `i64`, so an `int` value should satisfy the same numeric family membership as `i64` after alias normalization. This does not create a separate `BinaryWrite[int]` implementation alongside `BinaryWrite[i64]`; the trait target should canonicalize through the same alias rules RFC 009 uses for type identity.

### Interaction with associated types

This RFC relies on RFC 098-style associated type projection for `T.Storage`. If RFC 098 changes the projection spelling, the examples and reference rules in this RFC must update to match the accepted projection syntax. Generic associated types are not required because `Storage` is one concrete storage carrier per storage-backed integer type.

### Interaction with class inheritance

`extends` remains the class inheritance surface. It creates a new class that inherits fields and methods for behavior reuse and overrides; it does not define trait adoption for an existing class. Generic trait-targeted methods are written in the body of the type that owns the behavior. This keeps the Python-shaped class story clear: use `extends` when defining a new class from an existing class, and use methods in the class body when defining what the class itself can do.

### Interaction with Rust interop

The Rust backend may lower generic trait-targeted methods to generic Rust `impl` blocks when that is the natural target representation, but Incan source must not expose Rust `impl Trait for Type` syntax. Foreign trait and foreign type coherence rules from RFC 043 continue to apply. A generic trait-targeted method must be rejected if its target would require an invalid foreign-trait-for-foreign-type implementation after lowering.

### Diagnostics

Diagnostics should name the trait target, enclosing type, and failed substitution. Useful diagnostics include "generic trait-targeted method overlaps another adoption family," "where constraint `T.Storage with FixedWidthInteger` is not satisfied," "method target `BinaryWrite[T]` is not a trait method target," "method signature is incompatible with targeted trait method," and "storage carrier projection requires `T with StorageBackedInteger`." Diagnostics should avoid suggesting Rust `impl` syntax as the primary fix.

## Alternatives considered

An external `impl Trait for Type` block would be familiar to Rust users and easy to map to generated Rust, but it would violate the Incan direction set by RFC 043: users adopt capabilities with `with` and write behavior in type bodies. It would also split class behavior across declarations in a way that feels less Pythonic for owned types such as `BytesIO`.

An external `extend Type with Trait` block is more Incan-looking than `impl`, but it overloads `extend`/`extends` terminology that already belongs to class inheritance and method reuse. It also blurs whether the feature is reopening a class, adding methods, or declaring trait conformance. This RFC keeps the primary authoring model in the owning type body.

Repeating concrete adoptions for every numeric width would avoid new syntax, but it scales poorly and reintroduces exactly the boilerplate RFC 056 tried to avoid. It also leaves storage-backed newtypes without a principled representation-delegation path.

Letting storage-backed newtypes behave as their storage carrier would make many calls shorter, but it breaks RFC 091's core safety boundary. Storage is representation metadata, not the source-level type. This RFC requires explicit projection through `T.Storage` and value-level conversion through the storage-backed capability.

Adding general method overloading would also solve some call-site ergonomics, but it is too broad for this problem. The needed feature is checked trait conformance over a generic family, not arbitrary runtime-style overload selection by parameter shape.

## Drawbacks

Generic trait-targeted methods make method headers more expressive and therefore more complex. A header such as `def write[T with StorageBackedInteger for BinaryWrite[T] where T.Storage with FixedWidthInteger](...)` carries type parameters, a trait target, and constraints in one place. That is acceptable for capability-heavy stdlib code, but docs should continue to show ordinary methods first and reserve this form for APIs that truly need generic trait adoption families.

The trait solver also has more work to do. It must reason about adoption families, associated type projections, alias-normalized numeric family membership, and overlap detection. The benefit is that this work becomes central and checked instead of being duplicated across stdlib modules and backend codegen branches.

Storage-carrier dispatch may tempt users to think representation and semantics are interchangeable. The language and docs must be blunt: storage dispatch is a representation-oriented delegation tool, not a permission to bypass validation or treat domain newtypes as primitive numbers.

## Implementation architecture

This section is non-normative. A practical implementation should model generic trait-targeted methods as checked trait adoption families attached to the enclosing type's metadata. During typechecking, the method's type parameters, target trait expression, and `where` constraints should be stored together so trait conformance queries can ask whether a concrete trait instantiation is covered. Trait-owned capability families should be stored in the same conformance model, keyed by trait identity and type-family predicate, so generic bounds, method lookup, lowering, metadata export, and diagnostics read the same facts. Backend lowering may then emit a target-language generic implementation, a finite expansion for registry-known families, or another representation that preserves the same checked semantics.

Storage-backed constrained newtypes should expose their storage carrier through checked metadata produced by RFC 091. The storage-backed capability should be derived from that metadata rather than requiring users to restate `type Storage = ...` manually for every storage-backed newtype.

## Layers affected

- **Parser / AST**: method generic headers must accept `for TraitExpression` and method-local `where` constraints.
- **Typechecker / Symbol resolution**: concrete type metadata must record generic trait-targeted adoption families, trait metadata must record trait-owned capability families, method signatures must be checked against targeted trait methods, `where` constraints must be evaluated, and overlapping families must be rejected.
- **IR Lowering**: lowered method metadata must preserve the targeted trait instantiation and generic constraints so backend dispatch remains checked.
- **Emission**: Rust emission should lower eligible generic trait-targeted methods to valid Rust trait implementations or equivalent generated code while respecting coherence.
- **Stdlib / Runtime (`incan_stdlib`)**: numeric family traits and storage-backed integer traits should be exposed where `std.io`, codecs, serializers, and related APIs need them.
- **Formatter**: multiline generic method headers with `for` and repeated `where` clauses must format stably.
- **Library manifests / checked API metadata**: public adoption-family metadata must preserve trait targets, constraints, capability-family predicates, substitutions, overlap diagnostics, and provenance.
- **LSP / Tooling**: hover, completion, go-to-definition, diagnostics, semantic inspection, and explanation commands should surface the trait target, adoption family constraints, concrete substitutions, and reasons a substitution was accepted or rejected.
- **Documentation**: trait, class, numeric, newtype, and `std.io` docs should explain when generic trait-targeted methods are appropriate.

## Unresolved questions

- Should the target clause be spelled exactly as `for BinaryWrite[T]` inside the generic header, or should the language choose a different keyword to avoid overloading the existing method-level `for Trait` target?
- Should method-local `where` constraints be limited to generic trait-targeted methods in this RFC, or should the RFC introduce a general `where` surface for all generic functions and type declarations?
- What are the final stdlib names for numeric family traits such as `FixedWidthInteger`, `FixedWidthNumeric`, and `StorageBackedInteger`?
- Should storage-backed integer newtypes expose a standard `to_storage()` method, a property, or a more explicit representation adapter?
- Should binary reading for storage-backed newtypes be included in the same design, and if so, what error type should construction-from-storage use when the storage value is representable but violates the newtype's semantic constraints?
- Should alias-normalized types such as `int` and `i64` appear as one trait instantiation in diagnostics or preserve the authored spelling where possible?
- What exact source spelling, metadata annotation, or trait declaration shape should a trait use to declare a trait-owned capability family, and which families are accepted for the RFC 099 implementation milestone?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
