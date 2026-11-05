# RFC 121: Unified Incan/Rust type substrate

- **Status:** Planned
- **Created:** 2026-08-19
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 005 (Rust interop)
    - RFC 009 (sized integers)
    - RFC 024 (extensible derive protocol)
    - RFC 029 (union types)
    - RFC 030 (`std.collections`)
    - RFC 041 (first-class Rust interop authoring)
    - RFC 043 (Rust trait implementation from Incan)
    - RFC 091 (integer storage carriers)
    - RFC 097 (Rust-hosted Incan caller)
    - RFC 120 (canonical source symbol identity)
    - RFC 128 (one callable trait over function types)
    - RFC 129 (shared or copied bindings)
    - RFC 141 (type matching; `std.reflection`)
    - RFC 144 (trait fields and type-owned constants)
    - RFC 164 (explicit ownership and borrowing)
    - #975 (Oven: Cargo-free Incan/Rust toolchain)
    - #1077 (unified Incan/Rust type substrate)
- **Issue:** https://github.com/encero-systems/incan/issues/1077
- **RFC PR:** —
- **Written against:** v0.5
- **Shipped in:** —

## Summary

Incan and Rust are one type substrate, expressed through two surface syntaxes — not two independently-typed systems bridged by a conversion layer at every boundary. Every Incan type compiles to one real Rust type: `List[str]` is `Vec<String>`, `int` is `i64` unless the type selects another carrier, and a `model` is a Rust struct. Crossing between Incan-authored and Rust-authored code, in either direction, passes the same value rather than converting it. This is the same relationship Kotlin has to Java: Kotlin's `String` is `java.lang.String`, not a wrapper around one, because both languages target the same underlying object model.

## Motivation

Before Oven, this was not a real option. Rust was built by a separate toolchain (Cargo), so Incan's own type system had to be treated as independent — something that got translated to Rust as a backend implementation detail, at every boundary crossing, because there was no single build authority that could guarantee both sides stayed representation-compatible. Once Oven owns the full build graph for both Incan-authored and Rust-authored code in one project (RFC 119), that structural constraint disappears. There is no longer a reason to keep them as two systems needing a bridge.

A conversion framing has real costs. It implies generated adapter code per type, a check at each crossing, and a decision per signature about how a string or integer travels. It also leaves room for two libraries to produce two incompatible copies of the same standard type. If the two sides were never different representations, there is nothing to check or convert.

This claim affects several RFCs: RFC 097 for Rust calling Incan, and RFC 005, RFC 041 and RFC 043 for Incan calling Rust. This RFC states it once, in one place.

## Goals

- State the principle: Incan and Rust share one type substrate.
- Give one rule for each category of type: built-ins, integer carriers, standard-library types, user-declared types, unions and traits.
- Give every standard-library type one definition, so values pass freely between libraries.
- Hold symmetrically for Rust calling Incan and Incan calling Rust.
- Keep what authors already have: exact-width integers, storage carriers, validated construction and RFC 041's admitted adaptations.
- Name every exception explicitly.
- Make a type's Rust representation something any tool or program can read.

## Non-Goals

- This RFC does not change Incan's authoring syntax. `model`, `enum`, and `newtype` keep meaning what they mean to an Incan author; this RFC is about what those declarations are in Rust.
- This RFC does not define a new ABI, FFI boundary, C layout guarantee or serialization format. For identical types, there is no boundary left to define.
- This RFC does not change how operators behave. `//` on an `int` still rounds down.
- This RFC does not remove validation. A rule a type declares still holds.
- This RFC does not edit RFC 005, RFC 041 or RFC 043, which are closed.

## Guide-level explanation

Here is what an Incan author writes:

```incan
pub type Month = newtype int[ge=1, le=12, storage=u8]

pub model Reading:
    pub month: Month
    pub channel: int[storage=u8]
    pub counts: List[int]
    pub labels: Dict[str, str]
    pub note: Option[str]

pub def summarize(reading: Reading) -> Result[str, str]:
    ...
```

And here is what those types are to Rust code in the same project:

```rust
pub struct Month(u8);

pub struct Reading {
    pub month: Month,
    pub channel: u8,
    pub counts: Vec<i64>,
    pub labels: HashMap<String, String>,
    pub note: Option<String>,
}
```

Nothing is converted when a `Reading` crosses between the two. `counts` was never a different representation from `Vec<i64>`, so a Rust function can read it directly.

**Integers follow their carrier.** `int` is `i64` by default, not always. RFC 091 lets a type select another carrier, and that carrier is its real Rust type. `channel` above is a `u8`, `Month` is a struct holding a `u8`, and `type bigint = newtype int[storage=i128]` is a struct holding an `i128`. Exact-width types from RFC 009, such as `i8` and `u128`, are the Rust types of the same name.

**Identity removes conversion, not checks.** Every `i64` is a valid `int`, so there is nothing to check when one crosses. A `Month` has a rule, so its Rust field is private and Rust code builds one only through its checked constructor.

**Standard-library types have one definition.** Suppose two libraries both use `Duration` in a public signature:

```incan
# policy/sources/incan/lib.incn
pub def evaluate_policy(request: ProviderRequest, window: Duration) -> Result[ProviderDecision, str]:
    ...
```

```incan
# billing/sources/incan/lib.incn
pub def charge_for(amount: int, window: Duration) -> Result[Receipt, str]:
    ...
```

Both signatures name the same Rust type, declared once by the standard-library Loaf that owns `Duration`, so a Rust caller can pass one library's value to the other. User-declared types need no such sharing, because each library owns its own `ProviderRequest`.

**Borrowing has one spelling.** `str` is `String`, and `Borrowed[str]` is what Rust calls `&str`. An Incan author writes RFC 164's contracts, or lets RFC 129 infer them, and never needs a Rust spelling.

**A trait is a real Rust trait.** Adopting a trait in Incan produces a real implementation that Rust code can call and bound on, with named exceptions where Rust has no matching concept.

**A representation can be read.** A program can ask a type what it is in Rust, and each field names the type it belongs to:

```incan
def print_layout() -> None:
    for field in Reading.__fields__():
        println(f"{field.parent}.{field.name} is {field.type.representation}")
```

```text
shop.readings.Reading.month is Month(u8)
shop.readings.Reading.channel is u8
shop.readings.Reading.counts is Vec<i64>
shop.readings.Reading.labels is HashMap<String, String>
shop.readings.Reading.note is Option<String>
```

## Reference-level explanation

1. Every Incan type must compile to one real Rust type. Passing a value between Incan-authored and Rust-authored code must pass that value, with no generated wrapper, adapter or check at the crossing, except as rules 9 and 13 allow.

2. Built-in types must have these Rust types:

    | Incan | Rust |
    | --- | --- |
    | `int`, `i64` | `i64` |
    | `float`, `f64` | `f64` |
    | `bool` | `bool` |
    | `str` | `String` |
    | `bytes` | `Vec<u8>` |
    | `None` | `()` |
    | `i8` to `i128`, `u8` to `u128`, `isize`, `usize`, `f32` | the Rust type of the same name |
    | `Option[T]` | `Option<T>` |
    | `Result[T, E]` | `Result<T, E>` |
    | `Tuple[A, B, ...]` | `(A, B, ...)` |
    | `List[T]` | `Vec<T>` |
    | `Dict[K, V]` | `HashMap<K, V>` |
    | `Set[T]` | `HashSet<T>` |

3. An integer type with an RFC 091 carrier must be stored in that carrier. An unnamed `int[...]` must compile to its carrier, and a `newtype` must compile to a Rust struct holding its carrier. This RFC must not narrow what RFC 009 or RFC 091 allow an author to declare.

4. Every other standard-library type, including `FrozenStr`, `FrozenBytes`, `FrozenList[T]`, `FrozenDict[K, V]`, and `FrozenSet[T]`, must have exactly one Rust definition, owned by the standard-library Loaf that declares it and identified by its RFC 120 canonical identity. Consumers must reference that definition and must not regenerate it. Two libraries using the same standard-library type must produce mutually compatible Rust values.

5. A user-declared `model` or `class` must compile to a Rust struct, an `enum` to a Rust enum, and a `newtype` to a Rust struct holding its underlying type. Incan's validation and derives (RFC 024) must attach to that same type, with no shadow representation.

6. An anonymous union (RFC 029) must compile to one Rust enum per distinct set of member types, shared by every use of that set. `T | None` must be `Option<T>`.

7. Identity must not remove a declared rule. Rust code must not be able to build a `newtype`, or a `model` with a constrained field, in a way that breaks a declared rule, except by going through its checked constructor.

8. `Borrowed[T]` must be a Rust shared reference to `T`, and `BorrowedMut[T]` an exclusive one. `Borrowed[str]` must be `&str`, and `Borrowed[bytes]` must be `&[u8]`. A plain parameter follows RFC 129 and RFC 164. Incan source must not need a Rust spelling for any of these.

9. Where a Rust API asks for a different type than the value has, the adaptations RFC 041 admits, such as `float` to `f32` or `FrozenList[T]` to `Vec<T>`, must remain available. They must stay bounded by that table and visible in the target signature.

10. An Incan trait must compile to a real Rust trait, and adopting it must compile to a real implementation.

11. All of these rules must hold whether Rust-authored code calls Incan-authored code (RFC 097) or Incan-authored code imports and uses Rust (RFC 005, RFC 041, RFC 043). A Rust type imported into Incan must remain that Rust type.

12. These rules cover representation, not operators. An Incan operator must keep its Incan meaning on a value, and a Rust operator its Rust meaning on the same value.

13. Any departure from these rules must be a named exception on a published list, with its reason, and with a diagnostic when code relies on identity the exception does not provide. An implementation must not fall back silently to conversion.

14. `std.reflection` must expose these facts. `TypeInfo` must provide `identity`, the RFC 120 canonical identity printed as its full path, and `representation`, the Rust type. `FieldInfo` must provide `parent`, the identity of the type that declares the field. RFC 128's `SignatureInfo` must provide the declaration's identity, and each of its parameter entries the same `parent`.

15. `incan inspect types` must report, for a given type, its identity, its Rust representation, and any exception that applies to it. It must also list every named exception.

## Design details

### Integers and carriers

`int` is `i64` by default because it means `newtype Int[storage=i64]`, and RFC 009 keeps `int` and `i64` interchangeable. RFC 091 owns which carriers exist and which values each type admits. This RFC adds only that the chosen carrier is the type's real Rust representation, so narrowing and widening both come with no conversion.

### Why `Dict` is `HashMap`

RFC 041 already lowers `Dict[K, V]` to `HashMap<K, V>`, and that fixes its requirements: keys must be hashable and comparable, and iteration order is not promised. A program that needs an order uses RFC 030's `OrderedDict` or `SortedDict`, which are standard-library types under rule 4.

### Frozen types

`FrozenStr`, `FrozenBytes`, `FrozenList[T]`, `FrozenDict[K, V]`, and `FrozenSet[T]` are standard-library types over baked, read-only data, so rule 4 covers them. They are not `Vec` or `HashMap` with immutability added. RFC 041's table lists those Rust types for them, and this RFC treats that as what rule 9 keeps: the adaptation a Rust API can ask for.

### Traits and the exception list

The list starts with one entry:

- **Trait fields (RFC 144):** a Rust trait has no fields. An adopter's field is a real struct field, and the trait requirement appears in Rust as accessor methods. RFC 144's type-owned constants are not an exception, because Rust has associated constants.

### Relationship to other RFCs

RFC 097 describes the caller boundary in terms of this RFC, and its type table lists identities. RFC 005, RFC 041 and RFC 043 are closed and stay as written, so this RFC and the language reference hold the contract where they differ.

## Alternatives considered

- **Keep Incan and Rust as two independently-typed systems, bridged by conversion at every boundary.** It works, but it implies ongoing translation and checking cost, and duplicated logic per boundary, that a single Oven-owned build graph no longer requires.
- **Apply this only to RFC 097's caller boundary.** Rejected because it would leave the model asymmetric: one substrate calling in one direction, two systems calling in the other.
- **A dedicated crate for shared standard-library types.** Rejected because standard-library types already live in Loaves with RFC 120 identity, so a separate crate would add a second home for them.
- **A per-call choice between `String` and `&str`.** Rejected because RFC 164 and RFC 129 already say how a value is passed, and strings need no policy of their own.
- **Fix `int` at `i64` everywhere.** Rejected because it would take away RFC 009's exact-width types and RFC 091's carriers.

## Drawbacks

- A built-in's Rust type becomes a promise. Changing what `Dict` is in Rust would break Rust code that names it.
- Without a check at the crossing, the guarantee rests on the compiler's own correctness rather than a runtime backstop.
- Rust code sees Incan's choices directly, including `i64` indexes and owned strings where a hand-written Rust API might do otherwise.
- The exception list has to be kept accurate, because it is what makes the rest of the contract trustworthy.

## Layers affected

- **Typechecker / language type system**: each type's Rust representation is a checked fact, not a decision made during emission.
- **IR Lowering / Emission**: no conversion or adapter for identical types, and values pass through unchanged.
- **Standard-library Loaves**: own the single definition of each standard-library type.
- **Reflection**: `TypeInfo`, `FieldInfo` and `SignatureInfo` gain the facts in rule 14.
- **LSP / Tooling**: `incan inspect types` and hover report representation, identity and exceptions.
- **Library artifacts**: a Loaf records each public type's identity and representation, so consumers can confirm they agree.

## Design Decisions

- **Shared standard-library types live in the Loaf that declares them,** with RFC 120 identity, so no separate crate exists.
- **`String` versus `&str` is not a separate policy.** `str` is owned, `Borrowed[str]` is the borrowed form, and RFC 164 and RFC 129 decide how a value is passed.
- **Closed RFCs stay frozen.** RFC 005, RFC 041 and RFC 043 are not edited, and RFC 097 is updated directly.
- **Traits are covered,** with named exceptions, starting with RFC 144's trait fields.
- **The rule covers representation, not operators.** `int` is `i64`, while `//` rounds down and Rust's `/` truncates.
- **`int` is a default carrier, not a fixed type.** Nothing an author can declare under RFC 009 or RFC 091 is lost.
- **RFC 041's table is the base for built-ins, and its adaptations stay.** Frozen types are the correction: they are standard-library types, with `Vec`, `HashMap` and `HashSet` remaining as adaptations.
- **Representation is readable through reflection,** and each field names its parent by identity.
