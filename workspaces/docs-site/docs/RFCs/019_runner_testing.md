# RFC 019: test runner, CLI, and ecosystem

- **Status:** Planned
- **Created:** 2026-01-14
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 018 (language primitives for testing), RFC 007 (runner semantics; superseded), RFC 001 (runner portions; superseded), RFC 002 (runner portions; superseded)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/77
- **RFC PR:** —
- **Written against:** v0.1
- **Shipped in:** —

## Summary

Define the test runner and CLI semantics for Incan (pytest-inspired), including:

- discovery in test files and inline test contexts
- fixtures (scopes, autouse, teardown via `yield`)
- parametrized tests (`@parametrize`, `case(...)`, ids)
- markers and selection (`-m`, `skip/xfail/slow`, strict markers)
- parallel execution and resource locking (`--jobs`, `@resource`, `@serial`)
- `tests/**/conftest.incn` auto-discovery and precedence rules
- built-in fixtures (`tmp_path`, `tmp_workdir`, `env`)
- timeouts (CLI + per-test override)
- output/reporting (`-k`, `--list`, shuffle/seed, durations, JSON/JUnit)

Language constructs (`assert`, pattern binding, `module tests:`) are specified in **RFC 018**. This RFC is jointly normative with RFC 018.

## Motivation

Testing is a *system feature*: discovery, fixtures, parametrization, markers, parallelism, and reporting all interact. This RFC captures the runner/CLI behavior in one place, while language primitives are specified in RFC 018.

## Goals

- Define the runner and CLI contract for Incan testing in one place.
- Standardize discovery, fixtures, parametrization, markers, execution, timeouts, and reporting behavior.
- Keep the compiler-facing language semantics split cleanly across RFC 018 and RFC 019.
- Preserve a user-facing testing model that is Pythonic in ergonomics but explicit in semantics.

## Non-Goals

- Redefining language-level `assert` or inline test block semantics already owned by RFC 018.
- Delegating the public testing contract to Cargo, pytest, or another external runner.
- Leaving core runner semantics to repo-local scripts or convention-only behavior.

## Guide-level explanation (how users think about it)

### How to read this RFC

This RFC specifies **runner and CLI** behavior: discovery, collection-time evaluation, fixture injection and lifecycle, execution model (`--jobs`), selection (`-k`/`-m`), timeouts, and report formats.

Language constructs (`assert`, `module tests:`) are specified in RFC 018.

Scope boundary:

- **In scope here**: discovery, fixtures, parametrization, markers, execution model, timeouts, reporting.
- **Defined in RFC 018**: `assert`, pattern binding, and `module tests:` language semantics.

Suggested reading order:

1. Guide-level explanation to form the mental model.
2. Reference-level rules for precise behavior and edge cases.
3. Implementation plan + conformance checklist for build order and tests.

Quick navigation:

- Guide-level sections (discovery, fixtures, parametrization, markers, execution, reporting)
- Reference-level explanation (precise rules)
- Implementation plan and conformance checklist
- Appendix (runner surface inventory)

Quick reference:

- Discovery: test files named `test_*.incn` / `*_test.incn`; inline `module tests:` blocks in source files.
- Fixtures: injected by parameter name; `@fixture(scope=...)`, autouse supported.
- Parametrize: `@parametrize` data must be collection-time evaluatable.
- Markers: `@skip`/`@xfail`/`@slow`/`@mark`, selected via `-m`.
- Parallelism: worker processes; session fixtures are per worker.
- Reporting: stable test ids drive `-k`, `--list`, JSON/JUnit.

### Differences from pytest / cargo test / jest (important)

This RFC is pytest-inspired, but there are a few deliberate differences that are easy to miss if you bring strong pytest/Rust/Jest muscle memory:

- **`@slow` is excluded by default**: you must opt in via `incan test --slow` (closer to Rust’s ignored tests than pytest
markers).
- **`skipif` / `xfailif` conditions must be collection-time evaluatable** (const-evaluable + explicit `testing` probes).
- **`conftest.incn` is only auto-discovered under `tests/`** (it does not apply to inline `module tests:` in `src/`).
- **Parallel execution (`--jobs`) uses worker processes**, so process-global state (like `tmp_workdir` and `env`) remains
isolated.
- **`assert` can bind names via limited patterns** (e.g. `assert opt is Some(v)`), defined in RFC 018.

Quick CLI anchoring:

| Ecosystem    | Concept                        | Incan equivalent              |
| ------------ | ------------------------------ | ----------------------------- |
| pytest       | fail-fast (`-x`)               | `incan test -x`               |
| pytest-xdist | parallel workers (`-n 8`)      | `incan test -j 8`             |
| pytest       | keyword filter (`-k`)          | `incan test -k <substr>`      |
| pytest       | marker filter (`-m`)           | `incan test -m <expr>`        |
| pytest       | strict markers                 | `incan test --strict-markers` |
| Rust         | ignored-by-default tests       | `@slow` + `incan test --slow` |
| Rust         | show output (`-- --nocapture`) | `incan test --nocapture`      |

### The `testing` module

Testing utilities are normal functions/decorators imported from the `testing` module:

```incan
from testing import assert_eq, assert_true, assert_false, fail
```

Language-level `assert` semantics and the `testing.assert_*` mapping are defined in RFC 018.

### Where tests can live (two test contexts)

#### 1) Test files

Put tests under `tests/`, using test file naming:

- `test_*.incn`
- `*_test.incn`

Example:

```incan title="tests/test_math.incn"
"""Unit tests for math."""

from testing import assert_eq

def add(a: int, b: int) -> int:
    return a + b

def test_addition() -> None:
    assert_eq(add(2, 3), 5)
```

Note: In test files, the entire file is already a test context. Do not wrap tests in `module tests:`; that construct is reserved for inline tests in production source files.

#### 2) Inline test-only module blocks (recommended for unit tests)

Inline tests live next to production code, but inside a **test-only module block**.

See RFC 018 for the language-level `module tests:` example and scoping details.

This keeps helpers/fixtures/test imports scoped to the test module. Stripping behavior in non-test builds is defined in RFC 018.

A module block named `tests` (i.e. `module tests:`) is an exclusive test context; the language semantics are defined in RFC 018.

### When to use inline tests vs `tests/` files

Both test contexts exist on purpose; they serve different goals.

Use **inline tests** (`module tests:`) when:

- you are writing **unit tests** for code in the same file/module
- tests are **fast and deterministic**
- the test’s helpers/imports should remain **test-only** and scoped

Use **test files under `tests/`** when:

- you are writing **integration/system tests** spanning multiple modules
- the tests rely on shared suite infrastructure (e.g. DB, network, file layouts)
- you want to use shared fixtures via `tests/**/conftest.incn`

Guideline:

- Prefer inline tests for “local correctness”.
- Prefer `tests/` for “behavior of the system” and shared fixture setups.

### Discovery: “pytest-like”

Inside a test context, a function is a test if either:

- it is named `test_*`, or
- it is decorated with `@test` from the `testing` module

```incan

def add(a: int, b: int) -> int:
    return a + b

module tests:
    from testing import assert_eq, test
    
    # implicitly marked as a test because it is named `test_*`
    def test_by_name() -> None:
        assert_eq(add(1, 2), 3)

    @test  # explicitly marked as a test
    def any_name_is_ok() -> None:
        assert_eq(add(2, 2), 4)
```

### Fixtures (pytest-style)

Fixtures are functions decorated with `@fixture` and injected by parameter name:

```incan
module tests:
    from testing import fixture, assert_eq

    @fixture
    def base() -> int:
        return 40

    def test_uses_fixture(base: int) -> None:
        assert_eq(base + 2, 42)
```

### Parametrize

Parametrized tests expand into multiple cases:

```incan

def add(a: int, b: int) -> int:
    return a + b

module tests:
    from testing import parametrize, assert_eq, test

    @parametrize("a, b, expected", [
        (1, 2, 3),
        (0, 0, 0),
        (-1, 1, 0),
    ])
    def test_add(a: int, b: int, expected: int) -> None:
        assert_eq(add(a, b), expected)
```

### Running tests

```bash
# Run all tests under tests/
incan test tests/

# Run tests in a particular file
incan test tests/test_math.incn

# Run tests in source tree (includes inline test modules)
incan test src/

# Filter by keyword
incan test -k "add" src/

# Set a per-test timeout (default for all tests)
incan test --timeout 5s tests/

# Treat xfail tests as normal tests (run-xfail)
incan test --run-xfail tests/

# List collected tests without executing them
incan test --list tests/

# Machine-readable reports (CI)
incan test --format json tests/
incan test --junit reports/junit.xml tests/

# Output capture policy
incan test --nocapture tests/
```

### Additional runner examples

```bash
# Filter by marker expression (pytest-style)
incan test -m "slow and not flaky" tests/

# Fail collection on unknown markers (requires marker registry; see reference rules)
incan test --strict-markers tests/

# Run tests in parallel
incan test -j 8 tests/

# Shuffle collection order to catch order dependencies (seed for reproducibility)
incan test --shuffle --seed 12345 tests/

# Show slowest N tests
incan test --durations 10 tests/
```

### Parallelism and shared resources

When tests run with `--jobs > 1`, some tests can interfere with each other if they touch shared state (a database, external services, global singletons, etc.). Incan provides two stdlib-gated scheduling decorators:

- `@serial`: run this test *alone* (exclusive against the entire test suite)
- `@resource("name")`: run tests that share the same resource key exclusively, while allowing unrelated tests to run in parallel

Important:

- With the worker-process model, **session-scoped fixtures are per worker**, not global to the entire run. Use `--jobs 1` if you need a single shared session fixture.

Example:

```incan
module tests:
    from testing import resource, serial

    @resource("db")
    def test_migrate_schema() -> None:
        ...

    @resource("db")
    def test_user_repo_queries() -> None:
        ...

    @serial
    def test_uses_fixed_port_8080() -> None:
        ...
```

> Note: `@mark("db")` is *classification* for selection/reporting; it does not imply locking. Use `@resource("db")` when you need mutual exclusion.

### Markers (pytest-style) and marker selection

Beyond the built-in markers (`@skip`, `@xfail`, `@slow`), Incan supports user-defined markers and marker-based selection:

```incan
module tests:
    from testing import mark

    @mark("db")
    @mark("flaky")
    def test_query() -> None:
        ...
```

Run subsets of tests:

```bash
incan test -m "db and not flaky" tests/
```

### Default marks (file-level and subtree-level via `conftest.incn`)

Sometimes you want to classify a whole *group* of tests (e.g. everything under `tests/integrations/` is “integration”), without repeating `@mark("integration")` on every test. Incan supports default marks in the `tests/` test suite:

- **File-level**: apply to all tests collected from a single test file.
- **Subtree-level**: apply to all tests collected under a directory subtree, configured via `tests/**/conftest.incn`.

Syntax:

- `const TEST_MARKS = ["name1", "name2", ...]`

Example file structure:

- `tests/`
    - `integrations/`
        - `conftest.incn`
        - `test_some_things.incn`

`tests/integrations/conftest.incn`:

```incan title="tests/integrations/conftest.incn"
from testing import fixture

const TEST_MARKS = ["integration"]

@fixture(scope="session")
def base_url() -> str:
    return "http://localhost:8080"
```

`tests/integrations/test_some_things.incn`:

```incan title="tests/integrations/test_some_things.incn"
from testing import assert_eq

const TEST_MARKS = ["smoke"]

def test_ping(base_url: str) -> None:
    assert_eq(base_url, "http://localhost:8080")
```

Now:

- `incan test -m "integration" tests/` selects all integration tests
- `incan test -m "integration and smoke" tests/` selects the smoke subset inside integrations

Inline tests (`module tests:` in production files) may also use `const TEST_MARKS` inside the test module, but this is discouraged: keep inline tests simple and prefer per-test marks (or move richer test organization into `tests/`).

### Conditional skip/xfail

Incan supports conditional skipping/xfail for platform- and feature-gated tests:

```incan

module tests:
    from testing import skipif, xfailif, platform, feature
    
    @skipif(platform() == "windows", "path semantics differ")
    def test_paths() -> None:
        ...

    @xfailif(feature("new_parser") == false, "known bug #123")
    def test_known_bug() -> None:
        ...
```

### Parametrize with per-case marks

You can mark individual parameter cases (skip/xfail/etc.) via `case(...)`:

```incan
module tests:
    from testing import parametrize, case, skip, xfail

    @parametrize("x", [
        case(1),
        case(2, marks=[xfail("bug #123")]),
        case(3, marks=[skip("too slow")]),
    ])
    def test_x(x: int) -> None:
        assert x > 0
```

### Shared fixtures via `conftest.incn`

To avoid repeating fixtures across many test files, any `conftest.incn` under `tests/` is automatically loaded by the test runner and contributes fixtures to tests in that directory subtree.

Example:

```incan title="tests/conftest.incn"
from testing import fixture

@fixture(scope="session")
def base_url() -> str:
    return "http://localhost:8080"
```

```incan title="tests/test_api.incn"
from testing import assert_eq

def test_ping(base_url: str) -> None:
    assert_eq(base_url, "http://localhost:8080")
```

`conftest.incn` is only auto-discovered under `tests/` and only applies to tests located in that subtree. Inline tests in production source trees (e.g. `src/**` `module tests:` blocks) do not automatically receive `conftest` fixtures; share fixtures via explicit helper modules instead.

### Built-in fixtures

The test runner provides a small set of built-in fixtures:

- `tmp_path: Path`: a unique temporary directory per test (cleaned up automatically)
- `tmp_workdir: Path`: runs the test with the working directory set to a fresh temp directory (restored afterward)
- `env: TestEnv`: a helper for temporary environment variables (restored afterward)

Use `tmp_path` when you just need a scratch directory for files and don’t want to change process state. Use `tmp_workdir` when the code under test relies on relative paths; it switches the process working directory to a fresh temp directory and yields its path. Use `env` to temporarily set/unset environment variables for a test, with automatic restoration afterward.

Example:

```incan
module tests:
    from testing import fixture, tmp_workdir, tmp_path, env, TestEnv

    def test_uses_tmp_path(tmp_path: Path) -> None:
        let config = tmp_path / "config.json"
        # write/read config using an absolute path (error handling omitted in examples)
        config.write_text("{}")?
        assert config.exists()

    def test_relative_paths(tmp_workdir: Path) -> None:
        # current working directory is a fresh temp dir for this test
        Path("output.txt").write_text("ok")
        assert (tmp_workdir / "output.txt").exists()

    def test_env(env: TestEnv) -> None:
        env.set("MODE", "test")
        mode = env.get("MODE")
        assert mode is Some(v)
        assert v == "test"
```

### Timeouts

Timeouts may be configured globally via CLI and overridden per test with a decorator (see reference-level rules).

Warning:

- In the worker-process execution model, enforcing timeouts may terminate a worker process. In that case, fixture teardown is best-effort and may not run (this is called out explicitly in the reference-level timeout semantics).

## Reference-level explanation (precise rules)

### Collection-time evaluatability (“const-evaluable”) (reference rules)

Several runner features require evaluating expressions at collection time (before executing tests): `skipif`/`xfailif` conditions, `@parametrize` data (and ids), `TEST_MARKS`, and marker registries.

In this RFC, “collection-time evaluatable” means “const-evaluable”, using the same rules as `const` initializers:

- allowed: literals, tuples/lists/dicts/sets of const-evaluable values, simple operators, and references to other consts
- disallowed: function/method calls, IO, randomness, and anything requiring running the program

Static constructors (explicit exceptions):

- The following call-like forms are treated as **compile-time data constructors** and are allowed at collection time,
as long as their arguments are themselves collection-time evaluatable:
    - `case(...)`
    - `mark("name")`
    - `skip("reason")`
    - `xfail("reason")`
    - `slow`
    - `resource("name")`
    - `serial`

Guidance:

- Keep `@parametrize` data simple and const-evaluable (literals, tuples, lists). Use fixtures or build values inside the test body when you need `Path`, UUIDs, or domain objects.
- Example: pass a filename as `str` and construct a `Path` inside the test.

```incan
@parametrize("name", ["config.json", "defaults.json"])
def test_config(tmp_path: Path, name: str) -> None:
    let path = tmp_path / name
    assert path.exists() == False
```

If an expression required to be const-evaluable does not meet this, a **test collection error** (`TestCollectionError`) will be raised.

Evaluation responsibility:

- Collection-time expressions are evaluated by the **test runner** during test collection (not by the compiler at compile time). This allows collection-time probes like `platform()` and `feature(name)` to inspect the runtime environment while still requiring the expression structure to be const-evaluable.
- The compiler validates that the expression is structurally const-evaluable; the runner evaluates it at collection time.

### Error categories (reference rules)

The test runner distinguishes the following error categories:

- **Test collection error** (`TestCollectionError`): the runner cannot finish collecting a valid, executable test suite.
    - Examples:
        - non-const-evaluable `@parametrize` data / `ids`
        - non-evaluatable `skipif` / `xfailif` conditions
        - missing fixture for a required parameter
        - fixture dependency cycle
        - duplicate fixture definition at the same precedence level
        - unknown marker under `--strict-markers` (including in `TEST_MARKS`, `mark("...")`, or `-m` expressions)
    - Behavior:
        - abort test execution (tests are not run)
        - exit with failure
        - report the error with as much location context as possible (file/span, and the relevant declaration/decorator)

- **Test execution failure**: a collected test case ran and failed.
    - Examples: assertion failure, uncaught runtime error/panic, timeout.
    - Behavior: record as a failed test case and continue unless `-x` is set.

- **Test skipped / expected-failure outcomes**: a collected test case is skipped or treated specially by marker semantics.
    - Examples: `skip`, `xfail`, `skipif`, `xfailif`.

### Testing-gated resolution

The runner must only recognize testing constructs when they resolve to the `testing` module (imports and aliases) as specified in RFC 018.

### Test contexts

Test discovery operates on a set of `.incn` files selected by the CLI path argument.

A file contributes tests/fixtures if it contains one or more **test contexts**:

1. **Test file context**: the whole file is a test context if the filename matches:
   - `test_*.incn`, or
   - `*_test.incn`
2. **Inline test module context**: a `module tests:` block in the file is a test context.

A file may contain at most one `module tests:` block (enforced by the compiler; see RFC 018).

Restriction:

- A file that is a **test file context** (by name) must not also contain `module tests:` (restriction defined in RFC 018). This avoids a redundant/ambiguous “test context inside a test context” model and keeps the mental model simple.

Rationale: a single inline test block keeps stripping rules, tooling, and scoping simple. This restriction may be relaxed in a future RFC once the feature is implemented and exercised in real codebases.

### What is a test?

Within a test context, a function is a test if:

- its name matches `test_*`, OR
- it is decorated with `testing.test`

Markers and parametrization apply to tests as defined in their respective sections.

Test success criteria:

- A test is successful if it completes without an uncaught runtime error/panic or timeout.
- Test functions should return `None`; any return value is ignored by the runner.

### What is a fixture?

Within a test context, a function is a fixture if it is decorated with `testing.fixture`.

Fixture dependency injection matches test/fixture parameters to fixture names.

### Fixture injection errors (reference rules)

Fixture resolution happens at collection time.

Rules:

- If a test parameter has no matching fixture (including built-in fixtures), it is a **test collection error** (`TestCollectionError`).
- If fixture dependencies contain a cycle, it is a **test collection error** (`TestCollectionError`).
- If multiple fixtures with the same name are visible at the same precedence level (e.g. duplicate definitions in the same file), it is a **test collection error** (`TestCollectionError`).

### Fixture scopes

`@fixture(scope="function"|"module"|"session", autouse=true|false)`:

- **function**: created/teardown per test case
- **module**: shared across all tests collected from the same source file
- **session**: shared across the entire `incan test` run (note: under worker-process parallelism, session scope is per worker process unless a global coordinator is introduced)

### Autouse fixtures (reference rules)

An autouse fixture (`@fixture(..., autouse=true)`) runs automatically without being listed as a parameter.

Where autouse applies:

- In a **test file context**, autouse fixtures defined in that file apply to all tests in that file.
- In an **inline `module tests:` context**, autouse fixtures defined in the test module apply to all tests in that test module.
- In `tests/**/conftest.incn`, autouse fixtures apply to all tests collected from files in that conftest’s directory subtree (subject to precedence rules already defined for conftest).

Ordering and dependencies:

- Autouse fixtures may depend on other fixtures via parameters.
- For a given test case, the runner constructs the full fixture dependency graph (explicit + autouse) and executes setup in dependency order (topological order).
- If multiple fixtures are otherwise independent, ordering must be deterministic. The tie-breaker must be fixture name (lexicographic).

Scope interaction:

- Autouse fixtures follow normal scope rules:
    - function-scoped autouse fixtures run once per test case
    - module-scoped autouse fixtures run once per source file
    - session-scoped autouse fixtures run once per test run (with `--jobs > 1` this means once per worker process)

Failure behavior:

- If autouse fixture setup fails for a test case, the test is reported as failed due to fixture setup error (the test body does not run). Teardown for already-created fixtures is best-effort.

### Fixture teardown via `yield` (reference rules)

A fixture may optionally provide teardown logic using a `yield` expression, following Python/pytest's model.

Design note (Python alignment):

- A fixture that uses `yield` is a **generator function**. This is the same semantics as Python: `yield` in a function body makes it a generator.
- The test runner detects generator-based fixtures and consumes them: it calls the generator to get the yielded value (setup), injects that value into tests, and then resumes/exhausts the generator after the test (teardown).
- This means `yield` has one meaning in the language (generator), and the runner uses that mechanism for setup/teardown.

Form:

- The fixture function body contains exactly one `yield` statement.
- Either:
    - `yield <expr>` (yields a value), OR
    - `yield` (yields the unit value; only valid for fixtures declared as `-> None`)
- The yielded value is the fixture value injected into tests/fixtures that depend on it.
- Statements **after** the `yield` are teardown logic.

Rules:

- A fixture must execute `yield` exactly once. If `yield` executes zero times or more than once, it is a runtime error.
- Teardown must run even if the test fails (best-effort; failures in teardown are reported).
- Teardown timing is based on fixture scope:
    - **function**: teardown runs after each test case that used the fixture
    - **module**: teardown runs after all tests from that source file finish
    - **session**: teardown runs at the end of the full test run

Errors:

- If teardown fails, the test run is considered failed (report as an error tied to the tests that used the fixture; for module/session scope, report at the end of the run).

### Keyword selection (`-k`) (reference rules)

CLI:

- `-k <substr>`: include only tests whose identifier contains `<substr>` as a substring.

Rules:

- Matching is performed against the test’s stable identifier (see “Stable test identifiers” below).
- The match is case-sensitive for now (future extensions may add richer expressions; this RFC only requires substring matching).

Examples:

- `incan test -k add tests/` selects tests whose id contains `add`

### Fail-fast (`-x` / `--exitfirst`) (reference rules)

CLI:

- `-x` / `--exitfirst`: stop the run after the first failure.

Rules:

- “Failure” means any result that would make the overall run fail (e.g. `FAILED`, `XPASS`, fixture setup error, teardown error).
- `XFAIL` does not count as a failure for `-x` (it is an expected failure).
- In sequential runs (`--jobs 1`), the runner stops executing further tests immediately after the first failure is recorded.
- With worker processes (`--jobs > 1`):
    - the runner stops scheduling new tests after the first failure is recorded
    - in-flight tests may complete and be reported (implementations may choose to terminate workers early, but should
      prefer graceful shutdown to preserve fixture teardown)

### Markers and selection (`-m`)

#### Marker model

Markers are labels attached to tests (and to individual parameter cases). They are used for:

- selection (`incan test -m ...`)
- conditional behavior (`skipif` / `xfailif`)
- reporting

The following marker decorators are recognized when they resolve to the `testing` module:

- `@skip(reason: str = "")`
- `@xfail(reason: str = "")`
- `@slow`
- `@mark(name: str)` (user-defined markers)

Default marks:

- A `const TEST_MARKS: List[str]` binding (in a test file or in a `module tests:` context) adds default marks to all tests collected from that context.
- A `const TEST_MARKS: List[str]` binding in `tests/**/conftest.incn` adds default marks to all tests collected from files in that conftest’s directory subtree.

Unknown markers:

- By default, unknown markers are allowed and recorded.
- With `--strict-markers`, unknown markers are a **test collection error** (`TestCollectionError`).

Marker registration (required for `--strict-markers`):

- Marker names are considered “known” if they are:
    - built-in markers (`skip`, `xfail`, `slow`), OR
    - declared in a `const TEST_MARKERS: List[str]` registry visible to the test.

Rules:

- `TEST_MARKERS` may appear in:
    - a test file, or
    - an inline `module tests:`, or
    - `tests/**/conftest.incn` (applies to its directory subtree).
- All applicable `TEST_MARKERS` values from conftest files on the path to a test file are merged (union), outer-to-inner.
- The test file’s own `TEST_MARKERS` (if present) is also merged.
- With `--strict-markers`:
    - every `mark("name")` must use a name present in `TEST_MARKERS`
    - every name in `TEST_MARKS` must be present in `TEST_MARKERS`
    - every marker name referenced in `-m "<expr>"` must be present in `TEST_MARKERS`

Terminology:

- `TEST_MARKS` = default marks applied to tests
- `TEST_MARKERS` = registry of **allowed marker names** for strict marker validation

Restrictions:

- `TEST_MARKERS` must be collection-time evaluatable.
- Marker names must match `^[a-z][a-z0-9_]*$` (snake_case). If this is violated, it is a **test collection error**
(`TestCollectionError`).

#### Marker selection expression

`incan test -m <expr>` filters collected tests by marker expression, where `<expr>` supports:

- marker names (strings)
- `and`, `or`, `not`
- parentheses for grouping

Operator precedence:

- `not` > `and` > `or`
- all operators are left-associative; parentheses override precedence

Examples:

- `-m "slow"`
- `-m "db and not flaky"`

#### Slow tests (`@slow` and `--slow`)

By default, tests marked `@slow` are excluded from collection.

CLI:

- `--slow`: include slow tests in collection.

Interaction with `-m`:

- If `--slow` is not set, `@slow` tests are excluded even if they would otherwise match `-m` (use `--slow` explicitly to  opt in).

### Parallel execution and resource locking (reference rules)

#### CLI parallelism

CLI:

- `--jobs <n>` / `-j <n>`: maximum number of tests to execute concurrently.

Rules:

- The default is implementation-defined, but must be documented and stable (recommended: number of logical CPUs).
- `--jobs 1` forces sequential execution.

Execution model (important for correctness):

- Parallelism in this RFC refers to **multiple worker processes** (xdist-style), not concurrent execution within a single process. Each worker executes **one test case at a time**.
- This keeps process-global state changes (e.g. current working directory, environment variables) isolated per worker and avoids flakiness from thread-level shared state.
- Session-scoped fixtures under worker processes are a common source of surprises. In this RFC:
    - Session-scoped fixtures are **per worker process** (i.e., created once per worker). This is simple, deterministic, and avoids cross-process coordination.
    - A future extension may introduce a coordinator for truly global session fixtures (once per overall run).

#### Scheduling decorators

The following decorators are recognized when they resolve to the `testing` module:

- `@resource(name: str)`
- `@serial`

Rules:

- A test case may declare zero or more resources.
- Two test cases must not execute concurrently if they share any declared resource key.
- `@serial` is equivalent to `@resource("__serial__")` and additionally conflicts with *all* other tests (i.e., it runs alone).
- Resource locks apply to expanded parametrized cases as well (each case is scheduled independently but inherits the same declared resources from the test function and/or per-case marks).
- Lock acquisition order: when a test declares multiple resources, the scheduler MUST acquire locks in lexicographic order of resource key. This prevents deadlocks and remains valid if future implementations allow >1 concurrent test per worker.

Non-goal:

- Marks (e.g. `@mark("db")`) do not imply resources. Locking is explicit.

### Output, reporting, and runner ergonomics (reference rules)

### Stable test identifiers

The test runner must assign a stable identifier (“test id”) to each collected test case. This identifier is used by `--list`, `-k`, and machine-readable reports.

Format (conceptual):

- `<relative_path>::<context>::<test_name>[<case_id>]`

Rules:

- `<relative_path>` is the path relative to the **stable id root**, determined as follows (in order):
    1. If `incan test` is invoked with an explicit directory argument, that directory is the stable id root.
    2. Otherwise, if a project root exists (detected via `incan.toml` per RFC 015), the project root is the stable id root.
    3. Otherwise, the current working directory is the stable id root.
- `<context>` is:
    - `file` for test-file context (the entire file is a test context), OR
    - `tests` for inline `module tests:` context
- `<case_id>` is present only for parametrized expansions (stable index order, e.g. `[0]`, `[1]`, ...).

Examples:

- `tests/test_math.incn::file::test_addition`
- `src/math.incn::tests::test_addition`
- `tests/test_math.incn::file::test_add[2]`

#### Output capture

CLI:

- `--nocapture`: stream test output live.

Default behavior:

- By default, the runner captures output and prints it for failed tests (and optionally on verbose mode).

#### Listing

CLI:

- `--list`: list collected tests (after applying collection-time filters like `-k` / `-m`) and exit with success.

#### Shuffling

CLI:

- `--shuffle`: randomize test execution order.
- `--seed <n>`: set the shuffle seed for reproducibility.

Rules:

- Without `--shuffle`, the order is deterministic and stable.
- With `--shuffle`, the runner must print the seed it used (explicit or generated).

#### Durations

CLI:

- `--durations <n>`: print the slowest N tests at the end of the run (by wall-clock time).

#### Machine-readable reports

CLI:

- `--format json`: emit one JSON record per test result (stable schema) suitable for CI tooling.
- `--junit <path>`: write a JUnit XML report to `<path>`.

Minimum JSON schema (one record per test result):

- `schema_version: "incan.test.v1"`
- `id: str` (stable test id)
- `outcome: "passed" | "failed" | "skipped" | "xfailed" | "xpassed"`
- `duration_ms: number`
- `file: str`
- `name: str`
- `case_id: str | null`
- `parameters: object` (optional; name → rendered value)
- `markers: list[str]` (optional)
- `message: str` (optional failure/skip reason)

Outcome casing:

- JSON outcomes are lower-case as listed above.
- Console output may use upper-case labels for readability; this is purely presentation.

Minimum reporting guarantees for parametrized tests:

- Console output and machine-readable reports must include the stable test id (including case id, if parametrized).
- For parametrized failures, the runner must include the bound parameter values in the failure output:
    - Console: show a `name=value` list (at minimum for the failing case).
    - JSON: include a `parameters` object mapping parameter names to their rendered values.
    - JUnit: encode parameter values in the test name (via case id) and/or include them in the failure message text.

### Conditional skip/xfail (reference rules)

The following decorators are recognized when they resolve to the `testing` module:

- `@skipif(condition: bool, reason: str = "")`
- `@xfailif(condition: bool, reason: str = "")`

Rules:

- The condition is evaluated at collection time using the same “const-evaluable subset” used for other compile-time
evaluation, plus a small set of explicit `testing` probes (see below).
- If the condition is not evaluatable at collection time, it is a **test collection error** (`TestCollectionError`).

#### Collection-time probes (testing)

The following `testing` functions are intended for use in `skipif/xfailif` conditions and must be supported in the collection-time evaluatable subset:

- `platform() -> str`
    - Returns a stable platform identifier string (minimum set: `"linux"`, `"macos"`, `"windows"`).
- `feature(name: str) -> bool`
    - Returns whether a named test feature flag is enabled for this `incan test` run.
    - If the feature is not enabled, returns `false` (default).

Feature enabling (CLI):

- `--feature <name>` enables a named feature for `feature(name)` checks.
- The flag may be provided multiple times to enable multiple features.

### Parametrization (reference rules)

`@parametrize(argnames: str, argvalues: List[Tuple|case(...)], ids: List[str] | None = None)` expands a single test function into multiple test cases.

Collection-time evaluatability:

- `argvalues` must be collection-time evaluatable:
    - literals, tuples, lists, dicts of literals (const-evaluable)
    - `case(...)` values with const-evaluable payloads and marks
- If `argvalues` is not evaluatable at collection time, it is a **test collection error** (`TestCollectionError`).

Deliberate tightening vs earlier drafts:

- Complex runtime expressions in `argvalues` (e.g. calling constructors, IO, random) are intentionally disallowed in this RFC because they undermine deterministic collection and stable test ids. Use fixtures to build complex objects from const inputs, or construct values inside the test body.

Case identifiers:

- Expanded tests must get stable case ids.
- If `ids` is provided:
    - it must be collection-time evaluatable
    - its length must equal the number of generated cases
    - each id must be a `str`
    - ids must be unique within a single parametrized test function (duplicates are a **test collection error** (`TestCollectionError`))
    - id format constraints:
        - ids must match the regex `^[A-Za-z0-9][A-Za-z0-9_.]*$`
        - ids must not contain `[` or `]` (reserved for stable id formatting)
        - ids must not contain `-` (reserved for composing stacked parametrization case ids)
    - ids are used as the case id in the stable test identifier (e.g. `test_add[lowercase]`)
- Otherwise, stable numeric indices are used in the stable index order in `argvalues` (e.g. `[0]`, `[1]`, ...).

Multiple parametrization (cartesian product):

- A test may have multiple `@parametrize` decorators.
- The effective set of test cases is the cartesian product of the parameter sets.
- Expansion order is deterministic and follows source order of the decorators (top to bottom).
- Case id composition:
    - If any `@parametrize` provides `ids`, the composite case id joins components with `-` in decorator order (e.g. `[lowercase-utf8]`).
    - Otherwise, join numeric indices with `-` (e.g. `[0-2]`).

Empty parameter list:

- If `argvalues` is empty (or the cartesian product is empty), no tests are generated for that parametrized function and the runner should emit a warning.

Errors:

- If the number of values in a case tuple does not match `argnames`, it is a **test collection error** (`TestCollectionError`).
- If values do not type-check against the test function’s parameter types, it is a **test collection error** (`TestCollectionError`).

### Parametrize per-case marks

`@parametrize` supports per-case marks via a `case(...)` helper from the `testing` module:

```incan
@parametrize("x", [
    case(1),
    case(2, marks=[xfail("bug #123")]),
    case(3, marks=[skip("too slow")]),
])
```

Allowed per-case marks (in `case(..., marks=[...])`):

- `skip("reason")`
- `xfail("reason")`
- `slow`
- `mark("name")`
- `resource("name")`
- `serial`

Resources declared per-case are merged with decorator-level resources for scheduling.

### Shared fixtures via `conftest.incn` (reference rules)

Any file named `conftest.incn` under `tests/` is discovered automatically and may contribute fixtures to tests in its directory subtree.

Recognized constructs in `conftest.incn`:

- `@fixture`-decorated functions (fixture definitions)
- `const TEST_MARKS: List[str]` (default marks for the subtree)
- `const TEST_MARKERS: List[str]` (marker registry for strict mode)
- imports required by the above

Arbitrary top-level code in `conftest.incn` is **not executed** at collection time. Unlike pytest's conftest.py (which executes as a normal Python module), `conftest.incn` is parsed declaratively for fixture and marker definitions only. This avoids import-time side effects and keeps collection deterministic.

Resolution:

- Fixtures defined in the same file as the test take precedence over fixtures from `conftest.incn`.
- If multiple `conftest.incn` files define the same fixture name, the nearest one (deepest directory) wins.
- Ambiguous duplicates at the same directory level are a **test collection error** (`TestCollectionError`).

Default marks (`TEST_MARKS`):

- `conftest.incn` may define `const TEST_MARKS: List[str]`.
- All applicable `TEST_MARKS` values from conftest files on the path to a test file are **merged** (union), from
outer-to-inner directories.
- The test file’s own `TEST_MARKS` (if present) is also merged.
- Per-test and per-case marks are merged on top.

This provides a simple way to classify whole subtrees (e.g. all tests under `tests/integrations/` are “integration”).

### Built-in fixtures (reference rules)

The test runner provides built-in fixtures (names reserved in the fixture namespace):

- `tmp_path: Path`
    - Function-scoped by default (unique per test case)
    - Cleaned up automatically after the test
- `tmp_workdir: Path`
    - Sets the process current working directory for the duration of the test (restored afterward)
- `env: TestEnv`
    - A helper for temporary environment variables (restored afterward)
    - API:
        - `set(key: str, value: str) -> None`
        - `unset(key: str) -> None`
        - `get(key: str) -> Option[str]`

Concurrency note:

- Because `tmp_workdir` and `env` affect process-global state, they are only safe if tests do not run concurrently in the same process. This RFC’s `--jobs` execution model uses worker processes; if an implementation deviates (e.g. thread-based parallelism), it must add implicit global locking (treat `tmp_workdir`/`env` as `@serial`) or disallow these fixtures under parallel execution.

### Timeouts (reference rules)

CLI:

- `--timeout <duration>` sets the default per-test timeout (e.g. `5s`, `250ms`)

Per-test override:

- `@timeout(duration)` overrides the default timeout for a specific test.

Timeout behavior:

- On timeout, the test case is recorded as failed with a timeout reason.
- If a test is executed in its own worker process (the default `--jobs` model), an implementation may terminate the worker process to enforce the timeout. In that case, fixture teardown is best-effort and may not run.
    - This is acceptable for now; stronger teardown guarantees under timeout may be specified in a future RFC.

### XFail policy switches

CLI:

- `--run-xfail`: treat xfail tests as normal tests (do not convert failures into “expected failures”)

Rules:

- Default behavior (without `--run-xfail`):
    - A failing xfail test is recorded as **XFAIL** and does not fail the test run.
    - A passing xfail test is recorded as **XPASS** and **fails** the test run (it indicates the expectation is outdated).
- With `--run-xfail`:
    - xfail markers are ignored for pass/fail semantics; xfail tests behave like normal tests.

### Collection order (deterministic)

For each file:

1. Parse file and build a `testing` resolution map (imports/aliases).
2. Discover and load `tests/**/conftest.incn` fixture providers.
3. Collect inline test contexts (`module tests:`) and/or treat file as a test context if it is a test file.
4. Collect fixtures in that file’s test contexts (including conftest fixtures).
5. Collect tests in that file’s test contexts.
6. Expand parametrized tests into per-case tests (including per-case marks).
7. Apply selection/filters (`-k`, `-m`, `--slow`, `skip`, `skipif`, `xfail`, `xfailif`).
8. Execute tests with fixture injection, lifecycle, and timeouts.

## Design details

### Async testing (out of scope)

Async testing is **not in scope** in this RFC.

This RFC does not specify specific async fixture/test execution semantics.

Future direction:

- Async fixtures and async tests should compose with the same discovery model.
- See RFC 004 for Tokio integration; followup RFCs must specify runtime selection, timeout/cancellation interaction, and
teardown guarantees.

## Compatibility / migration

- Existing name-based discovery (`test_*` functions in `test_*.incn` files) remains valid.
- Inline test discovery depends on the language-level `module tests:` block (RFC 018).
- New runner features are additive, but change defaults for capture and xfail policy as specified.

CLI reference reconciliation:

- This RFC supersedes the `incan test` section of the CLI reference (`docs/tooling/reference/cli_reference.md`) for all flags and behaviors specified here.
- Flags in the current CLI reference that are not mentioned in this RFC (e.g. `--fail-on-empty`, `-v`) remain valid unless explicitly contradicted.
- Once this RFC is implemented, the CLI reference must be updated to reflect the authoritative surface defined here.

## Alternatives considered

- **Top-level `@test` next to production functions**: rejected; it pollutes the production namespace and makes it hard to keep test-only imports/helpers contained.
- **Doctest-only**: useful but insufficient for fixtures/parametrize/markers.
- **Magic language keywords for tests/fixtures**: rejected; harms tooling and contradicts the “stdlib-gated” principle.

Out of scope (for now):

- `capsys` / `caplog`-style capture fixtures (use explicit APIs; revisit later)
- monkeypatch-style runtime patching (prefer dependency injection and test doubles)
- a pytest-style plugin ecosystem (revisit once core semantics are stable)

## Appendix: testing surface inventory (informative)

This appendix is a contributor-oriented inventory of the testing surface **after this RFC is implemented**, with an informative snapshot of what exists **today** (at time of writing). It is **not normative**; the spec sections above are authoritative.

Legend:

- **Today**: implementation status in the current repository at RFC creation time
    - **Yes**: implemented
    - **Partial**: some pieces exist, but not the full RFC behavior
    - **No**: not implemented
- **After RFC 019**: whether this RFC introduces it (**New**), modifies semantics (**Changed**), or leaves it (**Unchanged**)

> Note: this table should be used as a checkmark toward implementation completeness when this RFC is implemented.

### Test runner + CLI surface

| Item                                                   | Today              | After RFC 019 | Notes                                             | Implemented |
| ------------------------------------------------------ | ------------------ | ------------- | ------------------------------------------------- | ----------- |
| Test file discovery (`test_*.incn`, `*_test.incn`)     | Yes                | Unchanged     | Test file context                                 |             |
| Inline test discovery (`module tests:`)                | No                 | New           | Inline test context                               |             |
| Test discovery by name (`test_*`)                      | Yes                | Unchanged     | In test contexts                                  |             |
| Test discovery by decorator (`@test`)                  | No                 | New           | In test contexts                                  |             |
| Fixtures from same file                                | Partial            | Changed       | Injection + lifetimes + errors                    |             |
| `tests/**/conftest.incn` fixtures                      | No                 | New           | Auto-discovery + precedence rules                 |             |
| Built-in fixtures (`tmp_path`, `tmp_workdir`, `env`)   | No                 | New           | Runner-provided                                   |             |
| `-k <substr>`                                          | Yes (fn-name)      | Changed       | RFC matches stable test id                        |             |
| `-m <expr>`                                            | No                 | New           | Marker expression selection                       |             |
| `--strict-markers`                                     | No                 | New           | Unknown marker is a collection-time error         |             |
| `@slow` excluded by default + `--slow` opt-in          | Yes (basic)        | Unchanged     | RFC clarifies interaction with `-m`               |             |
| `@xfail` / XPASS policy                                | Yes (basic)        | Changed       | XPASS fails; adds `--run-xfail`                   |             |
| `--run-xfail`                                          | No                 | New           | Ignore xfail semantics                            |             |
| `--timeout <duration>` + `@timeout(...)`               | No                 | New           | Timeouts (default + override)                     |             |
| `--jobs/-j <n>`                                        | No                 | New           | Parallel execution limit (worker processes)       |             |
| `--shuffle` / `--seed <n>`                             | No                 | New           | Reproducible order randomization                  |             |
| `--durations <n>`                                      | No                 | New           | Report slowest N tests                            |             |
| `--list`                                               | No                 | New           | List tests after filters; do not execute          |             |
| `--format json`                                        | No                 | New           | One JSON record per test result                   |             |
| `--junit <path>`                                       | No                 | New           | JUnit XML report                                  |             |
| `--nocapture`                                          | Yes (default today)| Changed       | RFC makes capture default; `--nocapture` opts out |             |
| Stable test id                                         | No                 | New           | Used by `--list`, `-k`, JSON, JUnit               |             |
| `--feature <name>`                                     | No                 | New           | Enables `testing.feature(name)` probes            |             |

## Layers affected

- **CLI (`incan test`)** — the test runner is the primary deliverable of this RFC. It must implement test discovery (both `tests/` files and inline `module tests:` contexts), fixture graph resolution, parametrization expansion, marker evaluation, and parallel/serial scheduling with resource locking.
- **`conftest.incn` support** — the compiler and test runner must support shared fixture files discovered at directory boundaries, with precedence rules for fixture shadowing.
- **Typechecker** — must validate fixture injection signatures, detect unresolvable fixture dependencies at collection time, and surface them as `TestCollectionError` rather than runtime failures.
- **Stdlib (`std.testing`)** — `@fixture`, `@parametrize`, `@skip`, `@xfail`, `@timeout`, `@serial`, `@resource`, and the `TEST_MARKS` registry must be defined and consumable by the runner as specified in this RFC.
- **Build system** — `incan test` must compile test modules against the project source root, enabling `from mymodule import ...` in test files without duplication; it must strip `module tests:` bodies from production builds.
- **Reporting layer** — the runner must support `--format json`, `--junit <path>`, `--list`, `--durations`, and `--shuffle` as specified; JSON reports must include a stable `schema_version` field.

## Implementation architecture

Implement incrementally:

1. Collection pipeline: discovery (test files + `module tests:` contexts) and stable test ids.
2. Fixture resolution and lifecycle (scopes, autouse, teardown via `yield`).
3. Parametrization expansion, case ids, and per-case marks.
4. Marker model + selection (`-m`, strict markers, skip/xfail policies).
5. `tests/**/conftest.incn` discovery and precedence.
6. Execution model: `--jobs`, resource locking, timeouts.
7. Reporting: list/durations/shuffle, JSON/JUnit, output capture.

### Implementation dependencies (informative)

This section is informative (non-normative). It exists to help contributors implement the RFC in a sensible dependency order.

Suggested dependency order:

- Discovery + stable test identifiers
- Fixture graph + `conftest.incn` precedence
- Parametrization expansion + per-case marks
- Marker evaluation + selection (`-m`, `--strict-markers`)
- Parallel execution + resource locking
- Timeouts + reporting surfaces (JSON/JUnit, durations, list, shuffle)

### Conformance tests

Turn the guide-level examples into real tests:

- [ ] stable test id formatting and `-k` filtering
- [ ] `testing.test` vs name-based discovery (`test_*`) in both test files and inline modules
- [ ] fixture resolution errors become `TestCollectionError`
- [ ] `conftest.incn` fixtures resolve with correct precedence and scoping
- [ ] autouse fixtures apply per scope and respect dependency ordering
- [ ] parametrization ids and cartesian product ordering are stable
- [ ] per-case marks in `parametrize` skip/xfail individual cases correctly
- [ ] marker selection: `-m "db and not flaky"` filters tests correctly
- [ ] `--strict-markers` rejects unknown markers in `TEST_MARKS`, `mark(...)`, and `-m`
- [ ] `skipif` / `xfailif` conditions are evaluated at collection time and behave as specified
- [ ] parallel scheduling: `--jobs 2` runs independent tests concurrently but respects `@resource("db")`
- [ ] serial scheduling: `@serial` forces exclusive execution
- [ ] shuffle reproducibility: `--shuffle --seed 123` produces stable randomized order
- [ ] list mode: `--list` prints collected tests and exits without running them
- [ ] built-in fixtures exist (`tmp_path`, `tmp_workdir`, `env`) and are scoped/cleaned up correctly
- [ ] timeouts: `--timeout` default + `@timeout` override (teardown best-effort)
- [ ] `--run-xfail` policy switch changes xfail behavior as specified
- [ ] durations: `--durations 10` prints slowest tests with correct ids
- [ ] reports: `--format json` and `--junit <path>` emit stable machine-readable output
- [ ] JSON reports include `schema_version: "incan.test.v1"`

## Design Decisions

- RFC 019 owns runner and CLI behavior, while RFC 018 owns language-level testing constructs.
- Test discovery includes both dedicated test files and inline `module tests:` contexts.
- Fixture injection, parametrization, markers, timeouts, reporting, and scheduling are part of the core runner contract rather than repo-local convention.
- Parallel execution uses worker-process isolation rather than shared in-process concurrency by default.

## References

- RFC 018: Language Primitives for Testing
- pytest good practices (discovery): `https://docs.pytest.org/en/stable/goodpractices.html#test-discovery`
- pytest fixtures: `https://docs.pytest.org/en/stable/explanation/fixtures.html`
- pytest parametrize: `https://docs.pytest.org/en/stable/how-to/parametrize.html`

<!-- The "Design decisions" section (if present) was renamed from "Unresolved questions" once all open questions were resolved. If new unresolved questions arise during implementation, add an "Unresolved questions" section and move resolved items to "Design decisions" after resolution. -->
