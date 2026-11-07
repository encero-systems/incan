# RFC 091: Integer storage carriers

- **Status:** Planned
- **Created:** 2026-05-07
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 009 (sized integers)
    - RFC 017 (validated newtypes with implicit coercion)
    - RFC 058 (`std.datetime`)
    - RFC 085 (field metadata and type-shaped constraints)
    - RFC 088 (iterator adapter surface)
    - RFC 121 (unified Incan/Rust type substrate)
- **Issue:** [#537](https://github.com/encero-systems/incan/issues/537)
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

An Incan integer type has a storage carrier, and `int` is the default: the built-in `int` means `newtype Int[storage=i64]`. This RFC lets an author select a different exact-width carrier, narrower or wider than `i64`, with `int[storage=i8]`, `type Month = newtype int[ge=1, le=12, storage=u8]` or `type bigint = newtype int[storage=i128]`. The carrier sets how a value is stored and how far its range reaches, written bounds narrow that range, and a value must never silently truncate or wrap.

## Core model

1. **`int` is the default, not a special case:** `int[...]` without `storage` keeps the `i64` carrier, and `storage` replaces it.
2. **The carrier sets the range and bounds narrow it:** an integer type admits its carrier's values, narrowed by any `ge`, `gt`, `le` or `lt` that is written. A side with no written bound reaches the carrier's limit.
3. **A written bound must fit its carrier:** this is statically checked.
4. **No ambient primitive conversion:** the compiler may insert validated coercions at RFC 017 sites, but must not use this feature as a reason to insert unrelated primitive conversions or lossy casts.
5. **Arithmetic must preserve the range:** operations that reconstruct a storage-backed type must re-enter the same checked construction path rather than relying on carrier overflow behavior.

## Motivation

Validated newtypes already give Incan good domain modeling for primitive values: `PositiveInt`, `RetryAttempts`, `UserId`, and similar types can validate inputs while remaining cheap and explicit. The gap is that every one of them stores an `i64`. A constrained integer whose valid range is `1..12` should not need eight bytes because the public source-level concept is "an integer with a narrow valid domain."

This matters for dense data models, standard-library domain types, schema-backed values, and purpose-built libraries that carry many small integer fields. `std.datetime` is a concrete example: `Month`, `Hour`, `Minute`, `Second`, and `Nanosecond` all have small, deterministic value domains, but the user-facing surface should still feel like ordinary integer construction and validation rather than manual resizing into exact-width primitive types.

The same gap exists in the other direction. A value that needs more than 64 bits, such as a large counter or an amount in very small units, should not have to give up ordinary integer input and validated construction to get there. Treating `i64` as one carrier among several covers both directions with one rule.

The feature also keeps Incan honest about where safety belongs. Users should be able to say "this is an integer stored in this carrier" without opening the door to ambient `int -> u8` truncation.

## Goals

- Allow any integer type written with `int[...]` to select an exact-width integer carrier, narrower or wider than `i64`.
- Define an integer type's range once: the carrier's range, narrowed by written bounds.
- Preserve RFC 017 validated-newtype coercion ergonomics for storage-backed newtypes.
- Reject written bounds that the carrier cannot hold.
- Keep `int` itself stored as `i64`.
- Make compact domain types suitable for standard-library values such as datetime components and for user-defined dense model fields, and make wide integer types as easy to declare as narrow ones.
- Preserve clear diagnostics for invalid constraints, unsupported storage carriers, and values that fail validation.

## Non-Goals

- This RFC does not introduce ambient primitive auto-resizing, truncation, wrapping, or saturation.
- This RFC does not change the representation of `int`.
- This RFC does not require the compiler to infer the smallest possible carrier automatically.
- This RFC does not allow constrained exact-width primitive underlyings such as `i8[ge=1, le=12]`; exact-width primitives are storage carriers in this RFC, not constrained source-level domains.
- This RFC does not introduce bitfields, packed model layout, or C ABI layout guarantees.
- This RFC does not define floating-point, decimal, or arbitrary-precision storage carriers.
- This RFC does not decide timezone, calendar, or locale semantics for datetime APIs.

## Guide-level explanation

Users pick a carrier by adding `storage` to an integer type:

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

The first call constructs a valid `Month`. The second call fails validation because `13` is outside the declared range. The failure is about the range, not about an attempted primitive cast.

A side with no written bound reaches as far as the carrier does:

```incan
type Count = newtype int[ge=0, storage=u8]    # 0 to 255
type Delta = newtype int[storage=i8]          # -128 to 127
type bigint = newtype int[storage=i128]       # every i128 value
```

This is the same rule that gives `int` its range. The built-in `int` means `newtype Int[storage=i64]`, so `int[ge=0]` admits zero up to the largest `i64`, and `int[ge=0, storage=u64]` admits zero up to the largest `u64`.

A written bound must fit the carrier, so this is invalid because `300` cannot be stored in a `u8`:

```incan
type Count = newtype int[le=300, storage=u8]
```

A carrier wider than `i64` works the same way. Every `int` fits in a `bigint`, so a plain `int` flows in without being able to fail, and a literal may use the whole range. Going back to `int` can lose values, so it is explicit and checked:

```incan
type bigint = newtype int[storage=i128]

def record(total: bigint) -> None:
    return

def main() -> None:
    record(42)
    record(170141183460469231731687303715884105727)
```

`storage` is accepted wherever `int[...]` is, so a field that needs a compact integer but no reusable name can say so directly:

```incan
model Sample:
    channel: int[storage=u8]
    month: Month
```

## Reference-level explanation

An integer type may declare a storage carrier inside the constrained primitive bracket list using `storage=<integer-carrier>`. The storage carrier must be one of the supported exact-width integer primitive types: `i8`, `i16`, `i32`, `i64`, `i128`, `u8`, `u16`, `u32`, `u64`, or `u128`. The storage attribute must be accepted wherever a constrained `int[...]` type is accepted, with or without comparison bounds. It must not be accepted on `float`, `decimal`, `str`, `bytes`, collection types, or exact-width primitive underlyings such as `i8[ge=1, le=12]`. An `int` or `int[...]` that omits `storage` has the carrier `i64`.

The range of an integer type must be computed as follows: the lower limit is the tightest written `ge` or `gt` bound, or the carrier's minimum when none is written, and the upper limit is the tightest written `le` or `lt` bound, or the carrier's maximum when none is written. The compiler must reject a type whose effective written bounds fall outside the carrier's range. The compiler must reject duplicate storage attributes, unsupported storage carrier names, and contradictory constraints. For example, `int[le=300, storage=u8]` and `int[ge=-1, storage=u8]` must be rejected, while `int[ge=0, storage=u8]` admits `0..255` and `int[gt=-1, lt=256, storage=u8]` also admits `0..255`.

The source-level type of a storage-backed newtype remains the nominal newtype, not the carrier primitive. A `Month` is not a `u8`, and a `u8` is not automatically a `Month`. An unnamed `int[storage=u8]` is likewise an integer type with that carrier, not the primitive `u8`. RFC 017 implicit validated-newtype coercion may apply to storage-backed newtypes at the same sites where it applies to other validated newtypes, and that coercion must validate the range before constructing the value. Where the type's range contains every `int` value, that coercion cannot fail. The compiler must not insert ambient primitive conversions because a storage carrier exists. In particular, a general `int` expression must not silently become `u8`, `u16`, or any other exact-width primitive, and a value whose range exceeds `int` must not silently become `int`; each needs an existing explicit conversion or a checked validated-newtype path.

Reading a newtype's underlying value with `.0` must produce `int` when the type's range fits in `int`, and the carrier otherwise. `Month.0` is an `int`, and `bigint.0` is an `i128`.

An integer literal takes its limits from the type it flows into. A literal beyond the range of `int` must be accepted where that type admits it. Literals flowing into a storage-backed type should receive precise compile-time diagnostics when their value is statically known to fall outside the range. Runtime integer values flowing into a storage-backed type must use the same validation behavior as other constrained newtypes. If the value fails validation, the failure mode must match RFC 017 for the relevant construction site. Operations that produce a storage-backed type must not use carrier overflow as their semantics: an arithmetic result outside the range must fail through that same validation path, including where the range is the carrier's whole range.

Checked reflection, library manifests, and generated type metadata must preserve the written constraints, the carrier and the resulting range so that downstream compilation and tooling see the same contract. Serialized type descriptors must retain those facts when their owning format exposes them; ordinary value serialization retains its owning format contract, without a new wire object introduced by this RFC. Supported structured type reflection must expose the carrier by default as a representation fact beside the nominal type, logical underlying type, and checked constraints. The carrier must not replace those semantic facts or imply primitive substitutability, an implicit conversion, or a C ABI guarantee. The descriptor follows the owning reflection contract; this RFC does not add a separate runtime reflection API. A storage carrier does not by itself promise stable C ABI layout or packed model layout.

## Design details

The spelling keeps storage in the same bracket block as the bounds because it belongs to the same integer type, and the `storage` value is a type name parsed as a bare identifier from the exact-width integer primitive set:

```incan
type Port = newtype int[ge=0, le=65535, storage=u16]
type SmallDelta = newtype int[storage=i8]
```

This RFC intentionally uses `int[...]` as the source-level integer type and exact-width primitives as storage carriers. `type Month = newtype i8[ge=1, le=12]` is not the same contract: it exposes `i8` as the source-level underlying type, so ordinary `int` input would need an ambient primitive conversion before validation. The storage-backed spelling is `type Month = newtype int[ge=1, le=12, storage=i8]`.

The carrier remains visible in checked API metadata so documentation and schema tools can show both the range and the storage choice without guessing from generated output. Documentation and hover must show the resulting range, so `int[ge=0, storage=u8]` reads as `0..255` even though `255` was not written. `storage` does not change where validation happens: a model field of type `Month` validates through `Month`, and a model field of type `int[ge=1, le=12, storage=u8]` validates that range at the same sites as RFC 085's `int[ge=0]`.

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

Users can already write exact-width primitives such as `u8` and `i128`, but direct primitive fields do not carry domain names, validation hooks, or RFC 017 boundary behavior. `month: u8` also accepts `0`, which is not a valid month, and `total: i128` does not accept an ordinary `int`. Integer types are the right surface; exact-width primitives are the right carrier.

### Make `int[ge=0]` automatically choose an unsigned carrier

Automatic carrier selection hides an ABI-relevant and tooling-relevant choice behind validation syntax. It also creates compatibility risk when a later constraint change silently changes layout. This RFC keeps storage explicit.

### Require every bound to be written, and keep the range inside `i64`

Requiring `int[ge=0, le=255, storage=u8]` and refusing `int[ge=0, storage=u8]` prevents a carrier change from changing a range. It also makes `i64` the only real range and `int` a special case, and it rules out carriers wider than `i64`, because `int[storage=i128]` would store 128 bits and admit 64. A carrier sets the limit for `int` already, and every other carrier should do the same.

### Use `type=u64`

`type` is misleading because the source-level type is still an integer type with its own range, not the primitive. The carrier is a storage detail with observable code-generation and metadata consequences, so `storage` is clearer.

### Add model field storage metadata

Field-level metadata would let users write `month: int { storage=u8 }`, but a carrier decides which values a type admits, so it belongs in the type. RFC 085 keeps type constraints out of field metadata for the same reason.

## Drawbacks

This adds another dimension to integer type syntax. Users must understand the difference between bounds and storage carriers.

Changing the carrier of a type with an unwritten bound changes its range. That is an API change, and it is why documentation, hover and reflection show the resulting range. An author who wants a fixed range while tuning storage writes both bounds.

Storage carriers make generated layout more intentional, which means changing `storage` can become a compatibility concern for libraries that expose generated host projections or serialized representations.

Carriers wider than `i64` mean a value's reachable range can exceed `int`, so reading such a value back as `int` needs an explicit, checked step.

The feature can be overused. Not every domain integer needs compact storage, and `storage` should not become noise in ordinary model declarations.

## Implementation architecture

This section is non-normative. The recommended shape is to keep the integer type as the user-facing semantic type, preserve a separate storage-carrier annotation in checked type metadata, validate the carrier during semantic analysis, and lower the value to the chosen carrier after computing its range. Construction should continue to flow through the validated-newtype path, with generated conversions occurring after validation rather than by exposing lossy primitive casts at the source level.

## Layers affected

- **Parser / AST**: constrained primitive bracket syntax must accept one storage-carrier attribute with or without comparison constraints.
- **Typechecker / Symbol resolution**: integer types must validate carrier names, duplicate storage attributes, constraint consistency, written bounds against the carrier, the resulting range, literals beyond `int`, and RFC 017 coercion behavior.
- **IR Lowering**: storage-backed types must carry the range and the chosen backing carrier into lowered form.
- **Backend representation**: the selected backend must store the value using the chosen exact-width carrier while preserving checked construction semantics; this contract must not depend on emitted Rust source.
- **Stdlib / Runtime**: validation error rendering and numeric conversion helpers must support storage-backed construction paths, including carriers wider than `i64`.
- **Formatter**: constrained primitive formatting must preserve the storage attribute in a stable order.
- **LSP / Tooling**: hover, completion, diagnostics, and metadata extraction should surface the range and the storage carrier distinctly.

## Design Decisions

- `int` is the default carrier rather than a special case: the built-in `int` means `newtype Int[storage=i64]`, and every integer type follows one rule, where the carrier sets the range and written bounds narrow it. Widening and narrowing are the same feature.
- `storage` is accepted wherever `int[...]` is, and does not require comparison bounds or a named newtype.
- Storage carriers are the exact-width integer primitives enumerated in the reference contract. Target-dependent isize/usize and floating-point carriers are outside this proposal; expanding that set requires a separate contract.
- `.0` produces `int` when the range fits in `int`, and the carrier otherwise, so existing newtypes keep reading as integers and a wide value is never silently cut down.
- Supported structured reflection exposes the carrier by default as a representation fact alongside the type and its constraints. The carrier sets the reach of an unwritten bound, and it does not change assignability, conversion, or ABI guarantees. This incorporates the agreed reflection direction without selecting a new reflection entry point.
