---
search:
  boost: 3
tags:
  - duckborrowing
  - ownership planning
  - generated rust
  - borrow
  - move
  - clone
---

# Duckborrowing

Duckborrowing is the backend ownership-planning layer that lets Incan keep value-oriented source semantics while emitting valid, predictable Rust. It is not a source-language feature and it is not a second Rust borrow checker. It is the compiler-side policy for deciding when generated Rust should move, borrow, mutably borrow, clone, convert with `.into()`, or materialize owned `String` storage.

The ownership module states the local contract directly: keep emitter modules calling the planner instead of open-coding ad hoc `.clone()`, `&`, `&mut`, `.to_string()`, or `.into()` decisions.

## Goals

- Keep Rust ownership details out of ordinary Incan code.
- Centralize ownership decisions so bug fixes improve whole classes of generated code.
- Preserve last-use moves where the IR proves a value can be consumed.
- Materialize owned values at Incan storage boundaries, especially for strings and non-`Copy` borrowed values.
- Borrow at Rust interop and helper boundaries where the Rust API expects references.
- Add required generic `Clone` bounds when the backend, not the source program, introduces a clone.

## Non-goals

- Duckborrowing must not become "clone until Rust compiles".
- It must not hide frontend typing mistakes or backend lowering bugs.
- It must not require users to add `.clone()`, `.as_ref()`, `str(...)`, or `.into()` as codegen escape hatches.
- It must not encode ownership policy separately in every emitter.

## Where it runs

Duckborrowing lives in the backend IR path:

```text
typed AST
  -> lowering records value shapes and VarAccess
  -> ownership planner chooses a use-site conversion
  -> emitters apply the selected conversion
  -> trait-bound inference mirrors backend-inserted clones for generics
  -> generated Rust
```

The main files are:

| File                                                            | Responsibility                                                                                   |
| --------------------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| `src/backend/ir/ownership.rs`                                   | Public ownership-planning facade. Defines `ValueUseSite` and use-site helpers.                   |
| `src/backend/ir/conversions.rs`                                 | Core conversion policy. Returns `None`, `ToString`, `Into`, `Borrow`, `MutBorrow`, or `Clone`.   |
| `src/backend/ir/emit/expressions/mod.rs`                        | Applies `emit_expr_for_use` recursively for expressions, literals, tuples, and match scrutinees. |
| `src/backend/ir/emit/expressions/calls.rs`                      | Applies call-argument ownership policy for Incan and external Rust calls.                        |
| `src/backend/ir/emit/expressions/methods/collection_methods.rs` | Plans collection receiver/key borrows, including string lookup probes.                           |
| `src/backend/ir/emit/statements.rs`                             | Applies assignment, return, match, dict-assignment, and loop ownership policy.                   |
| `src/backend/ir/lower/stmt.rs`                                  | Marks move-vs-read access for statement lowering, including tuple unpacking.                     |
| `src/backend/ir/trait_bound_inference.rs`                       | Adds generic trait bounds needed by backend-inserted clones.                                     |
| `src/backend/ir/types.rs`                                       | Defines `IrType::is_copy()` so planning can distinguish cheap copies from owned materialization. |

## Use-site model

Every ownership decision starts with a typed use site. `ValueUseSite` is the boundary contract between emitters and the planner:

| Use site            | Meaning                                                                                                |
| ------------------- | ------------------------------------------------------------------------------------------------------ |
| `IncanCallArg`      | Argument to an Incan-defined callable. Parameters generally receive owned Incan values.                |
| `ExternalCallArg`   | Argument to a Rust interop callable. Rust APIs often receive borrowed values or custom `Into` targets. |
| `StructField`       | Value stored into an owned generated Rust struct field.                                                |
| `CollectionElement` | Value stored into a list, set, dict, or tuple literal.                                                 |
| `Assignment`        | Value assigned to a binding or existing place.                                                         |
| `ReturnValue`       | Value returned from a function.                                                                        |
| `MatchScrutinee`    | Value consumed by generated Rust `match`.                                                              |
| `MethodArg`         | Argument to method-style lowering where the method controls borrow behavior.                           |

Emitters should call `emit_expr_for_use(expr, site)` or `plan_value_use(expr, site)` instead of applying ownership tokens directly.

## Decision order

The planner should answer these questions in this order:

1. What use site is consuming the value?
2. Is there a target type from the typechecker, field, parameter, collection element, assignment, or return signature?
3. Does lowering mark the value as `VarAccess::Move`, or must the source remain usable?
4. Is the source already a borrow, a static string, a field read, or a borrowed method-chain result such as `as_ref()`?
5. Is the source type `Copy` according to `IrType::is_copy()`?
6. Does the target require owned Incan storage or a Rust interop shape?
7. If the emitted plan clones a generic value, did trait-bound inference add the matching `Clone` bound?

That ordering matters. Last-use moves should win over defensive cloning. Borrowed materialization should win over passing `&T` into an owned Incan sink. Rust interop should preserve the shape the Rust API expects instead of forcing Incan-owned values everywhere.

## Core policies

### Owned Incan sinks

Incan function arguments, struct fields, collection elements, assignments, returns, and match scrutinees are owned value sinks unless the specific sink says otherwise. At these boundaries:

- String literals and static strings materialize to owned `String` when the target is `str`, generic, or otherwise inferred as owned Incan storage.
- Non-`Copy` variables move when lowering marks them as `VarAccess::Move`.
- Non-`Copy` variables clone when the value must remain usable after the use site.
- Field reads clone when moving the field would move out of a parent object that remains borrowed or owned elsewhere.
- Borrowed `as_ref()` and interop-unwrapped borrowed results clone when the sink expects an owned Incan value.

### Rust interop sinks

External Rust calls are different from Incan calls. They often accept references, custom string wrappers, or generic `Into` targets. At these boundaries:

- String-like values use `.into()` when the Rust target may resolve through `Into`.
- Non-string non-`Copy` values generally borrow rather than clone.
- Mutable aggregate Incan parameters can reborrow as `&mut T` when the callee parameter requires mutation.

### Borrowed callable parameters

Function-typed parameters can be refined by lowering when their body uses a non-`Copy` payload as an observer and then keeps using that payload afterward. For example, a helper such as `tap[T, E](result: Result[T, E], f: Callable[T, None]) -> Result[T, E]` can call `f(value)` and then return `Ok(value)` without requiring `T: Clone`. The generated callable boundary becomes `fn(&T) -> ()`, the callback call borrows `value`, and the original branch payload remains available to move into the returned `Result`.

This is a backend ownership decision, not new source syntax. Incan code still writes `Callable[T, None]`; the compiler records the borrowed callable shape in IR when the use site proves that borrowing is the correct ownership plan. Named function values passed to such a borrowed function-pointer parameter receive a private borrowed adapter when their source signature takes the payload by value.

### Collections and tuples

Collection and tuple literals recursively apply `CollectionElement` planning to each item. This prevents nested string/borrow issues from escaping through literal syntax.

Examples of expected behavior:

- `["a", "b"]` stores `String` elements when the list element type is `str`.
- `{"id": value}` materializes the key and value according to the dict key/value types.
- `(borrowed.as_ref(), "x")` clones or materializes items when the tuple is stored as owned data.

### Lookup probes

Lookup and membership probes should not allocate just because the stored collection owns strings. For dict and set string keys, the planner can emit `AsRef<str>` probe shapes so `String`, `&str`, and static string values can all look up owned string keys without extra user code.

### Match scrutinees

Generated Rust `match` consumes the scrutinee shape. The planner treats match scrutinees like owned-result materialization so borrowed or shared non-`Copy` values are cloned before matching when needed.

### Generic clone bounds

Backend-inserted `.clone()` calls are invisible to source-level trait-bound inference unless the backend mirrors them. When ownership planning can clone a generic value, `src/backend/ir/trait_bound_inference.rs` must add the corresponding `Clone` bound. Otherwise the generated Rust may fail only after codegen.

## Contributor rules

- Do not fix ownership bugs by adding local `.clone()`, `&`, `&mut`, `.as_ref()`, `.to_string()`, or `.into()` in an emitter unless the operation is truly local to a Rust helper API.
- If a new boundary consumes a value, add or reuse a `ValueUseSite`.
- If a new source expression can produce borrowed data, teach `conversions.rs` how it materializes at owned sinks.
- If a new sink stores values recursively, route children through `emit_expr_for_use`.
- If a generic value can be cloned by backend policy, update trait-bound inference in the same change.
- If a callable parameter is refined from owned to borrowed payloads, make trait-bound inference follow the borrowed target so it does not add `Clone` merely because the original source spelling was `Callable[T, None]`.
- If a library or consumer needs workaround calls to compile, treat that as evidence of a missing planner rule.

## Testing expectations

Ownership changes need tests at the layer where the behavior is decided and at least one generated-Rust check when the bug was observable only after emission.

Use these test shapes:

- Conversion planner unit tests in `src/backend/ir/conversions.rs` for pure policy decisions.
- IR/codegen snapshot tests for emitted Rust shapes.
- Build or run tests for generated Rust when borrow checker behavior is the failure mode.
- Consumer checks from sibling projects when the bug came from real library patterns rather than a minimized fixture.
- Trait-bound inference tests when a backend-inserted clone touches generic `T`.
- User-authored helper tests when a stdlib feature depends on duckborrowing; the same pattern should work outside `std`.

## Review checklist

Before merging duckborrowing changes, answer these questions:

1. What exact use site is being planned?
2. What target type, if any, reaches the planner?
3. Is the source a move, read, borrow, field read, static value, or borrowed method-chain result?
4. Is the source `Copy` under `IrType::is_copy()`?
5. Does the fix improve a class of ownership bugs, or only one emitter branch?
6. Are generic `Clone` bounds inferred when needed?
7. Is there a regression test that would fail without the planner rule?
