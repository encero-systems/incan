# std.testing (reference)

This page specifies the standard-library testing API exposed by `std.testing`.

!!! info "Related pages"
    - If you are looking for how to run tests (`incan test`, discovery rules, CLI flags), see:
      [Tooling → Testing].
    - If you want a guided walkthrough, see: [The Incan Book → Unit tests].
    - If you want the language model for writing tests, including inline `module tests:`, see:
      [Language → How-to → Testing in Incan].

<!-- References -->
[Tooling → Testing]:../../../tooling/how-to/testing.md
[The Incan Book → Unit tests]:../../tutorials/book/13_unit_tests.md
[Language → How-to → Testing in Incan]:../../how-to/testing_stdlib.md

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

| Function                                                                                         | Default message                               | Fails when              | Returns |
| ------------------------------------------------------------------------------------------------ | --------------------------------------------- | ----------------------- | ------- |
| `testing.assert(condition: bool, msg: str = "assertion failed")`                                 | `"assertion failed"`                          | `condition` is `False`  | `None`  |
| `assert_true(condition: bool, msg: str = "assertion failed: expected true")`                     | `"assertion failed: expected true"`           | `condition` is `False`  | `None`  |
| `assert_false(condition: bool, msg: str = "assertion failed: expected false")`                   | `"assertion failed: expected false"`          | `condition` is `True`   | `None`  |
| `assert_eq[T](left: T, right: T, msg: str = "assertion failed: left != right")`                  | `"assertion failed: left != right"`           | `left != right`         | `None`  |
| `assert_ne[T](left: T, right: T, msg: str = "assertion failed: left == right")`                  | `"assertion failed: left == right"`           | `left == right`         | `None`  |
| `fail(msg: str)`                                                                                 | n/a                                           | Always                  | `None`  |
| `assert_is_some[T](option: Option[T], msg: str = "assertion failed: expected Some, got None")`   | `"assertion failed: expected Some, got None"` | `option` is `None`      | `T`     |
| `assert_is_none[T](option: Option[T], msg: str = "assertion failed: expected None, got Some")`   | `"assertion failed: expected None, got Some"` | `option` is `Some(...)` | `None`  |
| `assert_is_ok[T, E](result: Result[T, E], msg: str = "assertion failed: expected Ok, got Err")`  | `"assertion failed: expected Ok, got Err"`    | `result` is `Err(...)`  | `T`     |
| `assert_is_err[T, E](result: Result[T, E], msg: str = "assertion failed: expected Err, got Ok")` | `"assertion failed: expected Err, got Ok"`    | `result` is `Ok(...)`   | `E`     |

`assert_true(condition, msg?)` is an alias for `testing.assert(condition, msg?)`.

## Test markers (decorators)

The following decorators are recognized by the test runner only when they resolve to `std.testing` APIs. They are not magic global names.

Decorators work in both supported test contexts: top-level tests in conventional `test_*.incn` / `*_test.incn` files, and tests declared inside inline `module tests:` blocks in production files. For inline tests, import the decorators inside `module tests:` so production builds do not see test-only imports.

### `@test`

Marks a function as a test even when its name does not start with `test_`.

### `@skip(reason: str = "")`

Marks a test as skipped.

### `@skipif(condition: bool, reason: str = "")`

Skips a test when a collection-time condition is true. Conditions may use boolean literals/operators, string equality, `platform()`, and `feature("name")`; unsupported runtime expressions are collection errors.

### `@xfail(reason: str = "")`

Marks a test as expected to fail.

### `@xfailif(condition: bool, reason: str = "")`

Marks a test as expected to fail when a collection-time condition is true.

### `platform() -> str`

Returns the stable platform string used by `skipif` / `xfailif` collection probes.

### `feature(name: str) -> bool`

Returns true during collection when `incan test --feature <name>` was passed.

### `@slow`

Marks a test as slow (excluded by default unless enabled via tooling flags).

### `@mark(name: str)`

Attaches a user-defined marker for `incan test -m` selection.

### `@resource(name: str)`

Prevents overlapping scheduled execution with other tests that declare the same resource key.

### `@serial`

Runs the test alone against all other scheduled units.

### `@timeout(duration: str)`

Overrides the timeout for the generated test batch that contains this test. Durations use strings such as `"250ms"`, `"5s"`, or `"2m"`.

## `assert_raises`

`std.testing.assert_raises[E](block, msg = "")` asserts that a zero-argument callable raises or panics with the runtime error kind `E`.

The compiler also lowers `assert call() raises ErrorType[, msg]` to the same runtime check. Panic payloads match either the exact error kind name (for example `ValueError`) or the standard `Kind: message` prefix.

## Fixtures and parametrization

The `std.testing` module also defines the surface API for:

- `@fixture` (fixtures + dependency injection)
- `@parametrize` (parameterized tests)
- `param_case(...)` (per-case ids and marks for parameterized tests)

These are implemented by the `incan test` runner in both supported test contexts: top-level tests in conventional test files and tests inside inline `module tests:` blocks. For discovery rules, CLI flags, and current runner behavior, see: [Tooling → Testing].

Built-in fixtures are provided by parameter name:

- `tmp_path`
- `tmp_workdir`
- `env`

`env` is backed by `incan_stdlib::testing::TestEnv`, which restores environment changes when dropped.

Fixture scopes:

- `@fixture` / `@fixture(scope="function")`: created for each test that uses it.
- `@fixture(scope="module")`: cached once per source file inside a generated worker-batch harness.
- `@fixture(scope="session")`: cached once per worker batch. With `--jobs 1`, one session fixture instance is shared across collected files; with `--jobs N`, each worker gets its own instance.

Fixtures may use one top-level `yield` statement. Statements before `yield` are setup, the yielded value is injected, and statements after `yield` run as teardown at the fixture scope boundary. Teardown can reference setup locals and fixture parameters, runs after failing tests when the worker remains alive, and fails the run if teardown itself fails. Timeout-enforced worker termination can bypass teardown.
