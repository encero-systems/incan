# RFC 050: Enum methods and enum trait adoption

- **Status:** Planned
- **Created:** 2026-04-06
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 025 (multi-instantiation trait dispatch)
    - RFC 032 (value enums)
    - RFC 051 (`JsonValue`)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/334
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC extends enums so they can declare methods and adopt traits. The goal is to bring enums to parity with models and classes for attached behavior and trait-based protocol participation, without coupling that language feature to any one stdlib type.

## Motivation

### Enums need behavior, not only variants

Today, Incan enums are variant-only declarations. That makes them weaker than models and classes whenever a type needs both data-shape and behavior.

```incan
enum Direction:
    North
    South
    East
    West

    def opposite(self) -> Direction:
        match self:
            Direction.North => return Direction.South
            Direction.South => return Direction.North
            Direction.East => return Direction.West
            Direction.West => return Direction.East
```

Without enum methods, that kind of behavior has to move into clumsy module-level helpers.

### Enums also need trait adoption

Models and classes already adopt traits with `with Trait`. Enums should be able to do the same. Otherwise any enum-backed protocol surface needs special-case compiler support instead of reusing the same trait-based model the language already has for other types.

## Goals

- Allow enums to declare methods and associated functions.
- Allow enums to adopt traits with `with`.
- Keep the feature additive to existing enum semantics.
- Bring enums into parity with models and classes for attached behavior and protocol participation.

## Non-Goals

- Changing `match` semantics.
- Typed JSON derive support beyond what RFC 024 already specifies.
- A general-purpose `Any` or unrestricted dynamic type.
- Specifying a dedicated dynamic JSON type in this RFC.

## Guide-level explanation (how users think about it)

### Enum methods

```incan
enum Direction:
    North
    South
    East
    West

    def is_horizontal(self) -> bool:
        match self:
            Direction.East => return true
            Direction.West => return true
            _ => return false
```

Enums now support behavior where the behavior belongs with the enum itself.

### Enum trait adoption

```incan
from std.traits.indexing import Index

enum Lookup with Index[str, int]:
    Mapping(Dict[str, int])
    Empty

    def __getitem__(self, key: str) -> int:
        match self:
            Lookup.Mapping(d) => return d[key]
            Lookup.Empty => return 0
```

This brings enums to parity with models and classes for trait adoption.

## Reference-level explanation (precise rules)

### Enum methods

- Enum bodies may declare methods and associated functions after their variant declarations.
- Enum methods follow the same receiver rules as methods on models and classes.
- Methods may reference the enum's type parameters and use `match self` or equivalent pattern matching against variants.

### Enum trait adoption

- Enum declarations may include an optional `with Trait1, Trait2, ...` clause after the enum name and any type parameters.
- Trait adoption on enums must follow the same validation and coherence rules already used for models and classes.

## Design details

### Syntax

Enum syntax gains two additive capabilities:

1. an optional `with` clause after the enum name; and
2. method declarations inside the enum body after the variant list.

This keeps the surface aligned with existing model/class conventions rather than inventing a separate enum-only form.

### Semantics

Enum methods behave like other instance or associated methods in the language. They do not change how variants are matched or constructed. Trait adoption on enums is likewise intended to extend existing trait machinery, not bypass it.

### Interaction with existing features

- RFC 025 is the clean route for allowing one enum to adopt multiple instantiations of the same generic trait when needed.
- RFC 032 benefits directly because value enums become able to carry user-defined methods and trait implementations alongside generated ones.
- Rust interop remains relevant because exported enum methods should follow the same conventions as model and class methods.

### Compatibility / migration

The feature is additive. Existing enums without methods or traits keep working unchanged.

## Alternatives considered

1. **Keep enums data-only**
   - Simpler, but it pushes obviously enum-owned behavior into module-level helpers and weakens the type system story.

2. **Keep trait adoption limited to models and classes**
   - Inconsistent. It would preserve an arbitrary capability gap between declaration kinds.

3. **Use module-level helpers instead of enum methods**
   - Functional, but significantly worse for discoverability and consistency with the rest of the language.

## Drawbacks

- Enum methods and trait adoption expand the language surface and require careful end-to-end consistency.
- Some downstream library designs will become easier to propose once this lands, which means pressure for adjacent features may rise quickly.

## Layers affected

- **Language surface**: enums must accept `with` clauses and method declarations.
- **Type system**: enum methods and enum trait adoption must follow the same broad validation rules used for models and classes.
- **Interop / code generation**: emitted artifacts must preserve enum methods and adopted traits in the same spirit as model/class methods and traits.
- **Docs / tooling**: completion, hover text, and diagnostics should surface enum methods and enum-adopted traits ergonomically.

## Design Decisions

1. **Enum methods and trait adoption are general-purpose language features.** They are not justified only by one library use case.
2. **Enums should reach parity with models and classes for attached behavior and trait participation.** The language should not preserve an arbitrary capability gap here.
3. **Dedicated stdlib types that depend on this feature, such as a dynamic JSON value type, belong in separate RFCs.** This RFC defines the language capability they can build on.
