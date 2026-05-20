# RFC 007: Inline Tests

**Status:** Planned **Created:** 2024-12-10 **Issue:** [#54](https://github.com/dannys-code-corner/incan/pull/54)

## Summary

Allow `@test` functions to live alongside production code in the same file, combining Python's familiar decorator syntax with Rust's proximity benefits. Test functions are automatically stripped from production builds.

## Motivation

### The Problem with Separate Test Files

Python's convention of `tests/test_*.py` files works, but has friction:

1. **Context switching** — Edit `utils.py`, switch to `test_utils.py`, lose mental context
2. **Drift** — Tests get out of sync when code moves/renames
3. **Discoverability** — New contributors don't see tests alongside code
4. **Small modules** — Creating a separate test file for 10 lines of code feels heavyweight

### Rust's Solution

Rust embeds unit tests in the same file:

```rust
fn add(a: i64, b: i64) -> i64 { a + b }

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_add() {
        assert_eq!(add(1, 2), 3);
    }
}
```

This is stripped from release builds via `#[cfg(test)]`.

### Incan's Opportunity

Combine Python's ergonomics with Rust's proximity:

```incan
def add(a: int, b: int) -> int:
    return a + b

@test
def test_add() -> None:
    assert_eq(add(1, 2), 3)
```

## Design

### Basic Syntax

Any function decorated with `@test` is a test function:

```incan
# src/math.incn

def factorial(n: int) -> int:
    if n <= 1:
        return 1
    return n * factorial(n - 1)

@test
def test_factorial_base() -> None:
    assert_eq(factorial(0), 1)
    assert_eq(factorial(1), 1)

@test
def test_factorial_recursive() -> None:
    assert_eq(factorial(5), 120)
```

### Running Tests

```bash
# Run tests in a specific file
incan test src/math.incn

# Run all tests in directory (finds @test in all .incn files)
incan test src/

# Run tests matching pattern
incan test -k "factorial" src/
```

### Build Behavior

| Command         | `@test` functions                |
| --------------- | -------------------------------- |
| `incan build`   | **Stripped** (not compiled)      |
| `incan run`     | **Stripped** (not included)      |
| `incan test`    | **Included** and executed        |
| `incan --check` | **Type-checked** but not emitted |

### Inline Tests with Fixtures

Fixtures work naturally with inline tests:

```incan
# src/database.incn

class Database:
    def connect(url: str) -> Database:
        ...
    
    def query(self, sql: str) -> List[Row]:
        ...

@fixture
def test_db() -> Database:
    db = Database.connect("sqlite::memory:")
    yield db
    db.close()

@test
def test_query(test_db: Database) -> None:
    result = test_db.query("SELECT 1")
    assert_eq(len(result), 1)
```

### Private Function Testing

Inline tests can access private (non-`pub`) functions:

```incan
# src/parser.incn

def parse(source: str) -> AST:
    tokens = _tokenize(source)  # private helper
    return _build_ast(tokens)

def _tokenize(source: str) -> List[Token]:
    # Private implementation detail
    ...

def _build_ast(tokens: List[Token]) -> AST:
    # Private implementation detail
    ...

# Tests can access private functions!
@test
def test_tokenize() -> None:
    tokens = _tokenize("1 + 2")
    assert_eq(len(tokens), 3)
```

This mirrors Rust's `use super::*` pattern where inline tests access module internals.

### Mixing Inline and Separate Tests

Both approaches coexist:

```bash
project/
├── src/
│   ├── math.incn           # Has @test functions inline
│   └── complex_module.incn # No inline tests
└── tests/
    └── test_complex.incn   # Separate test file for complex cases
```

## Compilation Strategy

### Detection

During parsing, track which functions have `@test` decorator:

```rust
pub struct FunctionDecl {
    pub name: String,
    pub is_test: bool,  // true if @test decorated
    // ...
}
```

### Conditional Compilation

In codegen, check compilation mode:

```rust
fn emit_function(&mut self, func: &FunctionDecl) {
    // Skip @test functions in production builds
    if func.is_test && !self.test_mode {
        return;
    }
    
    // Emit test attribute when in test mode
    if func.is_test {
        self.emitter.line("#[test]");
    }
    
    // ... rest of function emission
}
```

### Generated Rust

**Incan:**

```incan
def add(a: int, b: int) -> int:
    return a + b

@test
def test_add() -> None:
    assert_eq(add(1, 2), 3)
```

**Production build (`incan build`):**

```rust
fn add(a: i64, b: i64) -> i64 {
    a + b
}
// test_add is NOT emitted
```

**Test build (`incan test`):**

```rust
fn add(a: i64, b: i64) -> i64 {
    a + b
}

#[test]
fn test_add() {
    assert_eq!(add(1, 2), 3);
}
```

## Examples

### Simple Unit Tests

```incan
# src/strings.incn

def reverse(s: str) -> str:
    return s[::-1]

def is_palindrome(s: str) -> bool:
    return s == reverse(s)

@test
def test_reverse() -> None:
    assert_eq(reverse("hello"), "olleh")
    assert_eq(reverse(""), "")

@test
def test_palindrome() -> None:
    assert_true(is_palindrome("radar"))
    assert_false(is_palindrome("hello"))
```

### With Parametrize

```incan
# src/validators.incn

def is_valid_email(email: str) -> bool:
    return "@" in email and "." in email

@test
@parametrize("email, expected", [
    ("user@example.com", true),
    ("invalid", false),
    ("no-at.com", false),
    ("user@domain", false),
])
def test_email_validation(email: str, expected: bool) -> None:
    assert_eq(is_valid_email(email), expected)
```

### Error Case Testing

```incan
# src/division.incn

def safe_divide(a: int, b: int) -> Result[int, str]:
    if b == 0:
        return Err("division by zero")
    return Ok(a / b)

@test
def test_divide_success() -> None:
    result = safe_divide(10, 2)
    assert_true(result.is_ok())
    assert_eq(result.unwrap(), 5)

@test
def test_divide_by_zero() -> None:
    result = safe_divide(10, 0)
    assert_true(result.is_err())
    assert_eq(result.unwrap_err(), "division by zero")
```

## Test Discovery

The test runner discovers tests by:

1. **Scanning files** — Find all `.incn` files in target path
2. **Parsing** — Look for `@test` decorated functions
3. **Filtering** — Apply `-k` pattern if provided
4. **Executing** — Run matching tests

```bash
$ incan test src/

Discovered 12 tests in 4 files:
  src/math.incn: 3 tests
  src/strings.incn: 2 tests
  src/validators.incn: 4 tests
  src/division.incn: 3 tests

Running tests...
✓ test_factorial_base (0.001s)
✓ test_factorial_recursive (0.001s)
...

12 passed, 0 failed
```

## Alternatives Considered

### 1. Rust-style `test` Module

```incan
def add(a: int, b: int) -> int:
    return a + b

@test_module
module tests:
    def test_add() -> None:
        assert_eq(add(1, 2), 3)
```

**Rejected:** More syntax to learn, less Pythonic. The `@test` decorator is simpler.

### 2. Doctest Only

```incan
def add(a: int, b: int) -> int:
    """
    >>> add(1, 2)
    3
    """
    return a + b
```

**Rejected:** Good for simple examples, but insufficient for complex tests with setup/teardown.

### 3. Separate Files Only (Python-style)

Keep all tests in `tests/` directory.

**Rejected:** Loses Rust's proximity benefits. Can still use this for integration tests.

## Implementation Plan

### Phase 1: Parser Changes

1. Recognize `@test` decorator
2. Mark `FunctionDecl.is_test = true`
3. Track in AST for later filtering

### Phase 2: Test Runner Changes

1. Scan source files (not just `tests/` directory)
2. Discover `@test` functions in any `.incn` file
3. Include in test execution

### Phase 3: Codegen Changes

1. Add `test_mode: bool` to codegen context
2. Skip `@test` functions when `test_mode = false`
3. Emit `#[test]` attribute when `test_mode = true`

### Phase 4: CLI Integration

1. `incan build` — strips tests (default)
2. `incan test path/` — discovers and runs inline tests
3. `incan test -k pattern` — filters tests

## Checklist

- [ ] Parser: recognize `@test` decorator on functions
- [ ] AST: track `is_test` on `FunctionDecl`
- [ ] Codegen: conditional compilation based on `test_mode`
- [ ] Test runner: scan source files for `@test` functions
- [ ] Test runner: merge inline tests with `tests/` directory tests
- [ ] CLI: `incan test src/` works for inline tests
- [ ] Documentation and examples
- [ ] Verify fixtures work with inline tests

## References

- [Rust Testing](https://doc.rust-lang.org/book/ch11-01-writing-tests.html)
- [pytest Discovery](https://docs.pytest.org/en/stable/goodpractices.html#test-discovery)
- RFC 001: Test Fixtures
- RFC 002: Parametrize
