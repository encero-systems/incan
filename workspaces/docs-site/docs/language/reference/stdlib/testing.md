# std.testing (reference)

This page specifies the standard-library testing API exposed by `std.testing`.

!!! info "Related pages"
    - If you are looking for how to run tests (`incan test`, discovery rules, CLI flags), see:
      [Tooling → Testing].
    - If you want a guided walkthrough, see: [The Incan Book → Unit tests].
    - If you want integration details for how `std.testing` is compiled and wired, see:
      [Language → How-to → `std.testing` guide].

<!-- References -->
[Tooling → Testing]:../../../tooling/how-to/testing.md
[The Incan Book → Unit tests]:../../tutorials/book/13_unit_tests.md
[Language → How-to → `std.testing` guide]:../../how-to/testing_stdlib.md

## Importing the testing API

The language `assert` statement is always available and does not require importing `std.testing`:

```incan
assert user.active
assert count == 3, "unexpected row count"
```

`std.testing` provides the helper functions and test decorators:

```incan
import std.testing as testing
from std.testing import assert_eq, assert_ne, assert_true, assert_false, fail
```

## Assertion functions

All assertion helpers fail the current test when the assertion does not hold. The helper named `assert` is available as `testing.assert(condition, msg?)`; prefer the language statement for ordinary boolean assertions.

### `testing.assert(condition: bool, msg: str = "assertion failed") -> None`

Fails if `condition` is `False`.

### `assert_true(condition: bool, msg: str = "assertion failed: expected true") -> None`

Alias for `testing.assert(condition)`.

### `assert_false(condition: bool, msg: str = "assertion failed: expected false") -> None`

Fails if `condition` is `True`.

### `assert_eq[T](left: T, right: T, msg: str = "assertion failed: left != right") -> None`

Fails if `left != right`.

### `assert_ne[T](left: T, right: T, msg: str = "assertion failed: left == right") -> None`

Fails if `left == right`.

### `fail(msg: str) -> None`

Unconditionally fails the current test with a message.

### `assert_is_some[T](option: Option[T], msg: str = "assertion failed: expected Some, got None") -> T`

Fails if `option` is `None`; otherwise returns the inner value.

### `assert_is_none[T](option: Option[T], msg: str = "assertion failed: expected None, got Some") -> None`

Fails if `option` is `Some(...)`.

### `assert_is_ok[T, E](result: Result[T, E], msg: str = "assertion failed: expected Ok, got Err") -> T`

Fails if `result` is `Err(...)`; otherwise returns the `Ok(...)` value.

### `assert_is_err[T, E](result: Result[T, E], msg: str = "assertion failed: expected Err, got Ok") -> E`

Fails if `result` is `Ok(...)`; otherwise returns the `Err(...)` value.

## Test markers (decorators)

The following decorators are recognized by the test runner only when they resolve to `std.testing` APIs. They are not magic global names.

### `@skip(reason: str = "")`

Marks a test as skipped.

### `@xfail(reason: str = "")`

Marks a test as expected to fail.

### `@slow`

Marks a test as slow (excluded by default unless enabled via tooling flags).

## `assert_raises`

`std.testing.assert_raises[E](block, msg = "")` asserts that a zero-argument callable raises or panics with
the runtime error kind `E`.

The compiler also lowers `assert call() raises ErrorType[, msg]` to the same runtime check. Panic payloads match either
the exact error kind name (for example `ValueError`) or the standard `Kind: message` prefix.

## Fixtures and parametrization

The `testing` module also defines the surface API for:

- `@fixture` (fixtures + dependency injection)
- `@parametrize` (parameterized tests)

These are implemented by the `incan test` runner. RFC 018 owns the language primitive and stdlib helper contracts; runner discovery and CLI behavior belong to RFC 019. For current behavior and CLI support, see:
[Tooling → Testing].
