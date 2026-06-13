# RFC 110: `Comparable` trait and derive

- **Status:** Draft
- **Created:** 2026-06-06
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 024 (extensible derive protocol)
    - RFC 028 (trait-based operator overloading)
    - RFC 042 (traits are always abstract)
    - RFC 068 (protocol hooks for core language syntax)
    - RFC 088 (iterator adapter surface)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC proposes a user-facing `Comparable` trait and `@derive(Comparable)` surface that lets types define one canonical comparison operation and receive equality, ordering operators, sorting compatibility, and convenience helpers such as `between` and `clamp` through explicit checked trait behavior.

## Core model

1. **`Comparable` names the high-level comparison contract:** authors can say a type is comparable without spelling the lower-level operator hooks directly.
2. **`compare` is the canonical method:** a `Comparable` implementation defines `compare(self, other) -> Ordering`.
3. **`Ordering` avoids invalid integer states:** comparison returns `Ordering.Less`, `Ordering.Equal`, or `Ordering.Greater` rather than Ruby-style `-1`, `0`, or `1`.
4. **Operators derive from explicit defaults:** `==`, `!=`, `<`, `<=`, `>`, and `>=` are supplied by trait defaults or generated hooks, not compiler magic.
5. **`@derive(Comparable)` is structural:** deriving `Comparable` compares fields in declaration order when every field has a compatible comparison contract.
6. **Existing `Eq` and `Ord` remain valid:** `Comparable` is a friendlier high-level surface that should interoperate with the existing comparison machinery rather than replacing it abruptly.

## Motivation

Incan already supports `Eq`, `Ord`, comparison operators, and structural derives. That surface is close to Rust and precise for compiler lowering, but it is not the most readable authoring story for application developers who want to define one comparison rule and get the expected interface. Ruby's `Comparable` demonstrates the ergonomic win: implement one comparison method, then get a full comparison vocabulary. Incan can keep the same payoff while using a typed `Ordering` enum, explicit trait adoption, and compile-time diagnostics.

The current `Ord` story also pushes custom ordering toward `__lt__`, which is less canonical than a three-way comparison when a type needs total ordering. A single `compare` method is easier to audit: it must answer less, equal, or greater in one place. Default operator methods can then be derived from that answer consistently.

## Goals

- Add a `Comparable` trait with a canonical `compare` method.
- Add an `Ordering` enum with `Less`, `Equal`, and `Greater` variants if an equivalent standard enum does not already exist.
- Let `Comparable` provide equality and ordering operator behavior through explicit defaults or generated hooks.
- Add `@derive(Comparable)` for structural comparison by field order.
- Keep existing `@derive(Eq)`, `@derive(Ord)`, `__eq__`, and `__lt__` behavior compatible.
- Provide convenience helpers such as `between` and `clamp` where their error and bound semantics are settled.
- Give diagnostics for conflicting derives, incompatible field types, partial-order hazards, and invalid helper bounds.

## Non-Goals

- This RFC does not remove `Eq` or `Ord`.
- This RFC does not require existing code using `@derive(Eq, Ord)` to migrate.
- This RFC does not define partial ordering for values such as NaN; a future `PartialComparable` or equivalent may handle that separately.
- This RFC does not adopt Ruby's integer spaceship return convention.
- This RFC does not make unrelated traits with a method named `compare` part of the comparison surface.
- This RFC does not define locale-sensitive string ordering, natural sort, collation, or domain-specific comparison policy.

## Guide-level explanation

For structural types, deriving `Comparable` should be the simple spelling:

```incan
@derive(Comparable)
model Version:
    major: int
    minor: int
    patch: int

def supports(candidate: Version, minimum: Version) -> bool:
    return candidate >= minimum
```

The derived comparison uses field declaration order. `Version(major=1, minor=4, patch=0)` compares greater than `Version(major=1, minor=3, patch=9)` because `minor` breaks the tie after equal `major` values.

For custom ordering, implement one method:

```incan
model Task with Comparable[Task]:
    priority: int
    name: str

    def compare(self, other: Task) -> Ordering:
        priority_order = compare_values(self.priority, other.priority)
        if priority_order != Ordering.Equal:
            return priority_order
        return compare_values(self.name, other.name)
```

Once `Task` is comparable, ordinary operators work:

```incan
if task_a < task_b:
    println("task_a should run first")
```

Convenience helpers can use the same comparison contract:

```incan
if version.between(min_supported, max_supported):
    println("supported")

safe_version = version.clamp(min_supported, max_supported)?
```

The exact fallibility of `clamp` remains an open question in this Draft because inverted bounds should not become a hidden panic in an Incan API.

## Reference-level explanation

### `Ordering`

The standard library must expose a comparison result enum:

```incan
enum Ordering:
    Less
    Equal
    Greater
```

If an equivalent enum already exists in the standard library by the time this RFC is implemented, `Comparable` may reuse that enum instead of introducing a duplicate.

### `Comparable`

The trait surface is:

```incan
trait Comparable[T = Self]:
    def compare(self, other: T) -> Ordering
```

For `Comparable[Self]`, the trait should provide or imply equality and ordering behavior for the ordinary comparison operators:

- `a == b` is true when `a.compare(b) == Ordering.Equal`;
- `a != b` is true when `a.compare(b) != Ordering.Equal`;
- `a < b` is true when `a.compare(b) == Ordering.Less`;
- `a <= b` is true when `a.compare(b) != Ordering.Greater`;
- `a > b` is true when `a.compare(b) == Ordering.Greater`;
- `a >= b` is true when `a.compare(b) != Ordering.Less`.

The implementation may express those defaults through `Eq` and `Ord`, through explicit dunder hooks, through generated trait methods, or through another existing comparison mechanism. The source-level contract is that `Comparable` is the canonical comparison capability and the ordinary operators observe it consistently.

### Deriving `Comparable`

`@derive(Comparable)` must generate a structural `compare(self, other: Self) -> Ordering` implementation. The generated implementation compares fields in declaration order and returns the first non-equal field comparison. If all comparable fields are equal, it returns `Ordering.Equal`.

Every compared field must have a compatible comparison contract. If a field cannot be compared, deriving `Comparable` must fail with a diagnostic naming the field and its type.

`@derive(Comparable)` conflicts with a manually defined `compare` implementation for the same target type. A type must not both derive and manually define the same canonical comparison behavior.

### Relationship to `Eq` and `Ord`

`Eq` and `Ord` remain part of the lower-level comparison family. Existing code using `@derive(Eq)`, `@derive(Ord)`, `__eq__`, or `__lt__` remains valid.

For new code, `Comparable` should be the recommended spelling when a type has one total comparison relation and authors want the full comparison interface from one canonical method. `Eq` and `Ord` remain useful when a type needs only equality, only ordering hooks, or direct compatibility with lower-level comparison bounds.

If a type adopts both `Comparable` and `Ord`, their behavior must be consistent. If the compiler can detect conflicting manually defined hooks, it must reject the conflict rather than choosing one silently.

### Convenience helpers

`Comparable` should provide `between(min, max) -> bool` with inclusive bounds:

```incan
value.between(minimum, maximum)
```

`between` should return true when `minimum <= value and value <= maximum`. If `minimum > maximum`, the final design must choose whether this is false, a diagnostic when statically known, or a fallible helper. This Draft leaves the exact inverted-bound behavior unresolved.

`Comparable` should provide a clamping helper once its inverted-bound behavior is settled. A fallible spelling is currently the safest candidate:

```incan
value.clamp(minimum, maximum) -> Result[Self, ComparisonBoundsError]
```

If the final design chooses an infallible clamp, it must define inverted-bound behavior without hidden panics.

### Sorting and collection behavior

Sorting APIs and ordered collections may accept `T with Comparable` when a total ordering is required. They may continue to accept `T with Ord` for compatibility. The standard library should avoid maintaining two unrelated ordering pathways; `Comparable` and `Ord` should lower into a single coherent comparison capability.

### Diagnostics

The compiler should diagnose:

- deriving `Comparable` for a type with non-comparable fields;
- manually defining `compare` while also deriving `Comparable`;
- conflicting `Comparable`, `Eq`, or `Ord` behavior when statically detectable;
- using `Comparable` where a partial order would be required instead of a total order;
- calling helper methods such as `clamp` with invalid bounds when the invalidity is statically obvious.

## Design details

### Syntax

This RFC adds no new syntax beyond trait adoption and derive usage:

```incan
model Task with Comparable[Task]:
    ...

@derive(Comparable)
model Version:
    ...
```

### Semantics

`Comparable` defines a total order for the compared target type. A valid implementation must be reflexive, antisymmetric, transitive, and consistent with equality. The compiler cannot prove every semantic law, but docs, tests, and diagnostics should treat those laws as part of the contract.

### Interaction with operator overloading

RFC 028 defines comparison operators through explicit trait and dunder behavior. `Comparable` should provide that behavior through explicit defaults or generated hooks. The compiler must not synthesize hidden comparison behavior merely because a method named `compare` exists outside the `Comparable` trait.

### Interaction with derives

`@derive(Comparable)` is a built-in derive unless a future derive protocol can express the full comparison generation safely. It should appear in docs next to `Eq`, `Ord`, and `Hash`, with guidance on when to prefer each derive.

### Compatibility and migration

This RFC is additive. Existing `Eq` and `Ord` code keeps working. Documentation may gradually prefer `Comparable` for total ordering because it gives authors one canonical comparison method and a clearer high-level name.

## Alternatives considered

1. **Keep only `Eq` and `Ord`.** This is close to Rust and already works, but it misses the authoring clarity of one canonical comparison method.
2. **Name the trait `OrdBy`.** This emphasizes ordering but is less natural for users and does not communicate the full comparison interface as clearly as `Comparable`.
3. **Use Ruby's integer spaceship convention.** Returning `-1`, `0`, or `1` is compact but admits invalid values and weakens type checking.
4. **Only add `@derive(Comparable)` as an alias for `@derive(Eq, Ord)`.** This helps structural types but does not solve custom comparison ergonomics.
5. **Make `compare` a magic method without trait adoption.** This would be closer to dynamic protocol lookup and would conflict with Incan's explicit trait direction.

## Drawbacks

- The comparison vocabulary grows: users must understand `Comparable`, `Eq`, and `Ord`.
- The relationship between `Comparable` and existing lower-level comparison traits must be documented carefully.
- Deriving structural comparison by field order can be surprising if field order is not chosen deliberately.
- Helper methods such as `clamp` need explicit error semantics to avoid hidden panics or surprising bound handling.

## Layers affected

- **Parser / AST:** no new syntax is required, but derive and trait references must recognize `Comparable` once the standard surface exists.
- **Typechecker / Symbol resolution:** trait adoption, derive conflicts, field comparability, and operator compatibility must be checked consistently.
- **IR Lowering:** derived and default comparison behavior must lower through the canonical comparison contract without duplicating unrelated operator paths.
- **Emission:** generated comparison code must preserve field order, `Ordering` results, and operator behavior.
- **Stdlib / Runtime (`incan_stdlib`):** the standard library must expose `Ordering`, `Comparable`, default helper methods, and any comparison-bound error types.
- **Formatter:** formatter behavior is unchanged except for ordinary derive and trait formatting.
- **LSP / Tooling:** completion, hover, derived-member inspection, and diagnostics should show the generated comparison surface and conflicts with `Eq` or `Ord`.

## Unresolved questions

- Should `Comparable` imply `Eq` and `Ord` automatically for `Comparable[Self]`, or should docs recommend deriving/adopting all required traits explicitly?
- Should `clamp` be fallible, return `Option[Self]`, reject inverted literal bounds at compile time only, or define an infallible bound-normalization rule?
- Should `between(min, max)` return false for inverted bounds or share the same fallibility policy as `clamp`?
- Should cross-type comparison through `Comparable[T]` support ordinary operators, or should operators remain limited to `Self` comparisons in the first version?
- Should a `PartialComparable` trait be designed at the same time to handle partial orders explicitly?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
