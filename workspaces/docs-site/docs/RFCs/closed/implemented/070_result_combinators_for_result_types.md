# RFC 070: Result Combinators for `Result[T, E]`

- **Status:** Implemented
- **Created:** 2026-04-18
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 000 (core error handling and `Result` model)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/386
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** v0.3

## Summary

This RFC proposes adding first-class combinator methods to `Result[T, E]` in Incan, specifically `map`, `map_err`, `and_then`, `or_else`, `inspect`, and `inspect_err`, so fallible pipelines can be expressed directly without repetitive nested `match` scaffolding.

## Core model

1. `Result` stays explicit and typed; this RFC adds composition methods, not new error semantics.
2. Transformation combinators are pure transformations over `Ok` and `Err` branches; inspection combinators observe one branch and return the original `Result` unchanged.
3. `?` remains the primary early-return mechanism; combinators complement `?` when values must be transformed in-line.

Mechanisms:

- `map`: transform `Ok(T)` into `Ok(U)` while preserving `Err(E)`.
- `map_err`: transform `Err(E)` into `Err(F)` while preserving `Ok(T)`.
- `and_then`: chain `Result`-returning operations on `Ok(T)`.
- `or_else`: recover or remap from `Err(E)` with a `Result`-returning function.
- `inspect`: call a side-effecting observer with an implicitly borrowed `Ok(T)` payload while preserving the original `Result[T, E]`.
- `inspect_err`: call a side-effecting observer with an implicitly borrowed `Err(E)` payload while preserving the original `Result[T, E]`.

## Motivation

Current Incan code frequently repeats nested `match` expressions for straightforward `Result` branch transformations. This is correct but verbose, especially in backend adapters and interop-heavy code where planning, execution, and sink errors are repeatedly remapped to project-specific error types.

The language already supports `Result[T, E]` and `?`, but it lacks the compositional API surface users expect from Rust-shaped error handling. This gap leads to noisy code and pushes teams toward local helper functions for patterns that should be standard on `Result` itself.

## Goals

- Add canonical compositional methods on `Result[T, E]`.
- Reduce boilerplate for error/value transformation pipelines.
- Keep naming and semantics aligned with Rust conventions to reduce cognitive overhead.
- Preserve compatibility with existing `match` and `?` patterns.
- Support observation taps for success and failure branches without changing the carried `Result`.

## Non-Goals

- This RFC does not replace `match` as a control-flow construct.
- This RFC does not change `?` behavior.
- This RFC does not introduce exception-style implicit error propagation.
- This RFC does not define async-specific combinators in this slice.
- This RFC does not introduce new union error syntax beyond existing `Result` typing rules.
- This RFC does not add Python-style aliases for Rust-shaped combinator names.

## Guide-level explanation

Use `Result` combinators when the operation is branch transformation rather than multi-step control flow.

```incan
def parse_int(raw: str) -> Result[int, str]:
    ...

def validate_positive(v: int) -> Result[int, str]:
    ...

def prefix_port_error(e: str) -> str:
    return f"invalid port: {e}"

def normalize_port(raw: str) -> Result[int, str]:
    return parse_int(raw).and_then(validate_positive).map_err(prefix_port_error)
```

Use `map` when only success changes:

```incan
def read_len(path: str) -> Result[int, str]:
    return read_text(path).map(len)
```

Use `or_else` when recovering from failure:

```incan
def read_default_text(_err: str) -> Result[str, str]:
    return Ok("default")

def read_with_default(path: str) -> Result[str, str]:
    return read_text(path).or_else(read_default_text)
```

Use `inspect` and `inspect_err` when logging, metrics, or trace hooks need to observe a branch without changing the pipeline value:

```incan
def trace_text(text: str) -> None:
    println(f"read {len(text)} bytes")

def trace_error(err: str) -> None:
    println(f"read failed: {err}")

def read_traced(path: str) -> Result[str, str]:
    return read_text(path).inspect(trace_text).inspect_err(trace_error)
```

`match` remains preferred when both branches require substantial logic or side effects.

## Reference-level explanation

Incan must expose the following methods on `Result[T, E]`:

```incan
def map[U](self, f: Callable[T, U]) -> Result[U, E]
def map_err[F](self, f: Callable[E, F]) -> Result[T, F]
def and_then[U](self, f: Callable[T, Result[U, E]]) -> Result[U, E]
def or_else[F](self, f: Callable[E, Result[T, F]]) -> Result[T, F]
def inspect(self, f: Callable[T, None]) -> Result[T, E]
def inspect_err(self, f: Callable[E, None]) -> Result[T, E]
```

Normative behavior:

- `map` must call `f` exactly once for `Ok(T)` and must not call `f` for `Err(E)`.
- `map_err` must call `f` exactly once for `Err(E)` and must not call `f` for `Ok(T)`.
- `and_then` must call `f` exactly once for `Ok(T)` and must propagate `Err(E)` unchanged.
- `or_else` must call `f` exactly once for `Err(E)` and must propagate `Ok(T)` unchanged.
- `inspect` must call `f` exactly once for `Ok(T)` with an implicitly borrowed success payload, must not call `f` for `Err(E)`, and must return the original `Ok(T)` or `Err(E)` branch unchanged.
- `inspect_err` must call `f` exactly once for `Err(E)` with an implicitly borrowed error payload, must not call `f` for `Ok(T)`, and must return the original `Ok(T)` or `Err(E)` branch unchanged.
- All combinators must preserve left-to-right evaluation order of receiver then closure.
- All combinators must remain type-safe under existing generic bound rules.

Diagnostics:

- When function argument arity or return type is incompatible with a combinator contract, the compiler must emit a type error at the combinator call site.

## Design details

### Naming

Names intentionally mirror Rust (`map`, `map_err`, `and_then`, `or_else`, `inspect`, `inspect_err`) for predictability and interop mental-model alignment. This RFC does not add alternate Python-style names because Python's ordinary error-handling model is `try` / `except`, not an exact named `Result` combinator API. Documentation may explain the intent in Python-friendly prose, but examples and completions should present the canonical Rust-shaped names.

### Closure requirements

Callable argument and return types must satisfy each combinator signature exactly under existing callable typing and generic substitution rules. `Callable[Params, R]` is the source-facing vocabulary for these method signatures; implementations may desugar that spelling to the canonical function type internally. Callable objects that satisfy the relevant fixed-arity callable hook contract should be accepted wherever ordinary call checking already accepts them. For `inspect` and `inspect_err`, the source-facing callback still names the payload type (`Callable[T, None]` or `Callable[E, None]`), but the compiler must adapt the call so the observer receives a borrow of the branch payload and the original `Result` can be returned unchanged.

### Interaction with `?`

`?` remains the best tool for early exit in sequential code. Combinators should be used where local transformation or chaining is clearer than introducing a `match` or temporary bindings.

### Backward compatibility

This is additive API surface. Existing `Result` code using `match` and `?` remains valid.

## Alternatives considered

- Keep current style with helper functions + `match`: rejected because it duplicates a standard abstraction and fragments style.
- Add only `map_err`: rejected because it solves only one branch of the composition problem.
- Introduce pipeline syntax sugar instead of methods: rejected because method-based API is lower risk and easier to stage.
- Rename combinators to Python-style alternatives: rejected because semantic drift from Rust increases learning and documentation burden.
- Defer `inspect` and `inspect_err`: rejected because observation taps are small, coherent additions to the `Result` method family and do not require new error semantics.

## Drawbacks

- Adds more methods to learn for beginners already learning `Result` and `?`.
- Can be overused in places where explicit `match` is clearer.
- Requires high-quality diagnostics to avoid confusing generic/closure mismatch errors.

## Implementation architecture

Recommended approach is to model combinators as stdlib-visible methods on `Result` with lowering that reuses existing match-like branch semantics, so runtime behavior remains explicit and unsurprising while source ergonomics improve.

## Layers affected

- **Typechecker / Symbol resolution**: method signatures on `Result`, callable compatibility checks, generic substitution for combinator methods, and call-site diagnostics for incompatible observer or transformer functions.
- **IR Lowering**: lowering for combinator calls on `Result` receivers.
- **Emission**: stable Rust emission for combinator method calls with correct closure typing.
- **Stdlib / Runtime (`incan_stdlib`)**: `Result` method surface exposure where required by runtime bridge strategy.
- **LSP / Tooling**: completion, hover, and diagnostics for new `Result` methods.
- **Documentation**: error-handling reference and tutorial updates.

## Implementation Plan

### Phase 1: RFC lifecycle, design baseline, and planning

- Move the RFC to active implementation state with a settled design tail.
- Confirm the source-of-truth development version and the user-facing documentation pages that must change.
- Create Ralph loop slice state for typechecking/lowering/emission, stdlib/tooling/docs, and integration verification.

### Phase 2: Typechecker and method resolution

- Register the `Result[T, E]` combinator method surface with `Callable[...]`-shaped argument contracts.
- Validate success, error, chained, recovery, and observer callable signatures with call-site diagnostics.
- Preserve existing `match` and `?` behavior.

### Phase 3: Lowering and emission

- Lower each combinator call to explicit branch-preserving behavior or equivalent generated Rust.
- Preserve receiver-then-callable evaluation order.
- Ensure `inspect` and `inspect_err` return the original branch value unchanged after observer execution.

### Phase 4: Tests, docs, and release metadata

- Add focused typechecker coverage for valid and invalid combinator usage.
- Add codegen or integration coverage proving emitted behavior for all six methods.
- Update authored error-handling docs and release notes.
- Bump the active development version by one dev increment.

## Implementation log

### Spec / design

- [x] Settle method family: include `map`, `map_err`, `and_then`, `or_else`, `inspect`, and `inspect_err`.
- [x] Settle callable vocabulary: use `Callable[...]` rather than `Fn[...]`.
- [x] Settle naming: keep Rust-shaped method names only; do not add Python-style aliases.

### Typechecker / symbol resolution

- [x] Register `Result[T, E]` combinator method signatures.
- [x] Validate transformer callable argument and return types.
- [x] Validate observer callable argument and `None` return types, with compiler-owned borrowed-payload adaptation.
- [x] Emit call-site diagnostics for incompatible arity or return types.

### Lowering / IR

- [x] Lower `map` and `map_err`.
- [x] Lower `and_then` and `or_else`.
- [x] Lower `inspect` and `inspect_err` with borrowed-payload observer calls that do not change the original branch value.

### Emission

- [x] Emit stable Rust for all six combinator methods.
- [x] Preserve receiver-then-callable evaluation order.
- [x] Preserve existing `Result`, `match`, and `?` behavior.

### Stdlib / tooling

- [x] Expose the `Result` method surface where the stdlib/runtime bridge requires it.
- [x] Dogfood value-transforming combinator semantics through `std.result` Incan helpers where current callable typing can express the contract.
- [x] Confirm no separate LSP hover/completion registry exists for compiler-owned collection methods today.

### Tests

- [x] Add typechecker tests for valid `map`, `map_err`, `and_then`, `or_else`, `inspect`, and `inspect_err`.
- [x] Add typechecker diagnostics tests for bad callable arity and bad return types.
- [x] Add codegen snapshot or integration tests for all six methods.
- [x] Run targeted verification for touched compiler layers.
- [x] Run the repo-level gate.

### Docs / release

- [x] Update authored error-handling docs with combinator guidance.
- [x] Add a `std.result` reference page for the Incan-authored helper surface.
- [x] Add a release notes entry for issue #386 / RFC 070.
- [x] Bump the active development version by one dev increment.

## Design Decisions

- `inspect` and `inspect_err` are included in this RFC. They are branch-observation methods that call a one-argument observer through an implicit borrow of the branch payload and return the original `Result[T, E]` unchanged.
- The RFC uses `Callable[...]` as the callable vocabulary instead of introducing `Fn[...]`. Callable objects that satisfy the relevant fixed-arity callable hook contract are in scope wherever existing call checking already accepts them.
- Method names remain Rust-shaped: `map`, `map_err`, `and_then`, `or_else`, `inspect`, and `inspect_err`. This RFC does not add language-level aliases.
- The `std.result` helpers, including `inspect` and `inspect_err`, are authored in Incan. Borrowed observer payloads are handled by duckborrowing: source still spells `Callable[T, None]`, lowering can refine the generated callable boundary to a borrowed payload, and named function callbacks receive borrowed adapters when needed.
