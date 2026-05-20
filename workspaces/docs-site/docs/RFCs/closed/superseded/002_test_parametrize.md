# RFC 002: Parametrized Tests

**Status:** Draft **Created:** 2024-12-08 **Issue:** [#54](https://github.com/dannys-code-corner/incan/pull/54)

## Summary

Add pytest-style parametrized tests via the `@parametrize` decorator.

## Motivation

Parametrized tests allow:

1. **DRY testing** - One test definition, many inputs
2. **Clear failures** - Each parameter set is a separate test case
3. **Readable test data** - Parameters are explicit, not hidden in loops

## Design

### Basic Syntax

```incan
from testing import parametrize

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

### Named Parameters

```incan
@parametrize("input, expected", [
    ("hello", "HELLO"),
    ("World", "WORLD"),
    ("", ""),
], ids=["lowercase", "mixed", "empty"])
def test_upper(input: str, expected: str) -> None:
    assert_eq(input.upper(), expected)
```

Output:

```bash
test_string.incn::test_upper[lowercase] PASSED
test_string.incn::test_upper[mixed] PASSED
test_string.incn::test_upper[empty] PASSED
```

### Multiple Parametrize Decorators

```incan
@parametrize("x", [1, 2, 3])
@parametrize("y", [10, 20])
def test_multiply(x: int, y: int) -> None:
    result = x * y
    assert_true(result > 0)
```

Generates 6 tests (cartesian product):

```bash
test_multiply[1-10], test_multiply[1-20],
test_multiply[2-10], test_multiply[2-20],
test_multiply[3-10], test_multiply[3-20]
```

### Complex Types

```incan
model TestCase:
    input: str
    expected: int
    description: str

@parametrize("case", [
    TestCase(input="hello", expected=5, description="simple word"),
    TestCase(input="", expected=0, description="empty string"),
    TestCase(input="hello world", expected=11, description="with space"),
])
def test_length(case: TestCase) -> None:
    assert_eq(len(case.input), case.expected)
```

### Combining with Fixtures

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

## Implementation

### Phase 1: Parser Changes

Parse `@parametrize` decorator:

```rust
DecoratorArg::Parametrize {
    argnames: String,           # "a, b, expected"
    argvalues: Vec<Tuple>,      # [(1,2,3), (0,0,0), ...]
    ids: Option<Vec<String>>,   # Optional test IDs
}
```

### Phase 2: Test Discovery

When discovering tests:

1. Detect `@parametrize` decorator
2. Parse argnames and argvalues
3. Generate N `TestInfo` entries, one per parameter set

```rust
struct TestInfo {
    file_path: PathBuf,
    function_name: String,
    markers: Vec<TestMarker>,
    parameters: Option<ParametrizeInfo>,  // NEW
}

struct ParametrizeInfo {
    argnames: Vec<String>,
    values: Vec<Value>,
    id: String,  // e.g., "1-2-3" or custom ID
}
```

### Phase 3: Codegen

Generate a separate Rust test function for each parameter set:

```rust
// From: test_add with params [(1,2,3), (0,0,0)]

#[test]
fn test_add_1_2_3() {
    let a: i64 = 1;
    let b: i64 = 2;
    let expected: i64 = 3;
    assert_eq!(add(a, b), expected);
}

#[test]
fn test_add_0_0_0() {
    let a: i64 = 0;
    let b: i64 = 0;
    let expected: i64 = 0;
    assert_eq!(add(a, b), expected);
}
```

### Phase 4: Output Formatting

Display parameterized test names:

```bash
test_math.incn::test_add[1-2-3] PASSED
test_math.incn::test_add[0-0-0] PASSED
```

On failure, show which parameters failed:

```bash
FAILED test_math.incn::test_add[1-2-3]

    Parameters: a=1, b=2, expected=3
    
    assert_eq(add(a, b), expected)
    AssertionError: 2 != 3
```

## Edge Cases

### Empty Parameter List

```incan
@parametrize("x", [])
def test_nothing(x: int) -> None:
    pass
```

Result: No tests generated, warning emitted.

### Type Mismatches

```incan
@parametrize("x", [1, "two", 3.0])
def test_typed(x: int) -> None:
    pass
```

Result: Compile error - parameter types must match function signature.

### Nested Tuples

```incan
@parametrize("point", [
    ((0, 0), "origin"),
    ((1, 1), "diagonal"),
])
def test_points(point: Tuple[Tuple[int, int], str]) -> None:
    coords, name = point
    ...
```

## Alternatives Considered

### 1. Loop in Test

```incan
def test_add_all() -> None:
    cases = [(1,2,3), (0,0,0)]
    for a, b, expected in cases:
        assert_eq(add(a, b), expected)
```

**Rejected**:

- Stops at first failure
- No individual test reporting
- Harder to identify which case failed

### 2. Test Tables (Go-style)

```incan
def test_add() -> None:
    table = [
        {"a": 1, "b": 2, "expected": 3},
        {"a": 0, "b": 0, "expected": 0},
    ]
    for case in table:
        with subtest(case):
            assert_eq(add(case["a"], case["b"]), case["expected"])
```

**Rejected**: More verbose, requires subtest infrastructure.

## Open Questions

1. **Lazy evaluation**: Should parameter values be evaluated at collection time or test time?
2. **Indirect parametrization**: Should parameters be able to reference fixtures?
3. **Skip individual params**: `@parametrize(..., marks=[skip, None, xfail])`?

## Checklist

- [ ] Parser: `@parametrize` decorator args (argnames, argvalues, ids)
- [ ] Discovery: expand parameter sets into TestInfo entries
- [ ] Codegen: generate per-parameter Rust test functions
- [ ] Reporting: parameterized test names and failure context
- [ ] Validation: type-check param values vs function signature
- [ ] Edge cases: empty parameter list warnings
- [ ] Fixture interaction: combine with fixtures safely

## References

- [pytest parametrize](https://docs.pytest.org/en/stable/how-to/parametrize.html)
- [Rust test macros](https://docs.rs/test-case/latest/test_case/)
