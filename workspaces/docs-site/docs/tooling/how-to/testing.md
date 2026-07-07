# Testing in Incan

Incan provides a pytest-like test runner with `incan test`.

For the **language model for writing tests** (inline `module tests:`, assertions, markers, fixtures, parametrization), see: [Language → Testing in Incan](../../language/how-to/testing_stdlib.md).

For the **API reference** only, see: [Standard library reference → `std.testing`](../../language/reference/stdlib/testing.md).

For a guided walkthrough, see: [The Incan Book → Unit tests](../../language/tutorials/book/13_unit_tests.md).

## Quick Start

--8<-- "_snippets/callouts/no_install_fallback.md"

!!! note "If something fails"
    If you run into errors, see [Troubleshooting](troubleshooting.md). If it still looks like a bug, please [file an issue on GitHub](https://github.com/encero-systems/incan/issues).

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

For long suites, use `incan test -v` to see generated-harness planning, preparation-cache hits or misses, preheat timing, and Cargo test phase timing. Ordinary console runs still show the collected test count and any generated-harness preheat that has to run; when a cold preheat invokes Cargo, Cargo's own progress is streamed instead of hidden until the end. Verbose mode is the better troubleshooting view when you need to tell whether time is going into package preparation, Rust metadata prewarm, Cargo preheat, or actual test execution.

## Test Discovery

Tests are discovered automatically from two sources:

- **Conventional test files**: files named `test_*.incn` or `*_test.incn`
- **Inline test modules**: non-test `.incn` source files that contain a parsed `module tests:` block
- **Test functions**: functions named `def test_*()` in the active test context
- **Explicit test decorators**: functions decorated with `@test` in the active test context

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

Inline test modules support the same runner features as conventional test files, including explicit `@test` discovery, fixture injection, parametrization, marker selection, strict marker registries, and timeouts:

```incan
def bounded_discount(percent: int) -> int:
    if percent < 0:
        return 0
    if percent > 100:
        return 100
    return percent

module tests:
    from std.testing import assert_eq, fixture, mark, parametrize, test

    TEST_MARKERS = ["edge"]

    @fixture
    def base_percent() -> int:
        return 75

    @test
    def named_by_decorator(base_percent: int) -> None:
        assert_eq(bounded_discount(base_percent), 75)

    @mark("edge")
    @parametrize("adjustment, expected", [
        (-100, 0),
        (50, 100),
    ], ids=["floor", "cap"])
    def test_discount_edges(base_percent: int, adjustment: int, expected: int) -> None:
        assert_eq(bounded_discount(base_percent + adjustment), expected)
```

```bash
incan test --list src/pricing.incn
incan test -k "test_discount_edges[cap]" src/pricing.incn
incan test -m edge --strict-markers src/pricing.incn
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

If test fails: `XFAIL` (expected) If test passes: `XPASS` (unexpected - reported as failure)

### @slow - Mark slow tests

```incan
@slow
def test_integration() -> None:
    # Long-running test
    pass
```

Slow tests are excluded by default. Include with `--slow`.

### @test - Explicitly mark a test

```incan
from std.testing import test

@test
def checks_total() -> None:
    assert_eq(total(), 42)
```

Use `@test` when the function name should not start with `test_`.

## CLI Options

```bash
# Run all tests in directory
incan test tests/

# Run specific file
incan test tests/test_math.incn

# Filter by keyword
incan test -k "addition"

# List collected tests without running them
incan test --list tests/

# Verbose output (show timing and generated-harness preheat diagnostics)
incan test -v

# Stop on first failure
incan test -x

# Include slow tests
incan test --slow

# Select by marker expression
incan test -m "smoke and not slow" tests/

# Enforce marker registration
incan test --strict-markers tests/

# Enable collection-time feature probes
incan test --feature new_parser tests/

# Fail long-running generated test batches
incan test --timeout 5s tests/

# Print passing-test output
incan test --nocapture tests/

# Fail if no tests are collected
incan test --fail-on-empty

# Run expected-failure tests as ordinary tests
incan test --run-xfail tests/

# Show the slowest tests
incan test --durations 10 tests/

# Shuffle with a reproducible seed
incan test --shuffle --seed 12345 tests/

# Run independent worker batches concurrently
incan test --jobs 4 tests/
```

`-k` matches the stable test id shown by `--list`, for example `tests/test_math.incn::test_addition` or `tests/test_math.incn::test_add[1-2-3]`.

When a generated Rust test harness is new or stale, `incan test` preheats it with `cargo test --no-run` before executing the tests. This keeps subsequent runs on the already-built path instead of surprising the next hot test command with a full Cargo compile. If two Incan processes reach the same stale harness at once, one process performs the preheat and the other waits for the fingerprint to be written.

`-m` matches marker names from decorators such as `@slow` and `@mark("smoke")`, plus default marks from `TEST_MARKS`. Use `TEST_MARKERS` with `--strict-markers` to make unknown marker names a collection error.

`--timeout` and `@timeout` apply to generated test batches. They do not configure individual fixtures. The runner awaits async fixture teardown after ordinary assertion failures and panics while the worker remains alive. If timeout enforcement or external interruption terminates the worker process, remaining fixture teardown is best-effort and may not run.

Conditional markers are evaluated during collection:

```incan
from std.testing import assert_eq, feature, platform, skipif, xfailif

@skipif(platform() == "windows", reason="path semantics differ")
def test_posix_path() -> None:
    assert_eq("/", "/")

@xfailif(feature("new_parser"), reason="tracked parser bug")
def test_new_parser_case() -> None:
    assert_eq(parse("..."), expected)
```

Pass `--feature new_parser` to make `feature("new_parser")` true for collection.

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

For machine-readable CI output, use JSON Lines, JUnit XML, or both:

```bash
incan test --format json --junit reports/junit.xml tests/
```

Each JSON result record includes `schema_version: "incan.test.v1"`, a stable `test_id`, status, file, name, and duration. A final summary record closes the stream.

## Fixtures

Fixtures provide setup values and dependency injection for tests.

Shared fixtures can live in `tests/**/conftest.incn`; the runner loads matching conftest files for tests in that directory subtree. The runner also provides built-in `tmp_path`, `tmp_workdir`, and `env` fixtures by parameter name. `conftest.incn` fixtures are scoped to conventional tests under `tests/**`; they do not apply to inline `module tests:` blocks in production source directories.

### Basic Fixture

```incan
from std.testing import fixture

@fixture
def database() -> Database:
    """Provides a test database."""
    return Database.connect("test.db")

def test_insert(database: Database) -> None:
    database.insert("key", "value")
    assert_eq(database.get("key"), "value")
```

### Fixture Scope

Function-scoped fixtures are created each time a test needs them. Module fixtures are cached once per source file in a worker batch. Session fixtures are cached once per worker batch, so `--jobs 1` can share one session fixture instance across compatible collected files and `--jobs N` shares one instance per worker.

```incan
from std.testing import assert_eq, fixture

static calls: int = 0

@fixture(scope="module")
def once() -> int:
    calls += 1
    return calls

def test_first(once: int) -> None:
    assert_eq(once, 1)

def test_second(once: int) -> None:
    assert_eq(once, 1)
```

Fixtures can use a top-level `yield` for teardown:

```incan
@fixture
def resource() -> int:
    handle: int = open_resource()
    yield handle
    cleanup_resource(handle)
```

The teardown block runs after the test body for function fixtures, after all tests from the source file for module fixtures, and at the end of the worker batch for session fixtures. Teardown runs after assertion failures, can reference setup locals and fixture parameters, and fails the run if teardown itself fails. Timeout termination can still bypass teardown because the worker process may be killed.

### Async Fixtures

Use the same `@fixture` decorator for asynchronous setup. Declare the fixture with `async def`, await setup before `yield`, and await teardown after `yield`:

```incan
from std.async import sleep_ms
from std.testing import assert_eq, fixture

@fixture(scope="function")
async def resource() -> int:
    await sleep_ms(1)
    yield 42
    await sleep_ms(1)

async def test_uses_resource(resource: int) -> None:
    await sleep_ms(1)
    assert_eq(resource, 42)
```

Async fixtures must use `yield` exactly once. The yielded value is injected by name just like a synchronous fixture value. Setup before `yield` is awaited before dependents run, and teardown after `yield` is awaited before the runner continues to dependent teardowns or the next case.

Synchronous and asynchronous fixtures can depend on each other in one graph:

```incan
from std.async import sleep_ms
from std.testing import fixture

@fixture
def seed() -> int:
    return 40

@fixture
async def resource(seed: int) -> int:
    await sleep_ms(1)
    yield seed + 2
    await sleep_ms(1)
```

Scopes do not change for async fixtures. A function-scoped async fixture runs per test case, a module-scoped async fixture is cached for the source file inside the worker batch, and a session-scoped async fixture is cached for the worker batch. The runner awaits async setup before any dependent sync or async fixture starts, then tears down in reverse dependency order.

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
```

Autouse fixtures respect scope: function autouse runs per test, while module/session autouse is cached in the generated harness process.

## Parametrize

Run a test with multiple parameter sets:

```incan
from std.testing import assert_eq, parametrize

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
from std.testing import assert_eq, parametrize

@parametrize("input, expected", [
    ("hello", "HELLO"),
    ("World", "WORLD"),
    ("", ""),
], ids=["lowercase", "mixed", "empty"])
def test_upper(input: str, expected: str) -> None:
    assert_eq(input.upper(), expected)
```

Use `param_case(...)` when only one case needs its own id or marks:

```incan
from std.testing import assert_eq, param_case, parametrize, xfail

@parametrize("x, expected", [
    param_case((1, 3), id="known-bug", marks=[xfail("tracked bug")]),
    param_case((2, 4), id="happy-path"),
])
def test_double(x: int, expected: int) -> None:
    assert_eq(x * 2, expected)
```

### Combining Fixtures and Parametrize

```incan
@fixture
def database() -> Database:
    return Database.connect("test.db")

@parametrize("key, value", [
    ("name", "Alice"),
    ("age", "30"),
])
def test_insert(database: Database, key: str, value: str) -> None:
    database.insert(key, value)
    assert_eq(database.get(key), value)
```

Parametrization expands before fixture resolution. In the example above, the runner first creates `test_insert[name-Alice]` and `test_insert[age-30]`, then resolves `database` for each expanded case under the fixture's scope rules. Function-scoped fixtures run once per expanded case; module and session fixtures reuse cached instances when their normal scope permits it.

## Async Tests

```incan
from std.async import sleep_ms
from std.testing import assert_eq, fixture

@fixture
async def resource() -> int:
    await sleep_ms(1)
    yield 42
    await sleep_ms(1)

async def test_endpoint(resource: int) -> None:
    await sleep_ms(1)
    assert_eq(resource, 42)
```

Async tests and async fixtures run on the test runner's async runtime. User code should use ordinary `await`; there is no test-level runtime setup hook and no async-only fixture decorator.

## Best Practices

1. **One assertion per test** - Makes failures easier to diagnose
2. **Descriptive test names** - `test_user_creation_with_invalid_email_fails`
3. **Keep tests fast** - Mark slow tests with `@slow`
4. **Use xfail for known bugs** - Track them without blocking CI
