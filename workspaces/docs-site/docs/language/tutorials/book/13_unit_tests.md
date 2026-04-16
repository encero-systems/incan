# Unit tests

Incan supports a testing experience via `incan test`.

This chapter shows how to write and run **unit tests** in a way that’s friendly to the compiler and the LSP.

!!! tip "Coming from Python?"
    If you have pytest muscle memory:

    - **Discovery**: test files are found by name (e.g. `test_*.incn`) and test functions by name (`def test_*()`).
    - **Assertions**: import assertion helpers from `std.testing` (they are normal functions):
      `from std.testing import assert_eq, assert_ne, assert_true, assert_false, fail`.
    - **Markers/fixtures**: `@skip`, `@xfail`, `@fixture`, and `@parametrize` are provided by the `std.testing` module.

    References:

    - CLI and discovery rules: [Tooling → Testing](../../../tooling/how-to/testing.md)
    - API signatures: [Standard library reference: `std.testing`](../../reference/stdlib/testing.md)

## The testing module

Test assertions and helpers are imported from the `std.testing` module:

```incan
from std.testing import assert, assert_eq, assert_ne, assert_true, assert_false, fail
```

These are normal functions (not language keywords), which makes them easy for tooling to understand.

For the full API reference, see:
[Standard library reference: `std.testing`](../../reference/stdlib/testing.md).

## Your first unit test

Create a test file, for example `tests/test_math.incn`:

```incan
"""Unit tests for math utilities."""

from std.testing import assert_eq

def add(a: int, b: int) -> int:
    return a + b

def test_addition() -> None:
    assert_eq(add(2, 3), 5)
```

Run it:

```bash
incan test tests/
```

## Organizing tests

- Put tests under a `tests/` directory.
- Test files are discovered by name (e.g. `test_*.incn`).
- Test functions are discovered by name (e.g. `def test_*()`).

If you use inline tests (`module tests:` inside a production file), keep `from std.testing import ...` **inside** the
`module tests:` block so test-only imports don’t leak into the production module scope.

The exact discovery and CLI flags are documented here: [Tooling → Testing](../../../tooling/how-to/testing.md).

## Common patterns

### Boolean assertions

```incan
from std.testing import assert, assert_true, assert_false

def test_flags() -> None:
    assert(True)
    assert_true(1 < 2)
    assert_false(2 < 1)
```

### Explicit failure

```incan
from std.testing import fail

def test_not_reached() -> None:
    fail("this should not happen")
```

## What to learn next

That's the end of the Incan Book (Basics)! You now know the core language. Here are some directions to explore next:

- [Your first project](../../../tooling/tutorials/your_first_project.md) — Set up a real project with
  `incan init`, Rust crate dependencies, and reproducible builds
- [Rust interop](../../how-to/rust_interop.md) — Use Rust crates from Incan code
- [Async programming](../../how-to/async_programming.md) — Write concurrent programs
- [Standard library reference: `std.testing`](../../reference/stdlib/testing.md) — Full testing API: fixtures, parametrize, skip, xfail
