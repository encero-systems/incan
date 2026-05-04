# RFC 009: Numeric type system and builtin type registry

- **Status:** Draft
- **Created:** 2024-12-11
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 005 (Rust interop)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/325
- **RFC PR:** —
- **Written against:** v0.1
- **Shipped in:** —

## Summary

This RFC introduces Incan's explicit numeric type system: exact-width signed and unsigned integers, exact-width binary floats, fixed-precision decimals, analytics-oriented aliases, and a centralized builtin-type registry. The coupling is intentional: once Incan exposes a broader numeric surface, the language needs one canonical vocabulary source for builtin spellings, aliases, literal suffixes, bounds, conversion behavior, interop metadata, and user-facing diagnostics rather than a growing set of scattered compiler special cases.

## Motivation

The current numeric surface is intentionally simple: `int` lowers to `i64` and `float` lowers to `f64`. That is fine for general application code, but it is too blunt for several real cases:

- Rust interop and FFI frequently require exact-width numeric types.
- Binary protocols and file formats encode fixed-width fields.
- Memory-sensitive workloads benefit from smaller element types.
- Bit manipulation and hardware-facing work depend on explicit widths.
- Data and analytics workloads need schema-stable decimal values instead of binary floating-point approximations.
- Query-plan and columnar-data interop benefit from names that map cleanly to Substrait- and Arrow-shaped numeric schemas.

The RFC also addresses a second problem that appears immediately once this surface arrives: builtin numeric behavior is currently too easy to define piecemeal. If each new builtin type adds methods, coercions, and surface spellings through disconnected compiler logic, the language contract will drift. The builtin registry is therefore not incidental implementation detail in this RFC; it is the mechanism that keeps the expanded numeric surface coherent.

## Goals

- Add exact-width signed integers, unsigned integers, sized floats, and fixed-precision decimals to the language surface.
- Preserve `int` and `float` as ergonomic aliases for general-purpose code.
- Add data- and analytics-oriented aliases where the alias maps to an existing canonical numeric type without changing semantics.
- Require explicit conversion where precision, scale, sign, or width can change in a lossy way.
- Allow ergonomic lossless conversion and contextual numeric adaptation at explicit Rust interop boundaries.
- Make builtin numeric vocabulary come from a single language-owned registry rather than repeated string matching.

## Non-Goals

- Arbitrary-precision integers in this RFC.
- Unbounded arbitrary-precision decimals in this RFC.
- SIMD/vector numeric types in this RFC.
- Implicit numeric widening and narrowing rules beyond what this document explicitly allows.
- Making `usize` the ordinary user-facing indexing type for lists, strings, and slices.
- Adding `char` or Unicode scalar semantics.
- Freezing every future builtin numeric method.

## Guide-level explanation (how users think about it)

### Explicit widths when they matter

```incan
port: u16 = 8080u16
flags: u8 = 0b1010_0001u8
sample_rate: i32 = 44_100i32
```

Authors continue using `int` and `float` for ordinary code, but they can opt into explicit widths when interop, protocols, or memory layout demand it.

### Aliases remain ergonomic

```incan
count: int = 42
precise_count: i64 = 42

ratio: float = 3.14
precise_ratio: f64 = 3.14
```

`int` and `float` remain the default ergonomic spellings; `i64` and `f64` are the exact-width canonical forms.

### Data-oriented spellings map to canonical types

```incan
kind: byte = 255u8
warehouse_id: long = 9_223_372_036_854_775_000i64
score: double = 0.992
embedding_component: fp32 = 0.125f32
```

These spellings are aliases, not separate numeric semantics. Diagnostics and reflection may preserve the authored spelling where useful, but type identity normalizes to the canonical numeric type.

### Decimal values are schema-stable

```incan
price: decimal[10, 2] = 19.99d
tax_rate: numeric[5, 4] = 0.0825d
```

`decimal[P, S]` represents a fixed-precision decimal value with precision `P` and scale `S`. `numeric[P, S]` is an alias for `decimal[P, S]`. A bare `decimal` spelling is intentionally not introduced by this RFC because analytics schemas should not hide precision and scale behind a default.

### Conversion policy is explicit when data can change

```incan
small: u8 = 240u8
wide: int = small.resize()

count: int = 1000
maybe_byte: u8 = count.try_resize[u8]()?
wrapped_byte: u8 = count.wrapping_resize[u8]()
clamped_byte: u8 = count.saturating_resize[u8]()
```

Lossless upsizing may use `resize()` when the target type is known from context. Downsizings, sign changes, decimal scale changes, and binary-float/decimal conversions must use an explicit policy.

### Rust interop stays ergonomic

```incan
from rust::devices import configure_port

configure_port(8080)
```

When an explicit Rust boundary expects a numeric type such as `u16`, the compiler may adapt a numeric literal or provably lossless value to the boundary type. The compiler must not use Rust interop as a back door for arbitrary lossy conversions; if a conversion may fail or lose information, diagnostics should suggest `try_resize`, `wrapping_resize`, `saturating_resize`, or a more specific helper.

## Reference-level explanation (precise rules)

### Canonical numeric types

The language adds these canonical builtin numeric spellings:

| Incan type                        | Meaning                                    |
| --------------------------------- | ------------------------------------------ |
| `i8`, `i16`, `i32`, `i64`, `i128` | Signed fixed-width integers                |
| `u8`, `u16`, `u32`, `u64`, `u128` | Unsigned fixed-width integers              |
| `f32`, `f64`                      | Fixed-width binary floating-point values   |
| `isize`, `usize`                  | Pointer-sized signed and unsigned integers |
| `decimal[P, S]`                   | Fixed-precision decimal with scale `S`     |

`int` remains an alias for `i64`. `float` remains an alias for `f64`.

### Numeric aliases

The builtin registry must recognize these aliases:

| Alias              | Canonical type   | Notes                                                       |
| ------------------ | ---------------- | ----------------------------------------------------------- |
| `byte`             | `u8`             | Binary/data-oriented byte spelling                          |
| `short`            | `i16`            | Common small signed integer spelling                        |
| `smallint`         | `i16`            | SQL/data-system spelling                                    |
| `integer`          | `i32`            | SQL/data-system spelling; distinct from Incan's `int` alias |
| `long`             | `i64`            | Common large signed integer spelling                        |
| `real`             | `f32`            | SQL/data-system single-precision spelling                   |
| `double`           | `f64`            | Data-system double-precision spelling                       |
| `fp32`             | `f32`            | Substrait-style spelling                                    |
| `fp64`             | `f64`            | Substrait-style spelling                                    |
| `numeric[P, S]`    | `decimal[P, S]`  | Fixed-precision decimal alias                               |
| `decimal128[P, S]` | `decimal[P, S]`  | Explicit 128-bit decimal storage spelling                   |

Aliases must not create separate runtime or typechecker identities. Diagnostics may mention the authored alias when that improves clarity, but canonical type identity uses the right-hand side of the table.

### Reserved numeric names

The builtin registry must reserve these names so later features can use them without compatibility traps:

- `bigint`, for future arbitrary-precision integer semantics.
- Bare `decimal`, for a future decision about whether Incan should provide a default decimal precision and scale.
- Bare `numeric`, for the same reason as bare `decimal`.

### Decimal semantics

`decimal[P, S]` is a fixed-precision decimal type. `P` is the maximum number of significant decimal digits. `S` is the number of digits after the decimal point. This RFC requires `0 <= S <= P` and `P <= 38` for the required implementation surface.

The required decimal storage model is a signed 128-bit scaled integer. `decimal128[P, S]` is therefore an explicit alias for `decimal[P, S]`. A follow-up RFC may add `decimal256[P, S]` for higher precision, but this RFC does not require it.

Decimal literals use the suffix `d`, as in `19.99d`, so source code can distinguish decimal literals from binary float literals.

### Literals

- Unsuffixed integer literals default to `int` unless a surrounding annotation or inference context requires a different numeric type.
- Unsuffixed float literals default to `float` unless a surrounding annotation or inference context requires a different float type.
- Suffixed integer literals such as `42u16` and `7i8` must construct the explicitly named type.
- Suffixed float literals such as `3.14f32` must construct the explicitly named type.
- Decimal literals such as `19.99d` must construct a decimal type from surrounding annotation or inference context.
- Out-of-range suffixed literals are compile-time errors.
- Decimal literals that exceed the target precision or scale are compile-time errors when the target is statically known.

### Arithmetic and conversions

- Same-type integer arithmetic yields the same type.
- Same-type binary-float arithmetic yields the same type.
- Same-type decimal arithmetic preserves decimal semantics but may require operator-specific precision and scale rules. This RFC requires those rules to be registry-owned before implementation begins.
- Mixed-width integer arithmetic requires an explicit conversion unless a surrounding context admits only a lossless conversion and the compiler can prove it.
- Narrowing, sign-changing, precision-losing, scale-losing, and binary-float/decimal conversions must be explicit.
- Lossless upsizing may use `resize()` when the target type is known from context.
- Potentially lossy resizing must use `try_resize[T]()`, `wrapping_resize[T]()`, or `saturating_resize[T]()` depending on the intended behavior.

### Overflow behavior

Sized integers follow Rust's ordinary overflow behavior for generated Rust:

- debug builds trap on overflow;
- release builds wrap unless the program uses explicit checked, saturating, or wrapping operations.

The required integer helper families are:

- `checked_add`, `checked_sub`, `checked_mul`, and `checked_pow`;
- `wrapping_add`, `wrapping_sub`, `wrapping_mul`, and `wrapping_pow`;
- `saturating_add`, `saturating_sub`, `saturating_mul`, and `saturating_pow`.

The builtin registry must record which numeric families support each helper. A follow-up RFC may expand the helper catalog, but these families are part of this RFC's required surface.

### Indexing

Ordinary list, string, tuple, bytes, and slice indexing remains Incan-shaped and signed. `usize` is not required at ordinary indexing call sites. Lowering and runtime helpers may normalize signed indices to Rust `usize` internally after applying Incan indexing semantics such as negative-index handling.

APIs that explicitly traffic in capacities, offsets, Rust interop, or columnar layout metadata may use `usize` or another exact-width integer directly.

### Rust interop

Exact-width numeric types are exact-lowering types at Rust boundaries. `i32` maps to Rust `i32`, `u16` maps to Rust `u16`, `f32` maps to Rust `f32`, and so on. `decimal[P, S]` maps to the runtime decimal representation associated with that precision and scale.

The compiler may insert contextual numeric adaptation at explicit Rust boundaries only when the conversion is exact or provably lossless. Examples include an in-range integer literal passed to a Rust function expecting `u16`, or an `i16` value passed to a Rust function expecting `i64`. It must reject or require explicit conversion for downsize, sign-changing, decimal scale-changing, decimal/binary-float, or otherwise lossy cases.

## Design details

### Why the coupling is intentional

This RFC deliberately couples the numeric type system with a builtin registry because the registry is part of getting the language surface right. Without it, the feature would immediately push more builtin names, methods, bounds, literal suffixes, aliases, and coercion rules into scattered compiler branches, which would make the spec harder to reason about and the implementation easier to drift.

The important point is the contract, not the file layout: builtin behavior should come from one coherent vocabulary source instead of repeated hardcoded matches.

### Registry-first builtin vocabulary

The implementation therefore needs a language-owned builtin registry that defines:

- canonical builtin type spellings;
- aliases;
- literal suffixes;
- integer signedness and bit width;
- binary-float precision;
- decimal precision, scale, and storage width;
- numeric bounds;
- builtin method vocabulary;
- resize/conversion policy;
- Rust interop mapping;
- stable metadata needed for docs, diagnostics, and analytics/schema interop.

### Interaction with existing features

- Rust interop benefits directly because exact-width types can map to exact-width Rust signatures without widening `int` into an implicit conversion catch-all.
- Existing `int` and `float` code keeps working unchanged.
- Container indexing remains ordinary Incan indexing rather than forcing `usize` into normal user code.
- Future data/analytics features can map numeric schemas through the registry instead of inventing per-feature vocabulary.

### Compatibility / migration

The feature is additive at the user surface. Existing programs using `int` and `float` continue to compile. Existing uses of `i32`, `i64`, `f32`, and `f64` that were previously accepted as aliases must be audited during implementation because this RFC makes those spellings distinct exact-width types rather than aliases for `int` or `float`.

## Alternatives considered

1. **Expose exact widths only through Rust interop**
   - Too indirect. These types are useful inside ordinary Incan code, not only at FFI boundaries.

2. **Python-style arbitrary-precision `int` only**
   - That improves some numeric ergonomics, but it does not solve fixed-width interop, protocol parsing, explicit layout control, or columnar schema mapping.

3. **Wrapper types only**
   - Still requires real underlying fixed-width and decimal types, so it does not remove the core problem.

4. **C-style numeric names only**
   - Less explicit and often platform-dependent in ways that this RFC is trying to avoid.

5. **No aliases**
   - Canonical Rust-shaped spellings are clear, but data and analytics users routinely encounter SQL-, Arrow-, and Substrait-shaped numeric names. Registry-owned aliases give those users a familiar entry point without creating additional type identities.

6. **Bare `decimal` with a default precision and scale**
   - This is ergonomic but hides schema decisions. In a data-oriented language, decimal precision and scale are part of the contract, so this RFC requires explicit `decimal[P, S]` and reserves bare `decimal` for a future decision.

## Drawbacks

- More builtin numeric types increase the language surface and the testing matrix.
- Decimal support raises the implementation bar because parser, typechecker, lowering, runtime, docs, and interop need precision/scale-aware behavior.
- `isize` and `usize` expose target-dependent widths, which slightly weakens the otherwise explicit story.
- Aliases can confuse users if diagnostics do not normalize clearly to canonical types.
- The registry requirement raises the implementation bar, but that is preferable to baking in more ad hoc builtin behavior.

## Layers affected

- **Lexer / parser**: must recognize added type names, aliases, parameterized decimal types, suffixed numeric literals, and decimal literals.
- **Typechecker**: must model exact-width numeric types, decimal precision/scale, alias normalization, explicit conversion policy, contextual interop adaptation, and out-of-range literal diagnostics.
- **Lowering / emission**: must preserve exact widths and decimal metadata when lowering to backend representations.
- **Runtime / stdlib**: must provide required decimal representation and numeric helper families.
- **Builtin surface registry**: must own canonical spelling, aliases, literal suffixes, bounds, method vocabulary, conversion policy, and interop/schema metadata for builtin numeric types.
- **Formatter / LSP**: should preserve authored spellings where useful while exposing canonical type information and diagnostics.
- **Docs / tooling**: should surface width-specific help, aliases, conversions, decimal precision/scale, and overflow behavior consistently.

## Design Decisions

- `int` remains an alias for `i64`; `float` remains an alias for `f64`.
- The exact-width integer and binary-float spellings are distinct canonical numeric types, not aliases for `int` or `float`.
- Data-oriented aliases are included only when they map to an existing canonical numeric type without changing semantics.
- `bigint`, bare `decimal`, and bare `numeric` are reserved for future features rather than claimed as aliases in this RFC.
- `decimal[P, S]` and `numeric[P, S]` are in scope as fixed-precision decimal types backed by a 128-bit scaled integer for the required implementation surface.
- `char` is out of scope because this RFC is about numerics, not Unicode scalar or string semantics.
- Ordinary indexing remains signed and Incan-shaped; users should not need `usize` for normal list, tuple, string, bytes, or slice indexing.
- Lossless upsizing can be ergonomic through `resize()` when the target type is known. Downsize, sign-changing, precision-losing, scale-losing, and binary-float/decimal conversions require explicit policy.
- Explicit Rust interop boundaries may perform exact or provably lossless numeric adaptation for good DX, but they must not silently perform lossy conversion.
- The builtin registry is the source of truth for numeric vocabulary, aliases, bounds, methods, conversions, diagnostics, docs metadata, and interop/schema mappings.
