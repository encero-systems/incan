# RFC 001: Test Fixtures

- **Status:** Superseded
- **Superseded by:** RFC 018, RFC 019
- **Created:** 2024-12-07
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 018 (language primitives for testing)
    - RFC 019 (test runner, CLI, and ecosystem)
- **Issue:** —
- **RFC PR:** [#54](https://github.com/dannys-code-corner/incan/pull/54)
- **Written against:** v0.1
- **Shipped in:** —

## Summary

Add pytest-style fixtures to Incan's test framework via the `@fixture` decorator.

## Motivation

Fixtures provide:

1. **Setup/teardown** - Automatic resource management for tests
2. **Dependency injection** - Tests declare what they need by parameter name
3. **Reusability** - Share setup code across multiple tests
4. **Composability** - Fixtures can depend on other fixtures

## Design

### Basic Syntax

```incan
from testing import fixture

@fixture
def database() -> Database:
    """Fixture that provides a test database."""
    db = Database.connect("test.db")
    yield db          # Test runs here
    db.close()        # Teardown after test

def test_insert(database: Database) -> None:
    database.insert("key", "value")
    assert_eq(database.get("key"), "value")
```

### Fixture Scopes

```incan
@fixture(scope="function")  # Default: new instance per test
def temp_file() -> str:
    ...

@fixture(scope="module")    # Shared across all tests in file
def shared_client() -> Client:
    ...

@fixture(scope="session")   # Shared across entire test run
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
    # database fixture (and its config dependency) injected
    result = database.query("SELECT 1")
    assert_eq(result, 1)
```

### Autouse Fixtures

```incan
@fixture(autouse=true)
def setup_logging() -> None:
    """Automatically applied to all tests in this file."""
    logging.set_level("DEBUG")
    yield
    logging.set_level("INFO")
```

## Implementation

### Phase 1: Parser Changes

Add `yield` expression support for generators/fixtures:

```bash
Expr::Yield(Option<Box<Spanned<Expr>>>)
```

Parse `@fixture` decorator with optional arguments:

- `scope: str` - "function" | "module" | "session"
- `autouse: bool` - Whether to auto-apply

### Phase 2: Test Runner Changes

1. **Discovery**: Scan for `@fixture` decorated functions
2. **Dependency Graph**: Build fixture dependency tree
3. **Injection**: Match test parameters to fixtures by name
4. **Lifecycle**: Create/teardown fixtures per scope

### Phase 3: Codegen

Generate Rust code that:

1. Creates fixture instances before test
2. Passes fixtures as arguments
3. Runs teardown after test (even on panic)

Example generated Rust:

```rust
#[test]
fn test_insert() {
    // Setup
    let database = {
        let db = Database::connect("test.db");
        db
    };
    
    // Test body
    let result = std::panic::catch_unwind(|| {
        database.insert("key", "value");
        assert_eq!(database.get("key"), "value");
    });
    
    // Teardown (always runs)
    database.close();
    
    // Re-raise panic if test failed
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
```

## Alternatives Considered

### 1. Setup/Teardown Methods

```incan
class TestDatabase:
    def setup(self) -> None:
        self.db = Database.connect("test.db")
    
    def teardown(self) -> None:
        self.db.close()
    
    def test_insert(self) -> None:
        self.db.insert("key", "value")
```

**Rejected**: Less flexible than fixtures, doesn't support dependency injection.

### 2. Context Managers

```incan
def test_insert() -> None:
    with Database.connect("test.db") as db:
        db.insert("key", "value")
```

**Rejected**: Requires boilerplate in each test, no reusability.

## Open Questions

1. **Generator syntax**: Should we use `yield` or a different keyword?
2. **Async fixtures**: How to handle `async def` fixtures with Tokio?
3. **Fixture params**: Should fixtures support `@parametrize`?

## Checklist

- [ ] Parser: `yield` expression support in fixtures
- [ ] Parser: `@fixture` decorator args (scope, autouse)
- [ ] Runner: fixture discovery and dependency graph
- [ ] Runner: scoped lifecycle (function/module/session)
- [ ] Injection: match test parameters to fixtures by name
- [ ] Teardown: always run (even on panic)
- [ ] Autouse fixtures honored
- [ ] Async fixtures design (Tokio) — pending
- [ ] Docs/examples updated once implemented

## References

- [pytest fixtures documentation](https://docs.pytest.org/en/stable/explanation/fixtures.html)
- [Rust test setup patterns](https://doc.rust-lang.org/book/ch11-03-test-organization.html)
