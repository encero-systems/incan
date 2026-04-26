# Testing in Incan

Incan provides a pytest-like test runner with `incan test`.

For the **language model for writing tests** (inline `module tests:`, assertions, markers, fixtures, parametrization), see: [Language → Testing in Incan](../../language/how-to/testing_stdlib.md).

For the **API reference** only, see: [Standard library reference → `std.testing`](../../language/reference/stdlib/testing.md).

For a guided walkthrough, see: [The Incan Book → Unit tests](../../language/tutorials/book/13_unit_tests.md).

## Quick Start

--8<-- "_snippets/callouts/no_install_fallback.md"

!!! note "If something fails"
    If you run into errors, see [Troubleshooting](troubleshooting.md).
    If it still looks like a bug, please [file an issue on GitHub](https://github.com/dannys-code-corner/incan/issues).

You can write tests in either of two places:

- a conventional test file named `test_*.incn` or `*_test.incn`
- an inline `module tests:` block inside a production `.incn` source file

Use a conventional test file when the test is mostly black-box or spans multiple modules. Use an inline `module tests:` block when the test belongs next to the implementation or needs same-file private helpers.

Create a conventional test file, for example `tests/test_math.incn`:

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

Tests are discovered automatically from two sources:

- **Conventional test files**: files named `test_*.incn` or `*_test.incn`
- **Inline test modules**: non-test `.incn` source files that contain a parsed `module tests:` block
- **Test functions**: functions named `def test_*()` in the active test context

```bash
my_project/
├── src/
│   ├── main.incn           # ✗ not discovered unless it contains module tests:
│   └── math.incn           # ✓ discovered if it contains module tests:
└── tests/
    ├── test_math.incn      # ✓ discovered
    ├── test_strings.incn   # ✓ discovered
    └── helpers.incn        # ✗ not a test file
```

For conventional test files, `incan test` collects top-level `def test_*()` functions and top-level fixtures. These files must not contain `module tests:`; keep them as ordinary test modules.

For production source files, `incan test` ignores top-level `def test_*()` functions and only collects tests and fixtures declared inside the file's `module tests:` block. That rule keeps accidental production functions from becoming tests.

## Inline `module tests:`

Inline test modules let you keep unit tests next to the code they exercise:

```incan
def normalize_name(name: str) -> str:
    return name.strip().lower()

def is_internal_name(name: str) -> bool:
    return normalize_name(name).startswith("_")

module tests:
    from std.testing import assert_eq, assert_false

    def test_normalize_name() -> None:
        assert_eq(normalize_name("  Alice  "), "alice")

    def test_private_helper() -> None:
        assert_false(is_internal_name("public"))
```

Run the project or file normally:

```bash
incan test .
incan test src/math.incn
```

Inline test modules have test-only scope:

- Names from the enclosing file, including non-`pub` helpers, are visible inside `module tests:`.
- Imports and helpers declared inside `module tests:` do not leak into the production module.
- `incan build` and `incan run` strip the inline test body from generated production output.
- `incan test` compiles the inline test body as runner-only code and executes its `def test_*()` functions.

Put `std.testing` imports inside `module tests:` unless production code also needs that module:

```incan
def production_value() -> int:
    return 42

module tests:
    from std.testing import assert_eq

    def test_production_value() -> None:
        assert_eq(production_value(), 42)
```

Do not place `module tests:` in a conventional test file:

```incan
# tests/test_math.incn

module tests:
    def test_not_valid_here() -> None:
        pass
```

Use top-level tests in named test files instead:

```incan
# tests/test_math.incn

def test_valid_here() -> None:
    pass
```

## Assertions

Use the language `assert` statement for simple assertions:

```incan
def test_arithmetic() -> None:
    assert 1 + 1 == 2
    assert 3 > 2, "ordering changed"
```

Import assertion helpers from `std.testing` when a function-call helper is clearer or when you need a returned unwrapped value:

```incan
from std.testing import assert_eq, assert_ne, assert_true, assert_false, fail

# Equality
assert_eq(actual, expected)
assert_ne(actual, other)

# Boolean helpers
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
