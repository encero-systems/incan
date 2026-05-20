# Unit tests

Incan supports a testing experience via `incan test`.

This chapter shows how to write and run **unit tests** in a way that’s friendly to the compiler and the LSP.

!!! tip "Coming from Python?"
    If you have pytest muscle memory:

    - **Discovery**: tests can live in named test files (`test_*.incn`) or in inline `module tests:` blocks.
    - **Assertions**: the language `assert` statement is always available; import assertion helpers from `std.testing` when a function-call helper is clearer: `from std.testing import assert_eq, assert_ne, assert_true, assert_false, fail`.
    - **Markers/fixtures**: `@skip`, `@xfail`, `@fixture`, and `@parametrize` are provided by the `std.testing` module.

    References:

    - Language testing guide: [Testing in Incan](../../how-to/testing_stdlib.md)
    - CLI flags and output: [Tooling → Testing](../../../tooling/how-to/testing.md)
    - API signatures: [Standard library reference: `std.testing`](../../reference/stdlib/testing.md)

## The testing module

Additional assertion helpers are imported from the `std.testing` module:

```incan
from std.testing import assert_eq, assert_ne, assert_true, assert_false, fail
```

The statement form `assert expr[, msg]` is a language primitive and does not require importing `std.testing`. The helper functions mirror that behavior when a call form is useful.

For the full API reference, see: [Standard library reference: `std.testing`](../../reference/stdlib/testing.md).

## Your first unit test

Create a production file with an inline `module tests:` block, for example `src/math.incn`:

```incan
"""Math utilities."""

def add(a: int, b: int) -> int:
    return a + b

def is_even(value: int) -> bool:
    return value % 2 == 0

module tests:
    from std.testing import assert_eq, assert_false

    def test_addition() -> None:
        assert_eq(add(2, 3), 5)
        assert add(2, 3) == 5

    def test_private_helper() -> None:
        assert_false(is_even(3))
```

Run it:

```bash
incan test src/math.incn
```

Inline tests are useful for focused unit tests because the test block can call same-file helpers that are not exported with `pub`. The block is stripped from `incan build` and `incan run`, so test-only imports stay out of production output.

You can also write conventional test files under `tests/`, for example `tests/test_math.incn`:

```incan
from std.testing import assert_eq

def test_addition() -> None:
    assert_eq(2 + 3, 5)
```

Run those with:

```bash
incan test tests/
```

## Organizing tests

- Use `module tests:` for unit tests that belong next to production code.
- Put black-box or cross-module tests under a `tests/` directory.
- Conventional test files are discovered by name (e.g. `test_*.incn`).
- Test functions are discovered by name (e.g. `def test_*()`) inside the active test context.

Do not put `module tests:` inside a conventional `test_*.incn` or `*_test.incn` file. In named test files, write test functions at top level. In production source files, write test functions inside the single `module tests:` block.

The full language model is documented here: [Testing in Incan](../../how-to/testing_stdlib.md). CLI flags and output are documented here: [Tooling → Testing](../../../tooling/how-to/testing.md).

## Common patterns

### Boolean assertions

```incan
from std.testing import assert_true, assert_false

def test_flags() -> None:
    assert True
    assert 1 < 2, "ordering check failed"
    assert_true(1 < 2)
    assert_false(2 < 1)
```

### Explicit failure

```incan
from std.testing import fail

def test_not_reached() -> None:
    fail("this should not happen")
```

### Async fixtures

Fixtures can be asynchronous when setup or teardown needs `await`:

```incan
from std.async import sleep_ms
from std.testing import assert_eq, fixture

@fixture
async def resource() -> int:
    await sleep_ms(1)
    yield 42
    await sleep_ms(1)

async def test_uses_resource(resource: int) -> None:
    await sleep_ms(1)
    assert_eq(resource, 42)
```

Async fixtures still use `@fixture`; there is no async-only decorator. The runner awaits setup before the test runs and awaits teardown after `yield` when the worker remains alive. Parametrized tests expand first, then fixtures resolve for each expanded case under the usual fixture scopes.

## What to learn next

That's the end of the Incan Book (Basics)! You now know the core language. Here are some directions to explore next:

- [Your first project](../../../tooling/tutorials/your_first_project.md) — Set up a real project with `incan init`, Rust crate dependencies, and reproducible builds
- [Rust interop](../../how-to/rust_interop.md) — Use Rust crates from Incan code
- [Async programming](../../how-to/async_programming.md) — Write concurrent programs
- [Standard library reference: `std.testing`](../../reference/stdlib/testing.md) — Full testing API: fixtures, parametrize, skip, xfail
