# RFC 091: Constrained integer newtype storage carriers

- **Status:** Draft
- **Created:** 2026-05-07
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 009 (sized integers)
    - RFC 017 (validated newtypes with implicit coercion)
    - RFC 058 (`std.datetime`)
    - RFC 085 (field metadata and type-shaped constraints)
    - RFC 088 (iterator adapter surface)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/537
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC lets constrained integer newtypes keep Incan's logical `int` semantics at the source level while choosing an exact-width storage carrier for their generated representation. A type such as `Month = newtype int[ge=1, le=12, storage=u8]` remains a nominal validated integer-like domain type, accepts ordinary integer inputs through the RFC 017 validated-newtype boundary, and must never silently truncate or wrap values, but its backing storage may use `u8` instead of the default `int` carrier.

## Core model

1. **Logical type and storage carrier are separate:** the left side of the constrained primitive describes the source-level value domain, while `storage` describes the backing integer carrier used after validation.
2. **Validation remains authoritative:** bounds such as `ge`, `gt`, `le`, and `lt` define the allowed values; the storage carrier does not add hidden validation rules.
3. **No ambient primitive conversion:** the compiler may insert validated newtype coercions at RFC 017 sites, but must not use this feature as a reason to insert unrelated primitive conversions or lossy casts.
4. **Representability is statically checked:** a storage carrier is valid only when every value allowed by the declared constraints fits in that carrier.
5. **Arithmetic must preserve invariants:** operations that reconstruct a storage-backed newtype must re-enter the same checked construction path rather than relying on carrier overflow behavior.

## Motivation

Validated newtypes already give Incan good domain modeling for primitive values: `PositiveInt`, `RetryAttempts`, `UserId`, and similar types can validate inputs while remaining cheap and explicit. The gap is that the logical primitive and the storage primitive are currently the same thing. A constrained integer whose valid range is `1..12` should not need to store an `i64` just because the public source-level concept is "an integer with a narrow valid domain."

This matters for dense data models, standard-library domain types, schema-backed values, and future purpose-built libraries that carry many small integer fields. `std.datetime` is a concrete example: `Month`, `Hour`, `Minute`, `Second`, and `Nanosecond` all have small, deterministic value domains, but the user-facing surface should still feel like ordinary integer construction and validation rather than manual resizing into exact-width primitive types.

The feature also keeps Incan honest about where safety belongs. Users should be able to say "this is an integer domain type stored compactly" without opening the door to ambient `int -> u8` truncation. The compiler should check that the storage carrier can hold the declared domain and then generate the necessary validated construction.

## Goals

- Allow constrained integer newtypes to declare an exact-width integer storage carrier.
- Preserve RFC 017 validated-newtype coercion ergonomics for storage-backed constrained newtypes.
- Reject storage carriers that cannot hold every value allowed by the declared constraints.
- Keep bare `int` semantics unchanged.
- Make compact domain types suitable for standard-library values such as datetime components and for user-defined dense model fields.
- Preserve clear diagnostics for invalid constraints, unsupported storage carriers, and values that fail validation.

## Non-Goals

- This RFC does not introduce ambient primitive auto-resizing, truncation, wrapping, or saturation.
- This RFC does not change the representation of bare `int`.
- This RFC does not require the compiler to infer the smallest possible carrier automatically.
- This RFC does not allow constrained exact-width primitive underlyings such as `i8[ge=1, le=12]`; exact-width primitives are storage carriers in this RFC, not constrained source-level domains.
- This RFC does not introduce bitfields, packed model layout, or C ABI layout guarantees.
- This RFC does not make constrained primitives universal stand-alone types outside the validated-newtype model.
- This RFC does not define floating-point, decimal, or arbitrary-precision storage carriers.
- This RFC does not decide timezone, calendar, or locale semantics for datetime APIs.

## Guide-level explanation

Users define compact domain integers by writing a constrained integer newtype and adding a storage carrier:

```incan
type Month = newtype int[ge=1, le=12, storage=u8]
type Hour = newtype int[ge=0, lt=24, storage=u8]
type Nanosecond = newtype int[ge=0, lt=1000000000, storage=u32]
```

At normal Incan call sites, these values are still used as validated newtypes. A raw integer may flow into a `Month` where RFC 017 permits implicit validated newtype coercion, and invalid values fail through the same validation mechanism as any other constrained newtype:

```incan
type Month = newtype int[ge=1, le=12, storage=u8]

def bill_for(month: Month) -> None:
    return

def main() -> None:
    bill_for(4)
    bill_for(13)
```

The first call constructs a valid `Month`. The second call fails validation because `13` is outside the declared domain. The failure is about the domain constraint, not about an attempted primitive cast.

The storage carrier is not a shortcut for writing constraints. This is invalid because the declared domain admits values too large for `u8`:

```incan
type Count = newtype int[ge=0, storage=u8]
```

The correct declaration must make the source-level domain fit the carrier:

```incan
type Count = newtype int[ge=0, le=255, storage=u8]
```

Using an unsigned carrier may be useful even when it does not save bytes. For example, `storage=u64` stores the same width as the default `i64`-shaped `int` carrier, but it can be a better backing representation for a non-negative domain that needs the full positive range. Compactness comes from smaller carriers such as `u8`, `u16`, and `u32`.

## Reference-level explanation

A constrained integer newtype may declare a storage carrier inside the constrained primitive bracket list using a storage attribute, drafted here as `storage=<integer-carrier>`. The storage carrier must be one of the supported exact-width integer primitive types: `i8`, `i16`, `i32`, `i64`, `i128`, `u8`, `u16`, `u32`, `u64`, or `u128`. The storage attribute must be accepted only on constrained `int` newtype underlyings. It must not be accepted on bare `int`, unconstrained `int` outside a newtype declaration, `float`, `decimal`, `str`, `bytes`, collection types, model fields directly, function parameters directly, arbitrary type expressions, or exact-width primitive underlyings such as `i8[ge=1, le=12]`. If a constrained integer newtype omits `storage`, it must retain existing behavior.

The storage attribute must not be treated as a validation constraint. The declared comparison constraints define the source-level domain, and the compiler must reject a storage-backed newtype when those constraints do not prove that every valid source-level value fits in the storage carrier. The compiler must reject duplicate storage attributes, unsupported storage carrier names, contradictory constraints, and storage carriers whose value domain is incompatible with the declared constraints. For example, `int[ge=0, storage=u8]` must be rejected because values greater than `255` are valid according to the declared constraints but cannot be represented by `u8`.

The source-level type of a storage-backed newtype remains the nominal newtype, not the carrier primitive. A `Month` is not a `u8`, and a `u8` is not automatically a `Month`. RFC 017 implicit validated-newtype coercion may apply to storage-backed constrained integer newtypes at the same sites where it applies to other validated newtypes, and that coercion must validate the source-level constraints before constructing the newtype. The compiler must not insert ambient primitive conversions because a storage carrier exists. In particular, a general `int` expression must not silently become `u8`, `u16`, or any other exact-width primitive unless an existing explicit conversion or checked validated-newtype path requires it.

Integer literals flowing into a storage-backed newtype should receive precise compile-time diagnostics when their value is statically known to fail the declared constraints. Runtime integer values flowing into a storage-backed newtype must use the same validation behavior as other constrained newtypes. If the value fails validation, the failure mode must match RFC 017 for the relevant construction site. Operations that produce a storage-backed newtype must not use storage overflow semantics as the domain semantics. Reconstructing the newtype from an arithmetic result must validate the logical domain.

Serialization, reflection, library manifests, and generated metadata should preserve both the logical constrained type and the storage carrier so that downstream compilation and tooling see the same contract. A storage carrier does not by itself promise stable C ABI layout or packed model layout.

## Design details

The proposed spelling keeps storage in the same bracket block as the domain constraints because it modifies the constrained primitive's backing storage, and the `storage` value is a type name parsed as a bare identifier from the exact-width integer primitive set:

```incan
type Port = newtype int[ge=0, le=65535, storage=u16]
type SmallDelta = newtype int[ge=-128, le=127, storage=i8]
```

This RFC intentionally uses `int[...]` as the source-level constrained domain and exact-width primitives as storage carriers. `type Month = newtype i8[ge=1, le=12]` is not the same contract: it exposes `i8` as the source-level underlying type, so ordinary `int` input would need an ambient primitive conversion before validation. That is out of scope for this RFC. The storage-backed spelling is `type Month = newtype int[ge=1, le=12, storage=i8]`.

Representability checking is based on the effective lower and upper bounds of the declared constraints. For `int[ge=0, le=255, storage=u8]`, the source-level domain is `0..255`, so every value fits in `u8`; for `int[gt=-1, lt=256, storage=u8]`, the source-level domain is also `0..255`, so it also fits in `u8`. Open-ended domains must be checked against the logical `int` domain: `int[ge=0, storage=u64]` is representable because the logical `int` domain's non-negative values fit in `u64`, while `int[ge=0, storage=u32]` is not representable because the logical `int` domain admits non-negative values greater than `u32` can represent.

The storage carrier should be preserved as part of the newtype's checked API metadata so documentation and schema tools can show both the semantic domain and the storage choice without guessing from generated output. `storage` should not change field-level validation: a model field of type `Month` still validates through `Month`, and a model field of type `int[ge=1, le=12, storage=u8]` is outside this RFC because constrained primitives remain intended as newtype underlyings.

For datetime-style domain types, this RFC enables compact definitions without weakening cross-field validation:

```incan
type Month = newtype int[ge=1, le=12, storage=u8]
type DayOfMonth = newtype int[ge=1, le=31, storage=u8]

model Date:
    year: int
    month: Month
    day: DayOfMonth
```

`DayOfMonth` can express the standalone day range, but a checked `Date` constructor must still validate that the day exists in the given year and month. Storage carriers do not replace semantic validation that depends on multiple fields.

## Alternatives considered

### Use exact-width primitive types directly

Users can already write exact-width primitives such as `u8` and `u32`, but direct primitive fields do not carry domain names, validation hooks, or RFC 017 boundary behavior. `month: u8` also accepts `0`, which is not a valid month. Domain newtypes are the right surface; exact-width primitives are the right carrier.

### Make `int[ge=0]` automatically choose an unsigned carrier

Automatic carrier selection hides an ABI-relevant and tooling-relevant choice behind validation syntax. It also creates compatibility risk when a later constraint change silently changes layout. This RFC keeps storage explicit.

### Treat the carrier as an implicit additional bound

Letting `int[ge=0, storage=u8]` mean `0..255` is terse, but it makes storage metadata change the semantic domain. That would be surprising in API docs and dangerous when users change representation for performance. This RFC requires the semantic bounds to be written explicitly unless the open-ended logical domain is already provably representable by the carrier.

### Use `type=u64`

`type` is misleading because the source-level type is still a constrained `int` newtype. The carrier is a storage detail with observable code-generation and metadata consequences, so `storage` is clearer.

### Add model field storage metadata

Field-level storage metadata would let users write `month: int { storage=u8 }`, but that duplicates type-shaped constraints and weakens reuse. If `Month` is a meaningful domain concept, the storage and validation should travel with the type.

## Drawbacks

This adds another dimension to constrained primitive syntax. Users must understand the difference between logical constraints and storage carriers.

Storage carriers make generated layout more intentional, which means changing `storage` can become a compatibility concern for libraries that expose generated Rust or serialized representations.

The compiler must prove representability from constraints. That proof is straightforward for integer comparison constraints, but it becomes more complex if future RFCs add richer numeric predicates.

The feature can be overused. Not every domain integer needs compact storage, and `storage` should not become noise in ordinary model declarations.

## Implementation architecture

This section is non-normative. The recommended shape is to keep the constrained primitive as the user-facing semantic type, preserve a separate storage-carrier annotation in checked type metadata, validate carrier admissibility during semantic analysis, and lower the newtype backing storage to the chosen carrier only after the compiler has proven that every validated value is representable. Construction should continue to flow through the validated-newtype path, with generated conversions occurring after validation rather than by exposing lossy primitive casts at the source level.

## Layers affected

- **Parser / AST**: constrained primitive bracket syntax must accept one storage-carrier attribute alongside comparison constraints.
- **Typechecker / Symbol resolution**: constrained integer newtypes must validate carrier names, duplicate storage attributes, constraint consistency, representability, and RFC 017 coercion behavior.
- **IR Lowering**: storage-backed newtypes must carry the logical domain and the chosen backing carrier into lowered form.
- **Emission**: generated Rust output must store the newtype using the selected exact-width carrier while preserving checked construction semantics.
- **Stdlib / Runtime (`incan_stdlib`)**: validation error rendering and numeric conversion helpers may need to support storage-backed construction paths.
- **Formatter**: constrained primitive formatting must preserve the storage attribute in a stable order.
- **LSP / Tooling**: hover, completion, diagnostics, and metadata extraction should surface the logical constrained type and the storage carrier distinctly.

## Unresolved questions

- Should `storage` support `isize` and `usize`, or should the first version intentionally restrict carriers to target-stable exact-width integer primitives?
- Should storage carriers for constrained `float` newtypes be included in this RFC or left to a follow-up after integer carriers ship?
- Should reflection APIs expose the storage carrier by default, or should this remain compiler and manifest metadata only?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
