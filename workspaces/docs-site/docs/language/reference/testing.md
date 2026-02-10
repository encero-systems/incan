# Testing (reference)

This page specifies the **standard library testing API** exposed by the `testing` module.

If you’re looking for how to *run* tests (`incan test`, discovery rules, CLI flags), see: [Tooling → Testing](../../tooling/how-to/testing.md).

If you want a guided walkthrough, see: [The Incan Book → Unit tests](../tutorials/book/13_unit_tests.md).

## Importing the testing API

Incan test helpers are provided via the `std.testing` module:

```incan
from std.testing import assert, assert_eq, assert_ne, assert_true, assert_false, fail
```

## Assertion functions

All assertion helpers **fail the current test** when the assertion does not hold.

### `assert(condition: bool) -> None`

Fails if `condition` is `False`.

### `assert_true(condition: bool) -> None`

Alias for `assert(condition)`.

### `assert_false(condition: bool) -> None`

Fails if `condition` is `True`.

### `assert_eq[T](left: T, right: T) -> None`

Fails if `left != right`.

### `assert_ne[T](left: T, right: T) -> None`

Fails if `left == right`.

### `fail(msg: str) -> None`

Unconditionally fails the current test with a message.

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

These are implemented by the `incan test` runner. For current behavior and CLI support, see: [Tooling → Testing](../../tooling/how-to/testing.md).
