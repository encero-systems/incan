# RFC 068: protocol hooks for core language syntax

- **Status:** Implemented
- **Created:** 2026-04-16
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 028 (trait-based operator overloading)
    - RFC 030 (`std.collections`)
    - RFC 050 (enum methods and enum trait adoption)
    - RFC 051 (`JsonValue` for `std.json`)
- **Issue:** [#86](https://github.com/dannys-code-corner/incan/issues/86)
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** v0.3

## Summary

This RFC proposes a small set of compiler-recognized protocol hooks for core language syntax so user-defined types can participate in truthiness, `len(...)`, iteration, membership, indexing, assignment-through-indexing, and callability with predictable static typing and clear diagnostics. The surface is deliberately Python-shaped (`__bool__`, `__len__`, `__iter__`, `__next__`, `__contains__`, `__getitem__`, `__setitem__`, `__call__`) while remaining a statically checked language contract rather than dynamic runtime magic.

## Core model

Read this RFC as one foundation plus four mechanisms:

1. **Foundation:** certain core syntax forms need a stable user-definable protocol rather than builtin-only treatment.
2. **Mechanism A:** syntax such as `if x:`, `len(x)`, `for item in xs:`, `a in b`, `obj[key]`, `obj[key] = value`, and `obj(...)` resolves through a small set of named hooks.
3. **Mechanism B:** each hook has an accompanying nominal trait vocabulary so generic code can name the capability directly.
4. **Mechanism C:** hook resolution is static and type-checked; there is no ambient dynamic fallback.
5. **Mechanism D:** operator overloading stays governed by RFC 028; this RFC covers non-operator core syntax surfaces.

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
- `__iter__(self) -> Iterator[T]`
- `__next__(self) -> Option[T]`
- `__getitem__(self, key: K) -> V`
- `__setitem__(self, key: K, value: V) -> None`
- `__call__(self, ...) -> R`

### Hook-to-trait vocabulary

Hooks are the method-level implementation surface. Traits are the nominal capability vocabulary that authors can use in `with` clauses, bounds, docs, and diagnostics.

| Syntax surface     | Hook                                             | Trait vocabulary                                                                                        |
| ------------------ | ------------------------------------------------ | ------------------------------------------------------------------------------------------------------- |
| Truthiness         | `__bool__(self) -> bool`                         | `Bool`                                                                                                  |
| Length             | `__len__(self) -> int`                           | `Len`                                                                                                   |
| Membership         | `__contains__(self, item: T) -> bool`            | `Contains[T]`                                                                                           |
| Iteration source   | `__iter__(self) -> Iterator[T]`                  | `Iterable[T]`                                                                                           |
| Iterator step      | `__next__(self) -> Option[T]`                    | `Iterator[T]`                                                                                           |
| Read indexing      | `__getitem__(self, key: K) -> V`                 | `Index[K, V]`                                                                                           |
| Indexed assignment | `__setitem__(self, key: K, value: V) -> None`    | `IndexMut[K, V]`                                                                                        |
| Callable object    | `__call__(self, ...) -> R`                       | fixed-arity callable traits such as `Callable0[R]`, `Callable1[A, R]`, and `Callable2[A, B, R]`         |

The implementation must keep hook compatibility and trait adoption aligned: if a type claims one of these traits, its corresponding hook signature must type-check against the trait contract; if syntax resolves through a hook, diagnostics should mention the named capability where that helps the author fix the type. Syntax may resolve through a compatible hook even when the type has not explicitly adopted the nominal trait; explicit trait adoption is required when generic code wants to name the capability as a bound.

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

## Implementation Plan

### Phase 1: Protocol vocabulary and stdlib contracts

- Canonicalize the hook-to-trait mapping across the stdlib source, generated docs, and language reference.
- Treat `Index[K, V]` and `IndexMut[K, V]` as the canonical RFC 068 indexing trait names; preserve or route any existing `GetItem` / `SetItem` vocabulary as compatibility/operator aliases rather than a second source of truth.
- Confirm `Bool`, `Len`, `Contains[T]`, `Iterable[T]`, `Iterator[T]`, and fixed-arity callable traits expose signatures that match this RFC.
- Document the truthiness policy carefully: `Bool` is supported, but user docs should still prefer explicit checks for `Option`, `Result`, emptiness, and named boolean state.

### Phase 2: Hook lookup and typechecking

- Add static hook lookup for truthiness contexts, `len(...)`, membership, iteration, read indexing, indexed assignment, and callable object invocation.
- Validate hook signatures and return types against the RFC contracts, including `bool`, `int`, `Option[T]`, indexed value types, and callable result types.
- Allow syntax to resolve through compatible hooks structurally, while using nominal traits for explicit `with` clauses and generic bounds.
- Emit span-precise diagnostics that name the missing or incompatible capability and point at the relevant syntax form.

### Phase 3: Lowering, emission, and builtin parity

- Lower resolved protocol syntax through the same checked call path used for ordinary method calls or existing operator protocol handling.
- Preserve current builtin behavior for lists, dicts, strings, range/iterator-like values, and existing callable function values.
- Bring builtin and stdlib collection-like types under the documented protocol surface where the implementation already has equivalent behavior.
- Keep `Option` and `Result` explicitly non-truthy unless they define `Bool` in a separate accepted design.

### Phase 4: Tests, docs, and release integration

- Add parser/typechecker/codegen or integration tests for each supported syntax surface, including invalid-hook diagnostics.
- Add callable-object coverage for fixed arity and existing rest/keyword call binding behavior where applicable.
- Update authored user-facing docs for protocol hooks, collection traits, indexing, callable objects, and truthiness guidance.
- Add active-release notes coverage and bump the active `0.3.0-dev.N` version before closeout.
- Run targeted checks first, then the repository pre-commit gate before presenting the branch as review-ready.

## Implementation Log

### Spec / design

- [x] Move RFC 068 to `In Progress` now that implementation has been picked up.
- [x] Resolve the hook-vs-trait question with a trait-backed protocol model.
- [x] Canonicalize indexing vocabulary around `Index` / `IndexMut` and compatibility aliases.
- [x] Confirm callable-object arity policy against existing function/callable type support.
- [x] Confirm structural hook resolution versus explicit nominal trait adoption in typechecker design notes.

### Stdlib / trait surface

- [x] Align `std.traits.*` and `std.derives.*` trait declarations with the RFC hook signatures.
- [x] Ensure prelude/export paths expose the canonical protocol traits consistently.
- [x] Update collection/indexing/callable trait docs for the canonical vocabulary.

### Typechecker

- [x] Implement truthiness hook validation for `if` / `while` contexts.
- [x] Implement `len(...)` hook validation.
- [x] Implement membership hook validation for `in` / `not in`.
- [x] Implement iteration hook validation for `for` loops.
- [x] Implement read indexing hook validation.
- [x] Implement indexed assignment hook validation.
- [x] Implement callable-object hook validation.
- [x] Add diagnostics for missing hooks and incompatible hook signatures.

### Lowering / emission

- [x] Lower resolved truthiness hooks correctly.
- [x] Lower resolved `len(...)` hooks correctly.
- [x] Lower resolved membership hooks correctly.
- [x] Lower resolved iteration hooks correctly.
- [x] Lower resolved read indexing hooks correctly.
- [x] Lower resolved indexed assignment hooks correctly.
- [x] Lower resolved callable-object hooks correctly.
- [x] Verify builtin parity for existing list, dict, string, iterator/range, and callable behavior.

### Tests

- [x] Add valid custom-type tests for each protocol hook.
- [x] Add invalid missing-hook diagnostics for each syntax surface.
- [x] Add invalid return-type/signature diagnostics for each hook family.
- [x] Add tests showing explicit trait adoption works with matching hooks.
- [x] Add tests showing structural hook syntax works without requiring explicit trait adoption.
- [x] Add tests for `Option` / `Result` non-truthiness unless a separate accepted design changes that rule.

### Docs / release

- [x] Update language reference pages for protocol hooks and trait vocabulary.
- [x] Update how-to or tutorial docs where users learn collection/callable/truthiness behavior.
- [x] Add release notes for the active `0.3` development line.
- [x] Bump the active development version.
- [x] Run the full repository gate before closeout.

## Design Decisions

- This RFC standardizes both hook names and accompanying nominal trait vocabulary. Hooks remain the implementation methods, while traits such as `Bool`, `Len`, `Contains[T]`, `Iterable[T]`, `Iterator[T]`, indexing traits, and fixed-arity callable traits are the capabilities generic code can name.
- Syntax may resolve structurally through a compatible hook even when the type has not explicitly adopted the corresponding trait. Explicit trait adoption is still the nominal surface for trait bounds, generic constraints, docs, and clearer diagnostics.
- `Index[K, V]` and `IndexMut[K, V]` are the canonical RFC 068 names for read indexing and indexed assignment. Existing `GetItem` / `SetItem` vocabulary should be treated as compatibility/operator aliases, not as an independent semantic surface.
- `__iter__` may return `self` when the same object also defines a compatible `__next__`. Iterator and iterable roles remain semantically distinct, but the same value may satisfy both roles when its static type supports both hooks.
- `len(...)` remains strictly `int` for this RFC. Any future sized-integer widening story must be proposed separately and must define compatibility rules for existing `__len__` implementations.
- `__getitem__` controls lookup semantics for the target type. This RFC requires static hook compatibility and result typing, but it does not impose one universal missing-key or out-of-bounds policy; individual types may return an optional/result value or raise/report an error according to their documented contract.
- Additional non-operator hooks are follow-up RFC territory. This RFC intentionally stops at truthiness, length, iteration, membership, indexing, indexed assignment, and callability.
