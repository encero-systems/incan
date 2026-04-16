# RFC 009: Sized numeric types and builtin type registry

- **Status:** Draft
- **Created:** 2024-12-11
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 005 (Rust interop)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/325
- **RFC PR:** —
- **Written against:** v0.1
- **Shipped in:** —

## Summary

This RFC introduces explicit sized numeric types such as `i8`, `u16`, `f32`, and `usize`, together with a centralized builtin-type registry. The coupling is intentional: once Incan exposes a broader builtin numeric surface, the language needs one canonical vocabulary source for builtin spellings, methods, and typing behavior rather than a growing set of scattered compiler special cases.

## Motivation

The current numeric surface is intentionally simple: `int` lowers to `i64` and
`float` lowers to `f64`. That is fine for general application code, but it is
too blunt for several real cases:

- Rust interop and FFI frequently require exact-width numeric types.
- Binary protocols and file formats encode fixed-width fields.
- Memory-sensitive workloads benefit from smaller element types.
- Bit manipulation and hardware-facing work depend on explicit widths.

The RFC also addresses a second problem that appears immediately once sized types arrive: builtin numeric behavior is currently too easy to define piecemeal. If each new builtin type adds methods, coercions, and surface spellings through disconnected compiler logic, the language contract will drift. The builtin registry is therefore not incidental implementation detail in this RFC; it is the mechanism that keeps the expanded numeric surface coherent.

## Goals

- Add exact-width signed integers, unsigned integers, and sized floats to the language surface.
- Preserve `int` and `float` as ergonomic aliases for general-purpose code.
- Require explicit conversion where precision or sign can change.
- Make builtin numeric vocabulary come from a single language-owned registry rather than repeated string matching.

## Non-Goals

- Arbitrary-precision integers in this RFC.
- SIMD/vector numeric types in this RFC.
- Implicit numeric widening and narrowing rules beyond what this document explicitly allows.
- Freezing the entire builtin-method catalog for every future builtin type.

## Guide-level explanation (how users think about it)

### Explicit widths when they matter

```incan
port: u16 = 8080u16
flags: u8 = 0b1010_0001u8
sample_rate: i32 = 44_100i32
```

Authors continue using `int` and `float` for ordinary code, but they can opt into explicit widths when interop, protocols, or memory layout demand it.

### Aliases remain

```incan
count: int = 42
precise_count: i64 = 42

ratio: float = 3.14
precise_ratio: f64 = 3.14
```

`int` and `float` remain the default ergonomic spellings; `i64` and `f64` are the exact-width forms.

### Explicit conversions

```incan
big: i64 = 1000
small: i16 = big as i16
safe: i16 = i16.try_from(big)?
```

The language should not silently narrow or reinterpret numbers in ways that are easy to miss.

## Reference-level explanation (precise rules)

### Added types

The language adds these builtin numeric spellings:

| Incan type                        | Meaning                                    |
| --------------------------------- | ------------------------------------------ |
| `i8`, `i16`, `i32`, `i64`, `i128` | Signed fixed-width integers                |
| `u8`, `u16`, `u32`, `u64`, `u128` | Unsigned fixed-width integers              |
| `f32`, `f64`                      | Fixed-width floating-point values          |
| `isize`, `usize`                  | Pointer-sized signed and unsigned integers |

`int` remains an alias for `i64`. `float` remains an alias for `f64`.

### Literals

- Unsuffixed integer literals default to `int` unless a surrounding annotation
or inference context requires a different numeric type.
- Unsuffixed float literals default to `float` unless a surrounding annotation
or inference context requires a different float type.
- Suffixed literals such as `42u16`, `7i8`, and `3.14f32` must construct the explicitly named type.
- Out-of-range suffixed literals are compile-time errors.

### Arithmetic and conversions

- Same-type arithmetic yields the same type.
- Mixed-width arithmetic requires an explicit conversion.
- Narrowing or sign-changing conversions must be explicit.
- Widening conversions may be provided as named constructors or helpers, but
this RFC does not permit silent widening in ordinary arithmetic expressions.

### Overflow behavior

Sized integers follow Rust's overflow behavior:

- debug builds trap on overflow;
- release builds wrap unless the program uses explicit checked, saturating, or wrapping operations.

The language surface may expose helpers such as `checked_add`, `wrapping_add`, and `saturating_add` on applicable numeric types.

## Design details

### Why the coupling is intentional

This RFC deliberately couples sized numeric types with a builtin registry because the registry is part of getting the language surface right. Without it, the feature would immediately push more builtin names, methods, and coercion rules into scattered compiler branches, which would make the spec harder to reason about and the implementation easier to drift.

The important point is the contract, not the file layout: builtin behavior should come from one coherent vocabulary source instead of repeated hardcoded matches.

### Registry-first builtin vocabulary

The implementation therefore needs a language-owned builtin registry that defines:

- builtin type spellings;
- builtin method vocabulary;
- stable metadata needed for typing, docs, and diagnostics.

### Interaction with existing features

- Rust interop benefits directly because exact-width types can map to exact-width Rust signatures.
- Existing `int` and `float` code keeps working unchanged.
- Container indexing becomes more important once `usize` exists, but this RFC
does not silently settle every indexing coercion rule yet.

### Compatibility / migration

The feature is additive. Existing programs using `int` and `float` continue to compile. New code can opt into explicit widths incrementally.

## Alternatives considered

1. **Expose exact widths only through Rust interop**
   - Too indirect. These types are useful inside ordinary Incan code, not only at FFI boundaries.

2. **Python-style arbitrary-precision `int` only**
   - That improves some numeric ergonomics, but it does not solve fixed-width
     interop, protocol parsing, or explicit layout control.

3. **Wrapper types only**
   - Still requires real underlying fixed-width types, so it does not remove the core problem.

4. **C-style numeric names**
   - Less explicit and often platform-dependent in ways that this RFC is trying to avoid.

## Drawbacks

- More builtin numeric types increase the language surface and the testing matrix.
- `isize` and `usize` expose target-dependent widths, which slightly weakens the otherwise explicit story.
- The registry requirement raises the implementation bar, but that is
preferable to baking in more ad hoc builtin behavior.

## Layers affected

- **Lexer / parser**: must recognize the added type names and suffixed numeric literals.
- **Typechecker**: must model exact-width numeric types, enforce explicit
conversions, and diagnose out-of-range literals.
- **Lowering / emission**: must preserve exact widths when lowering to backend
representations.
- **Builtin surface registry**: must own the canonical spelling and method vocabulary for builtin numeric types.
- **Docs / tooling**: should surface width-specific help, conversions, and overflow behavior consistently.

## Unresolved questions

1. What is the exact rule for container indexing: explicit `as usize`, a
targeted compiler coercion in indexing position, or something else?
2. Should `char` join this RFC, or remain a separate feature with its own semantics?
3. Which sized-type helper methods are language promises in v1, and which
remain stdlib conveniences that can evolve separately?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
