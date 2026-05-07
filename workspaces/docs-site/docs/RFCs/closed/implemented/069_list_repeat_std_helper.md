# RFC 069: `list.repeat` Helper for Fixed-Length Initialization

- **Status:** Implemented
- **Created:** 2026-04-17
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 030 (std collections baseline)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/385
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** v0.3.0-dev.38

## Summary

This RFC proposes one import-free standard-library helper, `list.repeat[T](value: T, count: int) -> list[T]`, to create fixed-length lists initialized with the same value, so authors can express intent directly without manual append loops or sentinel-map boilerplate.

## Motivation

In current Incan code, fixed-length initialization often appears as repeated patterns like “create empty list, iterate N times, append default/sentinel value.” This is mechanically correct but noisy, harder to scan, and easy to get subtly wrong in graph and indexing code. Comprehensions reduce some noise, but the core intent is still “repeat this value N times,” and that intent deserves a dedicated API.

This is also a practical interop concern: Incan lowers to Rust, where this operation is first-class and widely used (for example, `vec![-1_i64; nodes.len()]`).

Python-first users are used to concise list initialization and should have an equally direct, explicit operation in Incan, without having to hand-write append loops for a common pattern. A standard helper keeps author intent clear while still matching Rust-side semantics (`Clone`-based element repetition).

## Goals

- Add one canonical helper for repeated list initialization on the built-in `list` surface.
- Improve readability for sentinel maps and pre-sized working buffers.
- Keep rollout lightweight by avoiding parser syntax changes.
- Make semantics explicit for count validation and element cloning.

## Non-Goals

- This RFC does not introduce new list literal syntax (for example Rust-style `vec![x; n]` sugar).
- This RFC does not add capacity-only allocation APIs.
- This RFC does not change existing list comprehension semantics.
- This RFC does not introduce deep-copy semantics beyond existing `Clone` behavior.
- This RFC does not define callable-driven generation such as `list.generate(count, fn(i) -> T)`.

## Guide-level explanation

Use `list.repeat` when you want a list of length `N` where every element starts from the same value.

```incan
ids = list.repeat(-1, 8)
# ids == [-1, -1, -1, -1, -1, -1, -1, -1]
```

This is especially useful for id maps and temporary buffers:

```incan
def unmapped_ids[T](items: list[T]) -> list[int]:
    return list.repeat(-1, len(items))
```

For non-primitive values, the helper follows normal `Clone` semantics and requires cloneable element types.

## Reference-level explanation

The standard library must provide:

```incan
list.repeat[T with Clone](value: T, count: int) -> list[T]
```

Normative behavior:

- `count == 0` must return an empty list.
- `count > 0` must return a list of length `count`.
- Each returned element must be clone-derived from `value`.
- The function must not return aliased mutable storage that would violate normal value semantics expected from cloned elements.
- `count < 0` must raise a runtime `ValueError` whose message identifies `list.repeat`, says `count` must be non-negative, and includes the provided count value.

Type rules:

- `T` must satisfy `Clone`.
- The return type must be `list[T]` with no widening.

Complexity:

- Time complexity should be linear in `count`.
- Space complexity should be linear in `count`.

## Design details

### API location

The helper belongs on the built-in `list` surface and must be available without an import, like existing built-in collection constructors and helpers.

### Why helper over syntax

This feature is a reusable collection operation, not a new language form. Keeping it as a helper avoids parser syntax churn and keeps behavior testable as a normal API.

### Clone semantics

`repeat` should behave like repeated `append(value.clone())` for each element. This keeps behavior aligned with existing trait expectations and avoids introducing custom duplication semantics.

### Error behavior

Negative `count` is a caller error and should fail at runtime with `ValueError` instead of silently returning empty output. The diagnostic should include the bad count value because runtime inputs are often computed and a fixed message would make the failure less actionable.

## Alternatives considered

- Add list literal repetition syntax (for example `[value; count]`): rejected for now because it expands language grammar for a problem that a stdlib API can solve cleanly.
- Add `std.collections.list.repeat` behind an explicit import: rejected because `list` is already the built-in user-facing list surface, and repeated initialization is a core list operation rather than a specialized collection type from RFC 030.
- Keep using comprehensions (`[x for _ in range(n)]`): acceptable but less direct when the intent is repeated initialization rather than transformation.
- Keep explicit loops with append: most verbose and least declarative for this use-case.
- Add `list.generate(count, fn(i) -> T)` in the same RFC: rejected for this RFC because generation is a callable-driven construction API with index and invocation semantics, while `repeat` is a clone/count helper.

## Drawbacks

- Adds another API surface that overlaps with patterns already expressible via comprehensions and loops.
- Requires `Clone` for repeated values, which can reject some types that authors might expect to repeat.
- Introduces one more runtime error path (`count < 0`) that callers must handle in dynamic inputs.

## Implementation architecture

Recommended implementation is a straightforward runtime helper that validates the `count` guard and appends `value.clone()` `count` times, with compiler/typechecker wiring only where needed to expose the import-free `list.repeat` surface. Tests should cover zero, positive, negative, and non-primitive cloneable values.

## Layers affected

- **Stdlib / Runtime (`incan_stdlib`)**: add the backing repeated-list helper and runtime tests.
- **Typechecker / Symbol resolution**: recognize `list.repeat` as an import-free built-in collection helper, enforce arity and `Clone` compatibility, and return `list[T]`.
- **Lowering / Emission**: lower recognized `list.repeat` calls to the runtime helper while preserving type-directed clone behavior.
- **LSP / Tooling**: surface completion/hover docs for the new stdlib function.
- **Documentation**: update collections reference/how-to examples.

## Implementation Plan

### Phase 1: Spec and built-in collection surface

- Finalize the import-free `list.repeat` contract, negative-count diagnostic shape, and callable-generation non-goal.
- Add a canonical built-in helper identity for `list.repeat` that does not require `std.collections` imports.

### Phase 2: Typechecker and lowering

- Type-check `list.repeat(value, count)` with exactly two arguments.
- Infer `T` from `value`, require `T` to satisfy `Clone`, require `count: int`, and return `list[T]` without widening.
- Preserve enough call identity through lowering so emission does not rely on ad hoc string matching.

### Phase 3: Runtime and emission

- Add the runtime helper that validates non-negative counts and builds the repeated list through clone-derived elements.
- Emit recognized `list.repeat` calls to the runtime helper.
- Ensure the negative-count `ValueError` includes the provided count.

### Phase 4: Tooling, docs, and release readiness

- Surface completion/hover docs for `list.repeat`.
- Update authored collection docs and examples where fixed-length initialization is taught.
- Add a release notes entry and bump the active development version.

## Implementation log

### Spec / design

- [x] Resolve RFC design questions and record `list.repeat` as an import-free built-in collection helper.
- [x] Exclude callable-driven `list.generate` from this RFC.

### Typechecker / symbol resolution

- [x] Recognize `list.repeat` without requiring an import.
- [x] Validate arity, `count: int`, and `T with Clone`.
- [x] Return `list[T]` without widening.

### Lowering / emission

- [x] Preserve recognized `list.repeat` identity through lowering.
- [x] Emit recognized calls to the runtime helper.

### Stdlib / runtime

- [x] Add the repeated-list runtime helper.
- [x] Raise rich `ValueError` diagnostics for negative counts.

### Tests

- [x] Add typechecker coverage for valid calls and invalid arity/count/clone-bound cases.
- [x] Add codegen snapshot or integration coverage for zero and positive counts.
- [x] Add runtime coverage for negative counts.
- [x] Add non-primitive cloneable-value coverage.

### Tooling / docs / release

- [x] Surface LSP completion/hover docs for `list.repeat`.
- [x] Update authored collection documentation/examples.
- [x] Add release notes entry.
- [x] Bump the active development version.

## Design Decisions

- `list.repeat` is an import-free helper on the built-in `list` surface. It does not live only behind `std.collections`, and RFC 069 does not add a separate top-level alias.
- Negative-count diagnostics should be rich: the runtime `ValueError` includes the provided `count` value as well as the non-negative-count requirement.
- Callable-driven list construction, such as `list.generate(count, fn(i) -> T)`, is a separate feature. It is not part of RFC 069 because it requires callable invocation semantics rather than clone repetition semantics.
