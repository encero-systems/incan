# Testing in Incan

This page covers the language model for writing tests in Incan: where tests live, how inline `module tests:` blocks work, and what `std.testing` provides.

> If you want CLI usage (`incan test`, discovery, flags), see [Tooling: Testing](../../tooling/how-to/testing.md).
> If you want the API reference only, see [Standard library reference: `std.testing`](../reference/stdlib/testing.md).

## Where tests live

Incan supports two test contexts.

| Context                | Use it for                                                                                   | Discovery rule                                           |
| ---------------------- | -------------------------------------------------------------------------------------------- | -------------------------------------------------------- |
| Conventional test file | Black-box tests, integration tests, tests spanning several modules                           | A file named `test_*.incn` or `*_test.incn`              |
| Inline test module     | Unit tests that belong next to production code, including tests of same-file private helpers | A production `.incn` file with one `module tests:` block |

Conventional test files collect top-level `def test_*()` functions and top-level fixtures. Inline test modules collect only tests and fixtures declared inside `module tests:`. A production-scope function named `test_*` is still just a production function unless it is inside the inline test block.

## Inline `module tests:`

An inline test module is a test-only block inside a production source file:

```incan
def slugify(title: str) -> str:
    return title.strip().lower().replace(" ", "-")

def has_private_prefix(slug: str) -> bool:
    return slug.startswith("_")

module tests:
    from std.testing import assert_eq, assert_false

    def test_slugify_trims_and_replaces_spaces() -> None:
        assert_eq(slugify("  Hello Incan  "), "hello-incan")

    def test_can_call_same_file_private_helper() -> None:
        assert_false(has_private_prefix("public-page"))
```

This is a language feature, not just a test-runner naming convention.

!!! tip "Coming from Python?"
    Python usually puts unit tests in separate files and relies on naming conventions plus module imports to reach the code under test. Inline `module tests:` is closer to Rust's `#[cfg(test)] mod tests`: the tests live in the same source file as the implementation, can call same-file private helpers, and are removed from normal build/run output. Because Incan is compiled, the compiler can omit inline tests from final package builds as part of the language contract; Python packaging can exclude test files, but Python has no equivalent compiler-stripped inline test block.

- Code inside `module tests:` can read names from the enclosing file, including names that are not `pub`.
- Names declared inside `module tests:` do not become production module members.
- Imports inside `module tests:` are test-only and do not affect `incan build` or `incan run`.
- A file may contain at most one `module tests:` block.
- A conventional `test_*.incn` or `*_test.incn` file must not contain `module tests:`; put top-level tests there instead.

Keep test-only imports inside the block:

```incan
def double(value: int) -> int:
    return value * 2

module tests:
    from std.testing import assert_eq, parametrize

    @parametrize("value, expected", [
        (1, 2),
        (4, 8),
    ])
    def test_double(value: int, expected: int) -> None:
        assert_eq(double(value), expected)
```

## Assertion helpers

The language `assert` statement is always available. You do not import `std.testing` to enable it:

```incan
def test_addition() -> None:
    assert 1 + 1 == 2
    assert 3 > 2, "math ordering changed"
```

`std.testing` provides function-call helpers that mirror the assertion behavior when a helper call is clearer or when you need an unwrap-style return value:

```incan
import std.testing as testing
from std.testing import assert_eq, assert_ne, assert_true, assert_false, fail
from std.testing import assert_is_some, assert_is_none, assert_is_ok, assert_is_err
```

| Function                          | Fails when                     | Returns |
| --------------------------------- | ------------------------------ | ------- |
| `testing.assert(condition, msg?)` | `condition` is false           | —       |
| `assert_true(condition, msg?)`    | `condition` is false           | —       |
| `assert_false(condition, msg?)`   | `condition` is true            | —       |
| `assert_eq(left, right, msg?)`    | `left != right`                | —       |
| `assert_ne(left, right, msg?)`    | `left == right`                | —       |
| `assert_is_some(option, msg?)`    | `option` is `None`             | `T`     |
| `assert_is_none(option, msg?)`    | `option` is `Some(...)`        | —       |
| `assert_is_ok(result, msg?)`      | `result` is `Err(...)`         | `T`     |
| `assert_is_err(result, msg?)`     | `result` is `Ok(...)`          | `E`     |
| `fail(msg)`                       | Always (unconditional failure) | —       |

All `msg` parameters are optional. When omitted, a sensible default message is used.

## `assert_raises`

Use `std.testing.assert_raises[E](block, msg = "")` when a test needs to assert that a zero-argument callable raises or panics with runtime error kind `E`.

- `assert call() raises ErrorType[, msg]` is the statement form for single-call checks.
- `assert_raises[E](helper, msg?)` is the helper form for named zero-argument functions or closures.
- Panic payloads match either the exact error kind name, such as `ValueError`, or the standard `Kind: message` prefix.

## Assert statement syntax

Incan supports `assert` as a language statement. It is part of the language, not a marker or decorator, and it does not require `import std.testing`.

| Statement               | Desugars to                           |
| ----------------------- | ------------------------------------- |
| `assert cond`           | `std.testing.assert(cond)`            |
| `assert a == b`         | `std.testing.assert_eq(a, b)`         |
| `assert a != b`         | `std.testing.assert_ne(a, b)`         |
| `assert opt is Some(v)` | `v = std.testing.assert_is_some(opt)` |
| `assert opt is None`    | `std.testing.assert_is_none(opt)`     |
| `assert res is Ok(v)`   | `v = std.testing.assert_is_ok(res)`   |
| `assert res is Err(e)`  | `e = std.testing.assert_is_err(res)`  |

The mapping is semantic. The compiler may implement the language statement as an intrinsic, but the `std.testing.assert_*` helpers must keep matching behavior and message propagation.

## Markers and decorators

Test markers control how `incan test` discovers and runs tests:

| Decorator                           | Effect                                                               |
| ----------------------------------- | -------------------------------------------------------------------- |
| `@test`                             | Marks a non-`test_*` function as a test.                             |
| `@skip(reason?)`                    | Skips the test unconditionally.                                      |
| `@skipif(condition, reason?)`        | Skips the test when a collection-time condition is true.             |
| `@xfail(reason?)`                   | Marks the test as expected to fail (XPASS if it passes).             |
| `@xfailif(condition, reason?)`       | Marks the test expected-fail when a collection-time condition is true. |
| `@slow`                             | Excludes the test by default; include with `incan test --slow`.      |
| `@mark(name)`                       | Adds a custom marker for `incan test -m` selection.                  |
| `@timeout(duration)`                | Overrides the timeout for the generated test batch.                  |
| `@fixture`                          | Declares a test fixture (see below).                                 |
| `@parametrize(argnames, argvalues)` | Runs the test once per parameter set.                                |
| `param_case(..., id?, marks?)`      | Gives one parameter set an explicit id and/or marks.                 |
| `@resource(name)` / `@serial`       | Applies runner scheduling constraints for shared or exclusive tests. |
| `platform()` / `feature(name)`       | Collection-time probes for `skipif` / `xfailif`.                    |

Unlike the language `assert` statement, markers are `std.testing` APIs. Import the marker decorators you use.

Marker APIs in `std.testing` carry metadata that `incan test` consumes during discovery. This keeps marker behavior in the runner and prevents regular runtime calls to marker functions.

Conditional markers run during collection:

```incan
from std.testing import assert_eq, feature, platform, skipif, xfailif

@skipif(platform() == "windows", reason="path semantics differ")
def test_posix_path() -> None:
    assert_eq("/", "/")

@xfailif(feature("new_parser"), reason="tracked parser bug")
def test_new_parser_case() -> None:
    assert_eq(parse("..."), expected)
```

Pass `incan test --feature new_parser` to make `feature("new_parser")` true during collection.

## Fixtures

### The problem fixtures solve

Tests often need some shared setup — a database connection, a temporary file, a logged-in user. Without fixtures you end up repeating that setup in every test:

```incan
def test_query_users() -> None:
    db = Database.connect("test.db")    # repeated setup
    result = db.query("SELECT * FROM users")
    assert_eq(len(result), 3)
    db.close()                          # repeated teardown

def test_insert_user() -> None:
    db = Database.connect("test.db")    # same setup, again
    db.insert("users", {"name": "Alice"})
    assert_eq(db.count("users"), 4)
    db.close()                          # same teardown, again
```

This is tedious, error-prone (forget one `db.close()` and you leak a connection), and makes the actual test logic harder to spot.

### Declaring a fixture

A **fixture** is a function decorated with `@fixture` that produces a value your tests can reuse.

Mark it with the `@fixture` decorator and return the value:

```incan
from std.testing import fixture

@fixture
def database() -> Database:
    return Database.connect("test.db")
```

### Using a fixture in a test

To use a fixture, add a parameter to your test function **whose name matches the fixture function**. The test runner sees the matching name, calls the fixture, and passes the result in automatically:

```incan
def test_query_users(database: Database) -> None:
    result = database.query("SELECT * FROM users")
    assert_eq(len(result), 3)
```

You don't call the fixture yourself — `incan test` handles that. The parameter name `database` is what connects the test to the `database()` fixture.

### Teardown with `yield`

If your fixture needs cleanup after the test finishes, use `yield` instead of `return`. Everything before `yield` is setup; everything after is teardown:

```incan
@fixture
def database() -> Database:
    db = Database.connect("test.db")
    yield db          # test receives `db` here
    db.close()        # runs after the test finishes, even if it failed
```

Teardown can reference setup locals such as `db` and fixture parameters. If teardown fails, the test run fails. Timeout-enforced worker termination can still bypass teardown.

### Async fixtures

Use the same `@fixture` decorator for async setup and teardown. The only surface difference is that the fixture is declared with `async def`:

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

Async fixtures use `yield` exactly once. Setup before `yield` is awaited before any dependent fixture or test runs. Teardown after `yield` is awaited after the dependent test or scope finishes, and before the runner proceeds to the next dependent teardown. There is no separate async fixture decorator.

### Fixture scopes

By default, a fixture is created and torn down for **each test** that uses it. If the setup is expensive, share it across a wider scope with the `scope` argument:

```incan
@fixture(scope="module")
def shared_client() -> Client:
    client = Client.connect("https://api.example.com")
    yield client
    client.disconnect()
```

| Scope                      | Lifetime                                                                                          |
| -------------------------- | ------------------------------------------------------------------------------------------------- |
| `"function"` (the default) | Created and torn down for each test.                                                              |
| `"module"`                 | Shared across all tests from one source file inside a worker batch.                               |
| `"session"`                | Shared across a worker batch; with `--jobs 1`, compatible tests can share it across source files. |

Choose the narrowest scope that makes sense. `"function"` keeps tests fully isolated; wider scopes trade isolation for speed.

### Fixtures using other fixtures

Fixtures can depend on other fixtures, just like tests do. Use the same name-matching pattern:

```incan
@fixture
def database() -> Database:
    db = Database.connect("test.db")
    yield db
    db.close()

@fixture
def populated_db(database: Database) -> Database:
    database.insert("users", {"name": "Alice"})
    database.insert("users", {"name": "Bob"})
    return database

def test_user_count(populated_db: Database) -> None:
    assert_eq(populated_db.count("users"), 2)
```

The test runner resolves the dependency chain for you: `populated_db` needs `database`, so `database()` runs first, then its result is passed into `populated_db()`.

Sync and async fixtures can be mixed in the same dependency graph:

```incan
from std.async import sleep_ms
from std.testing import assert_eq, fixture

@fixture
def seed() -> int:
    return 40

@fixture
async def resource(seed: int) -> int:
    await sleep_ms(1)
    yield seed + 2
    await sleep_ms(1)

@fixture
def doubled(resource: int) -> int:
    return resource * 2

def test_mixed_fixture_graph(doubled: int) -> None:
    assert_eq(doubled, 84)
```

The runner awaits async setup before sync dependents run, and it still tears fixtures down in reverse dependency order. In this example, `doubled` finishes first, then `resource` teardown is awaited, then `seed` leaves scope.

## Parametrized tests

When you want to test the same logic with different inputs, you could write a separate test for each case:

```incan
def test_add_positive() -> None:
    assert_eq(add(1, 2), 3)

def test_add_zeros() -> None:
    assert_eq(add(0, 0), 0)

def test_add_negative() -> None:
    assert_eq(add(-1, 1), 0)
```

This works, but the test logic is identical every time — only the data changes. `@parametrize` lets you write the logic once and supply a table of inputs:

```incan
from std.testing import parametrize

@parametrize("x, y, expected", [
    (1, 2, 3),
    (0, 0, 0),
    (-1, 1, 0),
])
def test_add(x: int, y: int, expected: int) -> None:
    assert_eq(add(x, y), expected)
```

The first argument is a comma-separated string of parameter names. The second is a list of tuples — one tuple per test case. Each tuple is unpacked into the named parameters.

The test runner generates a separate test case per tuple, with the values shown in the test ID:

```bash
test_add[1-2-3] ... PASSED
test_add[0-0-0] ... PASSED
test_add[-1-1-0] ... PASSED
```

Adding a new case is just one more tuple — no new function needed.

Parametrized tests expand before fixture resolution. Each expanded case resolves fixtures by name after the case's parameter values are known. Function-scoped fixtures run separately for each expanded case, while module-scoped and session-scoped fixtures keep their normal cache boundaries.

Use `param_case(...)` when one parameter set needs a stable id or marker:

```incan
from std.testing import assert_eq, param_case, parametrize, xfail

@parametrize("x, expected", [
    param_case((1, 3), id="known-bug", marks=[xfail("tracked bug")]),
    param_case((2, 4), id="happy-path"),
])
def test_double(x: int, expected: int) -> None:
    assert_eq(x * 2, expected)
```

## Timeouts and worker termination

Use `incan test --timeout <duration>` or `@timeout("duration")` to set a timeout for the generated test batch. Fixtures do not have their own timeout configuration.

For async fixtures, the runner awaits teardown after ordinary assertion failures and panics while the worker remains alive. `--timeout`, `@timeout`, external interruption, and process termination are enforced at the generated worker or batch level; if that enforcement terminates the worker process, remaining teardown is best-effort and may not run. Keep teardown idempotent and prefer narrow fixture scopes for resources that must be released promptly.

## Full example

```incan
from std.testing import assert_eq, assert_true, assert_is_some, fixture, skip

@fixture
def database() -> Database:
    db = Database.connect("test.db")
    yield db
    db.close()

def add(a: int, b: int) -> int:
    return a + b

def find_user(name: str) -> Option[str]:
    if name == "alice":
        return Some("alice@example.com")
    return None

def test_add() -> None:
    assert_eq(add(2, 3), 5)
    assert_true(add(1, 1) == 2)

def test_find_user() -> None:
    email = assert_is_some(find_user("alice"))
    assert_eq(email, "alice@example.com")

@skip("not implemented yet")
def test_future_feature() -> None:
    pass
```
