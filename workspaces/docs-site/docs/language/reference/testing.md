# Testing (reference)

This page specifies the **standard library testing API** exposed by the `testing` module.

!!! info "Related pages"
    - If you’re looking for how to *run* tests (`incan test`, discovery rules, CLI flags), see: [Tooling → Testing].
    - If you want a guided walkthrough, see: [The Incan Book → Unit tests].
    - If you want integration details for how `std.testing` is compiled and wired, see:
      [Language → How-to → `std.testing` guide].

<!-- References -->
[Tooling → Testing]:../../tooling/how-to/testing.md
[The Incan Book → Unit tests]:../tutorials/book/13_unit_tests.md
[Language → How-to → `std.testing` guide]:../../how-to/testing_stdlib.md

## Importing the testing API

Incan test helpers are provided via the `std.testing` module:

```incan
from std.testing import assert, assert_eq, assert_ne, assert_true, assert_false, fail
```

## Assertion functions

All assertion helpers **fail the current test** when the assertion does not hold.

### `assert(condition: bool, msg: str = "") -> None`

Fails if `condition` is `False`.

### `assert_true(condition: bool, msg: str = "") -> None`

Alias for `assert(condition)`.

### `assert_false(condition: bool, msg: str = "") -> None`

Fails if `condition` is `True`.

### `assert_eq[T](left: T, right: T, msg: str = "") -> None`

Fails if `left != right`.

### `assert_ne[T](left: T, right: T, msg: str = "") -> None`

Fails if `left == right`.

### `fail(msg: str) -> None`

Unconditionally fails the current test with a message.

### `assert_is_some[T](option: Option[T], msg: str = "") -> T`

Fails if `option` is `None`; otherwise returns the inner value.

### `assert_is_none[T](option: Option[T], msg: str = "") -> None`

Fails if `option` is `Some(...)`.

### `assert_is_ok[T, E](result: Result[T, E], msg: str = "") -> T`

Fails if `result` is `Err(...)`; otherwise returns the `Ok(...)` value.

### `assert_is_err[T, E](result: Result[T, E], msg: str = "") -> E`

Fails if `result` is `Ok(...)`; otherwise returns the `Err(...)` value.

## Test markers (decorators)

The following decorators are **recognized by the test runner**:

### `@skip(reason: str = "")`

Marks a test as skipped.

### `@xfail(reason: str = "")`

Marks a test as *expected to fail*.

### `@slow`

Marks a test as slow (excluded by default unless enabled via tooling flags).

## Fixtures and parametrization

The `testing` module also defines the surface API for:

- `@fixture` (fixtures + dependency injection)
- `@parametrize` (parameterized tests)

These are implemented by the `incan test` runner. For current behavior and CLI support, see: [Tooling → Testing].
