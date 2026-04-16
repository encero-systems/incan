# RFC 018: language primitives for testing

- **Status:** Planned
- **Created:** 2026-01-14
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 019 (test runner and CLI), RFC 001 (language portions; superseded), RFC 002 (language portions; superseded)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/76
- **RFC PR:** —
- **Written against:** v0.1
- **Shipped in:** —

## Summary

Define the language-level testing primitives for Incan:

- the `assert` keyword (always-on), including message propagation and pattern-binding forms
- `assert ... raises ErrorType` (sync only)
- a reserved `module tests:` block for inline test-only code
- `testing`-gated resolution rules for test-only decorators/helpers (no magic global names)
- build/check stripping semantics for inline tests

Testing decorators/markers are gated behind the `testing` standard library module (no magic global names).

This RFC is jointly normative with **RFC 019**, which defines runner and CLI behavior (discovery, fixtures, parametrization, markers, parallelism, timeouts, reporting).

## Motivation

Testing spans both **language** concerns (syntax, scoping, desugaring) and **runner** concerns (discovery, fixtures, selection, reporting). Keeping those concerns in separate RFCs lets the compiler and the test runner evolve independently while still sharing precise invariants.

This RFC defines only the language primitives; RFC 019 specifies the runner and CLI semantics that consume them.

## Goals

- Define the language-owned testing primitives independently from runner and CLI behavior.
- Standardize `assert`, `assert ... raises ErrorType`, and inline `module tests:` blocks.
- Require `testing`-gated name resolution for test-only decorators and helpers.
- Preserve a clean split between compiler semantics and test-runner semantics across RFC 018 and RFC 019.

## Non-Goals

- Defining test discovery, fixtures, parametrization, markers, parallelism, or reporting semantics in this RFC.
- Introducing magic global test names outside the `testing` module gate.
- Changing production-code semantics beyond the explicitly defined testing primitives in this document.

## Guide-level explanation (how users think about it)

### How to read this RFC

This RFC covers **language-level** testing primitives only:

- `assert` (including messages and pattern binding)
- `assert ... raises ErrorType`
- `module tests:` inline test blocks
- `testing`-gated resolution rules for test-only constructs

Runner/CLI behavior (discovery, fixtures, parametrization, markers, parallelism, timeouts, reporting) is defined in
**RFC 019**.

If you are implementing this RFC, start with the conformance checklist near the end and the reference-level rules above, then implement in dependency order.

### The `testing` module

Testing utilities are normal functions/decorators imported from the `testing` module:

```incan
from testing import assert_eq, assert_true, assert_false, fail
```

This RFC only specifies how these names are *resolved* (gated behind `testing`) and how `assert` maps to the
`testing.assert_*` surface. Execution semantics for fixtures, parametrization, markers, and CLI options are defined in
RFC 019.

### Assertions: `assert ...` (keyword)

This RFC introduces an `assert` keyword that can be used **anywhere** (tests or production code) without importing anything:

```incan
assert 1 + 2 == 3
assert 2 < 3
assert user is Some(_), "user must be present"
```

It is syntax sugar for a **language-level assertion primitive** with the same user-facing behavior as the `testing` module’s assertion functions. See the reference-level table below for the full, exhaustive mapping.

Design note (important semantic commitment):

- `assert` must be valid in production code **without** pulling in the test runner or requiring `incan test`.
- Implementations may lower `assert` to a compiler intrinsic / core runtime primitive. The `testing.assert_*` helpers are
the test-oriented, explicit API surface and must remain consistent with `assert` behavior (messages, formatting, etc.).
- This RFC defines `assert` as **always-on** runtime checking (like Python). A compile-out variant (e.g. `debug_assert`)
may be introduced in a future RFC, but is out of scope here.

Rationale (brief):

- `assert` is used both in tests and in production code for invariants (“this should never happen”).
- Always-on semantics avoid “works in tests/debug, breaks in release” surprises.
- Mitigation: avoid asserts in hot paths and prefer explicit `Result`/`Option`-based error handling for recoverable
conditions. A future `debug_assert` can address performance-sensitive checks.

The `testing` module remains available for explicit imports, and richer APIs.

Note: this RFC specifies **assertion messages** (e.g. `assert x > 0, "x must be positive"`) and requires the underlying
`testing.assert_*` helpers to accept an optional `msg`. This is not fully supported in current Incan; it is a required
part of implementing this RFC.

#### Common `assert` patterns (guide-level, Python-inspired)

`assert` accepts any expression that type-checks to `bool`. In practice, users will write assertions across a handful of
common shapes.

Equality / inequality (special-cased to `assert_eq` / `assert_ne`):

```incan
assert a == b
assert actual != unexpected
assert user.name == "alice"
assert len(items) == 3

# Optional message (shown on failure)
assert user.name == "alice", "expected alice user"
```

#### Expected failure output (illustrative)

Exact formatting is implementation-defined, but failures should be understandable.

At minimum, failed `assert` statements should be reported as an `AssertionError` (Python-inspired), with the optional message rendered as `AssertionError: <msg>`.

Minimum examples:

```text
# assert x > 0, "x must be positive"
FAILED: AssertionError: x must be positive
  assertion failed: x > 0
  x = -1
```

```text
# assert x > 0
FAILED: AssertionError
  assertion failed: x > 0
  x = -1
```

```text
# assert a == b, "values must match"
FAILED: AssertionError: values must match
  assertion failed: left != right
  left:  2
  right: 3
```

Ordering comparisons:

```incan
assert x > 0
assert x >= 0
assert start <= end

# Incan does not support Python-style chained comparisons like `0 < x < 10` (at least not as part of this RFC).
# Use `and` explicitly:
assert 0 < x and x < 10
```

Boolean logic:

```incan
assert True
assert not True
assert a and b
assert a or b
assert (a and b) or c
```

Option / Result checks (recommended to be explicit; `Option`/`Result` are not implicitly truthy):

```incan
assert user.is_some()
assert user.is_none()

assert result.is_ok()
assert result.is_err()
```

Pattern matching with `is` (note: **`is` is pattern matching**, not identity):

```incan
assert user is Some(_)
assert result is Ok(_)
assert result is Err(_)
```

Binding notes (important semantic commitment):

- `assert x is Some(v)` may introduce a binding (here: `v`) for the **remainder of the current block scope**, as if the
compiler had emitted `let v = ...` at that point.
- In RFC 018, only the following binding patterns are supported in `assert`:
    - `Some(name)` and `Ok(name)` / `Err(name)` where `name` is a single identifier, OR
    - the wildcard `_` (no binding).
- Nested patterns, multiple bindings, and guards are out of scope for this RFC (they may be added later if/when the
general pattern-matching system is specified).

“Contains” / membership style checks:

```incan
assert name != ""

# If the type exposes an API (e.g. `contains(...)`) this works naturally:
assert tags.contains("release")
```

Identity checks (Python’s `is`) are intentionally **not** part of `assert` in this RFC, because `is` already has a different meaning (pattern matching). If/when Incan adds a reference-identity operation, it should be spelled explicitly (e.g. `ref_eq(a, b)`), not overloaded onto `is`.

### Inline test-only module blocks

Inline tests live next to production code, but inside a **test-only module block**:

```incan
def add(a: int, b: int) -> int:
    return a + b


module tests:
    from testing import assert_eq, test

    def test_addition() -> None:
        assert add(2, 3) == 5

    @test  # explicitly marked as a test (name doesn't need to start with test_)
    def explicit_test() -> None:
        assert add(2, 3) == 5
```

This keeps helpers/fixtures/test imports scoped to the test module and allows the compiler to strip all test-only code from `incan build` and `incan run`.

Rule of thumb:

- In inline tests (`module tests:` inside a production file), put `from testing import ...` **inside the `module tests:`
block** so the production module namespace stays clean.

Test file discovery and runner behavior are defined in **RFC 019**.

## Reference-level explanation (precise rules)

### Core principle: testing is gated behind `testing`

Test tooling must only recognize testing constructs when they resolve to the `testing` module. The compiler must resolve imports/aliases consistently so runner semantics (defined in RFC 019) can rely on these identities.

Runner-recognized constructs (see RFC 019) include:

- `@test` = `testing.test`
- `@fixture` = `testing.fixture`
- `@parametrize` = `testing.parametrize`
- `@skip`, `@xfail`, `@slow` = corresponding `testing.*` markers
- `@serial` / `@resource(...)` = corresponding `testing.*` scheduling decorators

This avoids “magic names” (e.g. a random user-defined `@fixture` decorator should not be treated as a test fixture).

#### Resolution rule (minimal)

For a decorator `@X` to be treated as `testing.<name>`, one of the following must hold in the file:

1. `X` is `testing::<name>` / `testing.<name>` (fully-qualified reference), OR
2. `X` is an imported alias of `testing.<name>` (e.g. `from testing import fixture as X`), OR
3. `X` is imported from the `testing` module without alias (e.g. `from testing import fixture`)

(Exact import-resolution machinery is an implementation detail, but the behavior must match these semantics.)

Rationale:

- Unlike ordinary modules (e.g. `web`), `testing` is a gateway for discovery and special semantics. Resolution must be
explicit and auditable, so only symbols that *resolve to* `testing` are treated as test constructs.

Module aliasing:

- A decorator expression of the form `@M.<name>` is treated as `testing.<name>` only if `M` resolves to the `testing`
module (e.g. `import testing as t; @t.fixture`).

Re-exports:

- If a decorator name resolves to a symbol re-exported from another module, it is treated as `testing.<name>` only if the
resolver can prove the symbol originates from `testing`. Otherwise it is treated as a normal decorator.

Star imports are disallowed for the `testing` module:

- `from testing import *` MUST be a compile-time error in any context.
- Rationale: explicit imports keep the testing gate analyzable and avoid accidental collisions with user-defined names.

### The `assert` keyword (reference semantics)

`assert` is a language-level statement that is valid in any file (not only in test contexts).

Form:

- `assert <expr>` where `<expr>` type-checks as `bool`
- `assert <expr>, <msg>` where `<msg>` type-checks as `str` (optional failure message)

The optional message is passed through to the underlying `testing.assert_*` helper and should be displayed as part of the assertion failure output.

Message presence:

- If no message is provided, output should not render a message line.
- An empty string is treated as “no message” for formatting purposes.

Minimum diagnostics guarantee:

- On failure, assertion output must include the optional message (if provided) and enough detail to diagnose the failing
condition.
- For equality/inequality assertions (`assert a == b` / `assert a != b`), the minimum guarantee is that the output
includes both “left” and “right” values (via `Debug`-style formatting) plus the optional message.

Runtime error model:

- A failed `assert` raises a built-in runtime error type named `AssertionError`.
- “Runtime error” refers to a panic-style failure (not a `Result`-returning error). It aborts the current test case.
- `ErrorType` in `assert ... raises ErrorType` must denote a runtime error type.

#### Runtime error types (normative; scope for this RFC)

This RFC uses the term **runtime error** to mean a panic-style failure that aborts execution (in contrast to
`Result`-returning errors that are explicitly handled with `?` and pattern matching).

Rules:

- This RFC requires at minimum the built-in runtime error type `AssertionError`.
- This RFC does **not** define a general, user-extensible runtime error hierarchy.
    - User-defined errors should use `Result`/`Option` (Incan’s primary error-handling model).
- Subtyping among runtime error types is **optional**:
    - If the implementation supports runtime error subtyping, `assert ... raises BaseType` must match subtypes.
    - If not, implementations MUST - at minimum - match the exact runtime error type named by `ErrorType` (as already
      specified in the raises rules below).

#### Exhaustive mapping to `testing.assert_*` helpers (required behavior)

| `testing.*` helper | `assert ...` surface form               | Lowers to                                        |
| ------------------ | --------------------------------------- | ------------------------------------------------ |
| `assert`           | `assert <bool-expr>[, msg]`             | `testing.assert(<bool-expr>, msg?)`              |
| `assert_true`      | `assert <bool-expr>[, msg]`             | `testing.assert(<bool-expr>, msg?)`              |
| `assert_false`     | `assert not <bool-expr>[, msg]`         | `testing.assert(not <bool-expr>, msg?)`          |
| `assert_eq`        | `assert a == b[, msg]`                  | `testing.assert_eq(a, b, msg?)`                  |
| `assert_ne`        | `assert a != b[, msg]`                  | `testing.assert_ne(a, b, msg?)`                  |
| `assert_is_some`   | `assert opt is Some(v)[, msg]`          | `let v = testing.assert_is_some(opt, msg?)`      |
| `assert_is_none`   | `assert opt is None[, msg]`             | `testing.assert_is_none(opt, msg?)`              |
| `assert_is_ok`     | `assert res is Ok(v)[, msg]`            | `let v = testing.assert_is_ok(res, msg?)`        |
| `assert_is_err`    | `assert res is Err(e)[, msg]`           | `let e = testing.assert_is_err(res, msg?)`       |
| `assert_raises`    | `assert call() raises ErrorType[, msg]` | `testing.assert_raises[ErrorType](..., msg?)`    |

Full signatures (required `testing` API surface for this RFC):

```incan
assert(condition: bool, msg: str = "") -> None
assert_true(condition: bool, msg: str = "") -> None
assert_false(condition: bool, msg: str = "") -> None
assert_eq[T: Debug](left: T, right: T, msg: str = "") -> None
assert_ne[T: Debug](left: T, right: T, msg: str = "") -> None
assert_is_some[T](option: Option[T], msg: str = "") -> T
assert_is_none[T](option: Option[T], msg: str = "") -> None
assert_is_ok[T, E](result: Result[T, E], msg: str = "") -> T
assert_is_err[T, E](result: Result[T, E], msg: str = "") -> E
assert_raises[E](block: () -> None, msg: str = "") -> E
```

Trait bounds for formatting:

- `assert_eq` and `assert_ne` require `T: Debug` so that failure output can render both values.
- If `T` does not implement `Debug`, the compiler must emit a diagnostic suggesting `@derive(Debug)` on the type.
- Other assertion helpers do not require `Debug` on their type parameters (they only report success/failure, not values).

Desugaring rule used by the compiler:

Let the optional message be `msg` when present (i.e. `assert <expr>, msg`).

- If `<expr>` is syntactically `a == b`, lower to `testing.assert_eq(a, b, msg?)`
- If `<expr>` is syntactically `a != b`, lower to `testing.assert_ne(a, b, msg?)`
- If the assert statement is of the form `assert opt is Some(v)`, lower to `let v = testing.assert_is_some(opt, msg?)`
- If the assert statement is of the form `assert opt is None`, lower to `testing.assert_is_none(opt, msg?)`
- If the assert statement is of the form `assert res is Ok(v)`, lower to `let v = testing.assert_is_ok(res, msg?)`
- If the assert statement is of the form `assert res is Err(e)`, lower to `let e = testing.assert_is_err(res, msg?)`
- If the assert statement is of the form `assert call() raises ErrorType`, lower to
  `testing.assert_raises[ErrorType](lambda: call(), msg?)`.
- Otherwise, lower to `testing.assert(<expr>, msg?)`

Note: the “lowers to” wording describes the required behavior and message propagation. Implementations may choose to lower
`assert` to a compiler intrinsic and have `testing.assert_*` call into that intrinsic, as long as the user-visible
semantics match this mapping.

The `testing` module is **not** required at runtime for `assert`; the mapping is semantic, and `testing.assert_*` must mirror the intrinsic behavior.

The `assert_true` / `assert_false` helpers are aliases/conveniences in the `testing` API; the compiler does not need to emit them directly.

On failure, assertions must produce the same failure semantics and (as much as possible) the same message formatting as the underlying `testing` assertion functions.

#### Pattern-binding scope and allowed patterns (reference rules)

`assert` supports a limited form of pattern binding via `is` (leveraging Incan’s existing pattern-matching semantics).

Allowed `Option` patterns:

- `assert opt is Some(_)[, msg]`
- `assert opt is Some(<ident>)[, msg]` (binds `<ident>`)
- `assert opt is None[, msg]`

Allowed `Result` patterns:

- `assert res is Ok(_)[, msg]`
- `assert res is Ok(<ident>)[, msg]` (binds `<ident>`)
- `assert res is Err(_)[, msg]`
- `assert res is Err(<ident>)[, msg]` (binds `<ident>`)

Restrictions:

- No nested patterns (e.g. `Some(Ok(x))`) and no multiple bindings in a single `assert`.
- The bound name is introduced in the *current* scope exactly as if the compiler had emitted `let <ident> = ...` at the
assertion site; it is in scope for subsequent statements in the same block.
- The bound name has the inner type of the matched value (e.g. `T` for `Option[T]`, `T`/`E` for `Result[T, E]`), and the
assertion does not otherwise narrow the type of the tested expression.
- Shadowing: if the bound identifier already exists in the current lexical block, the assertion is a compile-time error.
Users should pick a new name or bind in an inner block to avoid ambiguity.

Guidance (non-normative): avoid using `assert` as control flow in production code. Prefer explicit pattern matching or
`assert_is_*` helpers when unwrapping `Option`/`Result` values.

#### Raises semantics (reference rules)

Syntax: `assert <call-expr> raises <ErrorType>[, msg]` where `<call-expr>` is a call expression.

This is a convenience for asserting that a call fails by *raising* a runtime error.

- “Raises” refers to a runtime error/panic-style failure (the same category of failure used for failed assertions).
- It does **not** refer to `Result`-returning APIs; for results, use `assert res is Err(e)` / `assert_is_err`.
- Block-style “raises” assertions are out of scope; use `testing.assert_raises` for multi-statement checks.
- Matching: `ErrorType` matches that exact type or any of its subtypes.
If an implementation lacks subtype information, it MUST at minimum match the exact type.
- Async “raises” is out of scope for this RFC.

### Inline test module context (reference rules)

`module tests:` introduces a test-only scope inside a production source file. The compiler must treat this block as
strip-able in non-test compilation modes.

Rules:

- A file may contain at most one `module tests:` block. Additional `module tests:` blocks are a compile error.
- A file that is a test file context (as defined in RFC 019) must not also contain `module tests:`.
- Names declared inside `module tests:` do not leak into the surrounding module scope.

Test file discovery and how tests/fixtures are collected are defined in RFC 019.

### Build and check behavior

| Command         | Test contexts (`module tests:`)    |
| --------------- | ---------------------------------- |
| `incan build`   | stripped (not emitted)             |
| `incan run`     | stripped (not included)            |
| `incan test`    | included and executed              |
| `incan --check` | type-checked but not emitted       |

Test files are only relevant to `incan test` (they are not part of production builds).

Note: `incan --check` type-checks inline `module tests:` blocks in source files, but does not include `tests/` unless explicitly passed as a path argument.

## Design details

### Inline test module scoping

The inline test module:

- may access names from the surrounding file (like Rust’s `use super::*` unit-test pattern)
- introduces a scope boundary so test-only helpers/imports do not pollute the production namespace

Visibility rules (normative):

- Names declared in the enclosing module (including private names not marked `pub`) are visible inside `module tests:`.
- This is **lexical visibility**, not an implicit import: the test block can reference any name that is in scope at
the file level, as if the test block were nested code in the same file.
- Names declared inside `module tests:` (functions, imports, bindings) are **not** visible outside the test block.
- The test block does not introduce a separate module namespace for the purpose of `pub` visibility; it is purely a
scoped block that can be stripped.

This RFC does **not** define a general-purpose module system beyond existing file/module semantics; `module tests:` inside a file is specifically a scoped block that can be stripped in non-test compilation modes.

## Compatibility / migration

- Existing code that uses `testing.assert_*` continues to work; `assert` is additive syntax sugar.
- Adding a `module tests:` block enables inline tests without changing production code layout.
- Runner discovery and CLI compatibility are specified in RFC 019.

## Alternatives considered

- **Top-level `@test` next to production functions**: rejected; it pollutes the production namespace and makes it hard
to keep test-only imports/helpers contained.
- **Magic language keywords for tests/fixtures**: rejected; harms tooling and contradicts the “stdlib-gated” principle.
- **Compile-time-only assertions**: rejected; `assert` is intended for always-on runtime invariants.

Out of scope (for now):

- `debug_assert` or build-mode controlled assertion stripping
- richer pattern matching in `assert` (nested patterns, guards)

## Appendix: testing surface inventory (informative)

This appendix is a contributor-oriented inventory of the testing surface **after this RFC is implemented**, with an informative snapshot of what exists **today** (at time of writing). It is **not normative**; the spec sections above are authoritative.

Legend:

- **Today**: implementation status in the current repository at RFC creation time
    - **Yes**: implemented
    - **Partial**: some pieces exist, but not the full RFC behavior
    - **No**: not implemented
- **After RFC 018**: whether this RFC introduces it (**New**), modifies semantics (**Changed**), or leaves it (**Unchanged**)

> Note: this table should be used as a checkmark toward implementation completeness when this RFC is implemented.

### Language + assertion API surface

| Item                                             | Today      | After RFC 018 | Notes                                           | Implemented |
| ------------------------------------------------ | ---------- | ------------- | ----------------------------------------------- | ----------- |
| `assert <expr>` keyword                          | No         | New           | Lowers to `testing.assert_*` helpers            |             |
| `assert <expr>, <msg>`                           | No         | New           | Python-style message; passed through to helpers |             |
| `module tests:` inline tests                     | No         | New           | Reserved scope; stripped outside `incan test`   |             |
| `testing.assert*(..., msg="")`                   | Partial    | Changed       | RFC requires optional `msg` on core asserts     |             |
| `testing.assert_is_*` helpers                    | Partial    | Changed       | RFC pins behavior + msg propagation             |             |
| `testing.assert_raises` (+ `assert ... raises`)  | Partial    | Changed       | RFC pins desugaring + optional msg              |             |
| `testing.fail(msg)`                              | Yes        | Unchanged     | Explicit failure                                |             |

## Layers affected

- **Lexer** — `assert` must be treated as a soft keyword activated by `std.testing` imports, not a hard keyword. The lexer emits `Ident("assert")`; the parser promotes it via the keyword registry.
- **Parser** — must parse the `assert <expr>` form, the `assert <expr>, <msg>` form, and the `assert <expr> raises <Type>` form as distinct AST nodes; must parse and validate `module tests:` as a reserved inline block producing a `TestModule` AST node; must enforce that `module tests:` appears at most once per file at module scope.
- **Typechecker** — must gate resolution of test-only decorators and helpers behind `testing`-activated import context; must validate that names inside `module tests:` are not visible outside and that the block has read access to private module members.
- **Lowering** — must lower `assert <expr>` to the appropriate `testing.assert_*` call based on expression shape (equality, inequality, option/result, pattern binding); must lower `assert <expr> raises <Type>` to `testing.assert_raises`; must strip `module tests:` bodies from non-test compilation modes.
- **Stdlib (`std.testing`)** — `assert_eq`, `assert_ne`, `assert_is_some`, `assert_is_none`, `assert_raises`, `assert`, and their message-accepting overloads must conform to the desugaring rules specified in this RFC.
- **CLI** — `incan build` and `incan run` must strip `module tests:` bodies; `incan --check` must typecheck them; `incan test` must include them in the compilation unit.

## Design Decisions

- `assert` is an always-on language primitive and is not compiled out in release builds by this RFC.
- Testing-specific decorators and helpers are gated behind the `testing` module rather than treated as ambient global names.
- `module tests:` is the reserved inline scope for test-only code in production modules.
- Runner and CLI semantics remain split into RFC 019 rather than being folded into this language-level RFC.

## References

- RFC 019: Test Runner, CLI, and Ecosystem
- Python `assert` statement: `https://docs.python.org/3/reference/simple_stmts.html#the-assert-statement`

<!-- The "Design decisions" section (if present) was renamed from "Unresolved questions" once all open questions were resolved. If new unresolved questions arise during implementation, add an "Unresolved questions" section and move resolved items to "Design decisions" after resolution. -->
