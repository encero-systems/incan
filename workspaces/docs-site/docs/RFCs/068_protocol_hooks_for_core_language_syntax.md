# RFC 068: protocol hooks for core language syntax

- **Status:** Draft
- **Created:** 2026-04-16
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 028 (trait-based operator overloading)
    - RFC 030 (`std.collections`)
    - RFC 050 (enum methods and enum trait adoption)
    - RFC 051 (`JsonValue` for `std.json`)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/86
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC proposes a small set of compiler-recognized protocol hooks for core language syntax so user-defined types can participate in truthiness, `len(...)`, iteration, membership, indexing, assignment-through-indexing, and callability with predictable static typing and clear diagnostics. The surface is deliberately Python-shaped (`__bool__`, `__len__`, `__iter__`, `__next__`, `__contains__`, `__getitem__`, `__setitem__`, `__call__`) while remaining a statically checked language contract rather than dynamic runtime magic.

## Core model

Read this RFC as one foundation plus three mechanisms:

1. **Foundation:** certain core syntax forms need a stable user-definable protocol rather than builtin-only treatment.
2. **Mechanism A:** syntax such as `if x:`, `len(x)`, `for item in xs:`, `a in b`, `obj[key]`, `obj[key] = value`, and `obj(...)` resolves through a small set of named hooks.
3. **Mechanism B:** hook resolution is static and type-checked; there is no ambient dynamic fallback.
4. **Mechanism C:** operator overloading stays governed by RFC 028; this RFC covers non-operator core syntax surfaces.

## Motivation

Today, many useful syntax forms feel more builtin-oriented than language-oriented. Builtin lists, dicts, strings, and a few other standard shapes participate naturally in truthiness, length, iteration, membership, and indexing, but user-defined types often need special treatment or awkward adapter APIs to feel equally native.

That becomes more painful as the ecosystem grows. Custom collection-like types, JSON wrappers, lazily materialized sequences, indexable records, and callable adapters all want to participate in ordinary syntax. If the language does not define a stable protocol, the result is either builtin favoritism or one-off compiler handling per use case.

Python proves the ergonomic value of dunder-shaped hooks. The point here is not to copy Python's dynamic dispatch model wholesale. The point is to adopt the familiar hook vocabulary while keeping the contract static, explicit, and diagnosable in Incan.

## Goals

- Standardize protocol hooks for truthiness, length, iteration, membership, indexing, indexed assignment, and callability.
- Keep the hook surface intentionally small and understandable.
- Make missing-hook failures compile-time diagnostics with actionable messages.
- Preserve static typing for hook arguments and return types.
- Give user-defined types a first-class path to participate in common language syntax without bespoke compiler cases.

## Non-Goals

- Defining slicing in this RFC.
- Reopening the operator-overloading surface from RFC 028.
- Introducing dynamic `Any`-style fallback behavior for syntax hooks.
- Standardizing every possible Python dunder hook in one RFC.
- Defining formatting or numeric-conversion hooks such as `__str__`, `__int__`, or `__float__` here.

## Guide-level explanation

### Truthiness

If a type defines `__bool__`, it can participate in ordinary truthiness:

```incan
class QueryResult:
    def __bool__(self) -> bool:
        return self.count > 0

if results:
    println("have rows")
```

### Length

If a type defines `__len__`, it can participate in `len(...)`:

```incan
class Bucket:
    def __len__(self) -> int:
        return self.size

count = len(bucket)
```

### Iteration

If a type defines `__iter__`, and the returned iterator value defines `__next__`, it can participate in `for` loops:

```incan
for item in rows:
    println(item)
```

The point is that `rows` behaves like a collection because it satisfies the iteration protocol, not because it is a builtin list.

### Membership

If a type defines `__contains__`, it can participate in `in`:

```incan
if user_id in active_users:
    notify(user_id)
```

### Indexing and indexed assignment

If a type defines `__getitem__`, it can participate in read indexing:

```incan
value = cache["users"]
```

If it also defines `__setitem__`, it can participate in indexed assignment:

```incan
cache["users"] = users
```

### Callable objects

If a type defines `__call__`, instances can be invoked like functions:

```incan
class Rule:
    def __call__(self, value: str) -> bool:
        return value != ""

if rule(name):
    println("valid")
```

## Reference-level explanation

### Supported hooks

This RFC standardizes the following hook names and minimum return contracts:

- `__bool__(self) -> bool`
- `__len__(self) -> int`
- `__contains__(self, item: T) -> bool`
- `__iter__(self) -> IteratorLike`
- `__next__(self) -> Option[T]`
- `__getitem__(self, key: K) -> V`
- `__setitem__(self, key: K, value: V) -> None`
- `__call__(self, ...) -> R`

The RFC does not require one nominal `Iterator` trait spelling yet, but it does require the `__iter__` / `__next__` contract.

### Syntax-to-hook mapping

The language must interpret the following syntax through hook resolution:

- `if x:` and similar truthiness contexts use `__bool__`
- `len(x)` uses `__len__`
- `a in b` uses `b.__contains__(a)`
- `for item in xs:` uses `__iter__` on `xs` and `__next__` on the returned iterator
- `obj[key]` uses `__getitem__`
- `obj[key] = value` uses `__setitem__`
- `obj(...)` uses `__call__`

### Static validation

Hook resolution must be static and type-checked.

If a syntax form requires a hook and the relevant type does not provide a compatible hook, the language implementation must emit a compile-time diagnostic naming the missing capability.

Examples:

- using `if x:` on a type without `__bool__`
- calling `len(x)` when `__len__` is absent or returns a non-`int`
- indexing `obj[key]` when `__getitem__` is absent

### Iteration rules

The iteration contract must require:

1. the iterated value supplies `__iter__`
2. the iterator returned by `__iter__` supplies `__next__`
3. `__next__` returns `Option[T]`, where `Some(value)` produces the next item and `None` signals exhaustion

This keeps iteration explicit and typeable without requiring dynamic sentinel behavior.

### No dynamic fallback

This RFC does not allow a dynamic or reflective fallback path when hooks are absent. The language must not silently reinterpret these syntax forms through runtime magic.

## Design details

### Syntax

This RFC does not add new syntax forms. It standardizes how existing syntax forms resolve against user-defined types.

### Semantics

The semantic center is that builtin syntax becomes protocol-driven rather than builtin-only. The hook names are Python-shaped, but the resolution model is intentionally stricter:

- hook lookup is static
- hook signatures are validated
- diagnostics are explicit

### Interaction with existing features

- **RFC 028 (trait-based operator overloading)**: operator syntax remains governed by the operator protocol. This RFC covers non-operator syntax hooks.
- **RFC 030 (`std.collections`)**: custom collections should be able to participate in iteration, membership, and indexing through the standardized hooks.
- **RFC 050 (enum methods and trait adoption)**: enums can participate in these syntax hooks once they can define methods or adopt relevant traits.
- **RFC 051 (`JsonValue`)**: dynamic JSON value access patterns become easier to standardize when indexing and truthiness have a consistent language hook model.

### Compatibility / migration

This feature is additive. Existing builtins continue to behave as they do today. The design claim is that user-defined types gain the same language-surface participation through explicit hooks rather than special-cased compiler treatment.

## Alternatives considered

- **Keep these forms builtin-only**
  - Rejected because it limits user-defined types and invites special-case compiler behavior.
- **Traits only, no named dunder hooks**
  - Rejected because the language still needs one stable surface for syntax resolution, and the Python-shaped hook vocabulary is more immediately legible for Incan users.
- **Dynamic fallback**
  - Rejected because it weakens predictability and diagnostics.

## Drawbacks

- The language grows another protocol surface that users must learn.
- Poorly chosen hook implementations can make syntax behavior surprising even if it type-checks.
- Iteration and indexing hooks can overlap with collection-trait design, so the boundary needs careful documentation.

## Implementation architecture

*(Non-normative.)* A practical implementation can map each syntax form to a hook-resolution path in the same general spirit as RFC 028 operator resolution. Builtins and stdlib types can then be brought under the same documented protocol surface rather than living behind special cases alone.

## Layers affected

- **Language surface**: the supported syntax forms must resolve through the standardized hook names defined by this RFC.
- **Type system**: hook signatures, return types, and syntax-specific validation rules must be enforced statically.
- **Execution handoff**: implementations must preserve the resolved protocol semantics without introducing dynamic fallback behavior.
- **Stdlib / runtime**: builtin and stdlib collection-like types should document how they satisfy these hooks.
- **Docs / tooling**: diagnostics and docs should explain the hook model clearly enough that users can predict syntax participation.

## Unresolved questions

- Should the language also standardize a nominal iterator trait name in this RFC, or is the hook contract alone enough for the initial design?
- Should `__iter__` be allowed to return `self` when the same object also defines `__next__`, or should iterator and iterable roles stay distinct?
- Should `len(...)` remain strictly `int`, or should it allow a future sized-integer widening story?
- How should missing-key or missing-index behavior for `__getitem__` be positioned in docs without forcing one universal policy on all types?
- Which additional non-operator hooks, if any, belong in a follow-up RFC after this initial set?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
