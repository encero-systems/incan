# `std.testing` standard library guide

This page covers what `std.testing` provides and how to use it in your tests.

> If you want CLI usage (`incan test`, discovery, flags), see [Tooling: Testing](../../tooling/how-to/testing.md).
> If you want the API reference only, see [Standard library reference: `std.testing`](../reference/stdlib/testing.md).

## Assertion helpers

```incan
from std.testing import assert, assert_eq, assert_ne, assert_true, assert_false, fail
from std.testing import assert_is_some, assert_is_none, assert_is_ok, assert_is_err
```

|            Function             |           Fails when           | Returns |
| ------------------------------- | ------------------------------ | ------- |
| `assert(condition, msg?)`       | `condition` is false           | —       |
| `assert_true(condition, msg?)`  | `condition` is false           | —       |
| `assert_false(condition, msg?)` | `condition` is true            | —       |
| `assert_eq(left, right, msg?)`  | `left != right`                | —       |
| `assert_ne(left, right, msg?)`  | `left == right`                | —       |
| `assert_is_some(option, msg?)`  | `option` is `None`             | `T`     |
| `assert_is_none(option, msg?)`  | `option` is `Some(...)`        | —       |
| `assert_is_ok(result, msg?)`    | `result` is `Err(...)`         | `T`     |
| `assert_is_err(result, msg?)`   | `result` is `Ok(...)`          | `E`     |
| `fail(msg)`                     | Always (unconditional failure) | —       |

All `msg` parameters are optional. When omitted, a sensible default message is used.

## `assert_raises`

`std.testing.assert_raises` is not available for ordinary use yet.

- The name remains part of the public testing surface.
- Calling it currently fails with a clear "not implemented yet" diagnostic.
- For now, prefer the available helpers such as `assert_is_err(...)` or test the `Result` value directly.

## Assert statement syntax (planned)

Incan will support `assert` as a statement keyword. When available, the mapping will be:

|        Statement        |              Desugars to              |
| ----------------------- | ------------------------------------- |
| `assert cond`           | `std.testing.assert(cond)`            |
| `assert a == b`         | `std.testing.assert_eq(a, b)`         |
| `assert a != b`         | `std.testing.assert_ne(a, b)`         |
| `assert opt is Some(v)` | `v = std.testing.assert_is_some(opt)` |
| `assert opt is None`    | `std.testing.assert_is_none(opt)`     |
| `assert res is Ok(v)`   | `v = std.testing.assert_is_ok(res)`   |
| `assert res is Err(e)`  | `e = std.testing.assert_is_err(res)`  |

Until then, use the function-call forms directly.

## Markers and decorators

Test markers control how `incan test` discovers and runs tests:

|              Decorator              |                             Effect                              |
| ----------------------------------- | --------------------------------------------------------------- |
| `@skip(reason?)`                    | Skips the test unconditionally.                                 |
| `@xfail(reason?)`                   | Marks the test as expected to fail (XPASS if it passes).        |
| `@slow`                             | Excludes the test by default; include with `incan test --slow`. |
| `@fixture`                          | Declares a test fixture (see below).                            |
| `@parametrize(argnames, argvalues)` | Runs the test once per parameter set.                           |

Markers are declared in `testing.incn` with marker metadata, and `incan test` consumes that metadata during discovery.
This keeps marker behavior in the runner and prevents regular runtime calls to marker functions.

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

Mark it with the `@fixture` decorator and return (or yield) the value:

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
    yield db          # <-- test receives `db` here
    db.close()        # <-- runs after the test finishes, even if it failed
```

This guarantees cleanup runs regardless of whether the test passes or fails — no more leaked connections or orphaned temp files.

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

### Fixture scopes

By default, a fixture is created and torn down for **each test** that uses it. If the setup is expensive (e.g., a database connection or a network client), you can share it across a wider scope with the `scope` argument:

```incan
@fixture(scope="module")
def shared_client() -> Client:
    client = Client.connect("https://api.example.com")
    yield client
    client.disconnect()
```

|           Scope            |                 Lifetime                  |
| -------------------------- | ----------------------------------------- |
| `"function"` (the default) | Created and torn down for each test.      |
| `"module"`                 | Shared across all tests in one test file. |
| `"session"`                | Shared across the entire test session.    |

Choose the narrowest scope that makes sense — `"function"` keeps tests fully isolated, while wider scopes trade isolation for speed. You have to decide what is best for your test suite and your use case.

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
