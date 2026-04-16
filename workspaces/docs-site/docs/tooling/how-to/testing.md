# Testing in Incan

Incan provides a pytest-like testing experience with `incan test`.

For the **testing API** (assertions, markers, fixtures, parametrization), see:
[Standard library reference → `std.testing`](../../language/reference/stdlib/testing.md).

For a guided walkthrough, see:
[The Incan Book → Unit tests](../../language/tutorials/book/13_unit_tests.md).

## Quick Start

--8<-- "_snippets/callouts/no_install_fallback.md"

!!! note "If something fails"
    If you run into errors, see [Troubleshooting](troubleshooting.md).
    If it still looks like a bug, please [file an issue on GitHub](https://github.com/dannys-code-corner/incan/issues).

Create a test file (must be named `test_*.incn` or `*_test.incn`):

```incan
"""Test file for math operations"""

from std.testing import assert_eq

def add(a: int, b: int) -> int:
    return a + b


def test_addition() -> None:
    result = add(2, 3)
    assert_eq(result, 5)


def test_subtraction() -> None:
    result = 10 - 3
    assert_eq(result, 7)
```

Run tests:

```bash
incan test tests/
```

## Test Discovery

Tests are discovered automatically:

- **Files**: Named `test_*.incn` or `*_test.incn`
- **Functions**: Named `def test_*()`

```bash
my_project/
├── src/
│   └── main.incn
└── tests/
    ├── test_math.incn      # ✓ discovered
    ├── test_strings.incn   # ✓ discovered
    └── helpers.incn        # ✗ not a test file
```

## Assertions

Use assertion functions from the `std.testing` module (not Python-style `assert expr`):

```incan
from std.testing import assert, assert_eq, assert_ne, assert_true, assert_false, fail

# Equality
assert_eq(actual, expected)
assert_ne(actual, other)

# Boolean
assert(condition)
assert_true(condition)
assert_false(condition)

# Explicit failure
fail("this test should not reach here")
```

## Markers

### @skip - Skip a test

```incan
@skip("not implemented yet")
def test_future_feature() -> None:
    pass
```

Output: `test_future_feature SKIPPED (not implemented yet)`

### @xfail - Expected failure

```incan
@xfail("known bug #123")
def test_known_issue() -> None:
    assert_eq(buggy_function(), "fixed")
```

If test fails: `XFAIL` (expected)
If test passes: `XPASS` (unexpected - reported as failure)

### @slow - Mark slow tests

```incan
@slow
def test_integration() -> None:
    # Long-running test
    pass
```

Slow tests are excluded by default. Include with `--slow`.

## CLI Options

```bash
# Run all tests in directory
incan test tests/

# Run specific file
incan test tests/test_math.incn

# Filter by keyword
incan test -k "addition"

# Verbose output (show timing)
incan test -v

# Stop on first failure
incan test -x

# Include slow tests
incan test --slow

# Fail if no tests are collected
incan test --fail-on-empty
```

## Output Format

```bash
=================== test session starts ===================
collected 4 item(s)

test_math.incn::test_addition PASSED
test_math.incn::test_subtraction PASSED
test_math.incn::test_division FAILED
test_math.incn::test_future SKIPPED (not implemented)

=================== FAILURES ===================
___________ test_division ___________

    assertion failed: `assert_eq(10 / 3, 3)`
    left: 3.333...
    right: 3

    tests/test_math.incn::test_division

=================== 2 passed, 1 failed, 1 skipped in 0.05s ===================
```

## Exit Codes

| Code | Meaning                                                                 |
| ---- | ----------------------------------------------------------------------- |
| 0    | All tests passed (or no tests collected without `--fail-on-empty`)      |
| 1    | Any test failed, no test files found, or `--fail-on-empty` found none   |

## CI Integration

```yaml
# GitHub Actions
- name: Run tests
  run: incan test --fail-on-empty tests/
```

## Fixtures

Fixtures provide setup/teardown and dependency injection for tests.

### Basic Fixture

```incan
from std.testing import fixture

@fixture
def database() -> Database:
    """Provides a test database."""
    db = Database.connect("test.db")
    yield db          # Test runs here
    db.close()        # Teardown (always runs, even on failure)

def test_insert(database: Database) -> None:
    database.insert("key", "value")
    assert_eq(database.get("key"), "value")
```

### Fixture Scopes

Control when fixtures are created/destroyed:

```incan
@fixture(scope="function")  # Default: new per test
def temp_file() -> str:
    ...

@fixture(scope="module")    # Shared across file
def shared_client() -> Client:
    ...

@fixture(scope="session")   # Shared across entire run
def global_config() -> Config:
    ...
```

### Fixture Dependencies

Fixtures can depend on other fixtures:

```incan
@fixture
def config() -> Config:
    return Config.load("test.toml")

@fixture
def database(config: Config) -> Database:
    # config fixture is automatically injected
    return Database.connect(config.db_url)

def test_query(database: Database) -> None:
    result = database.query("SELECT 1")
    assert_eq(result, 1)
```

### Autouse Fixtures

Auto-apply fixtures to all tests in scope:

```incan
@fixture(autouse=true)
def setup_logging() -> None:
    """Automatically applied to all tests in this file."""
    logging.set_level("DEBUG")
    yield
    logging.set_level("INFO")
```

## Parametrize

Run a test with multiple parameter sets:

```incan
from std.testing import parametrize

@parametrize("a, b, expected", [
    (1, 2, 3),
    (0, 0, 0),
    (-1, 1, 0),
    (100, 200, 300),
])
def test_add(a: int, b: int, expected: int) -> None:
    assert_eq(add(a, b), expected)
```

Output:

```bash
test_math.incn::test_add[1-2-3] PASSED
test_math.incn::test_add[0-0-0] PASSED
test_math.incn::test_add[-1-1-0] PASSED
test_math.incn::test_add[100-200-300] PASSED
```

### Named Test IDs

```incan
@parametrize("input, expected", [
    ("hello", "HELLO"),
    ("World", "WORLD"),
    ("", ""),
], ids=["lowercase", "mixed", "empty"])
def test_upper(input: str, expected: str) -> None:
    assert_eq(input.upper(), expected)
```

### Combining Fixtures and Parametrize

```incan
@fixture
def database() -> Database:
    db = Database.connect("test.db")
    yield db
    db.close()

@parametrize("key, value", [
    ("name", "Alice"),
    ("age", "30"),
])
def test_insert(database: Database, key: str, value: str) -> None:
    database.insert(key, value)
    assert_eq(database.get(key), value)
```

## Async Tests (Coming Soon)

Support for async test functions and fixtures with Tokio:

```incan
import std.async
from std.testing import fixture

@fixture
async def http_server() -> ServerHandle:
    server = await start_server(port=0)
    yield server
    await server.shutdown()

async def test_endpoint(http_server: ServerHandle) -> None:
    response = await fetch(f"http://localhost:{http_server.port}/health")
    assert_eq(response.status, 200)
```

## Best Practices

1. **One assertion per test** - Makes failures easier to diagnose
2. **Descriptive test names** - `test_user_creation_with_invalid_email_fails`
3. **Keep tests fast** - Mark slow tests with `@slow`
4. **Use xfail for known bugs** - Track them without blocking CI
