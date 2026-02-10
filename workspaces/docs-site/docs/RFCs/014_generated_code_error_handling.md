# RFC 014: Error Handling in Generated Rust Code

**Status:** Draft

## Summary

The Incan compiler currently emits `.unwrap()` calls in generated Rust code for operations that can fail at runtime.
This causes cryptic Rust panics when users encounter edge cases. This RFC proposes a phased approach to improve error
handling in generated code.

## Problem

Generated Rust code contains several `.unwrap()` patterns that can panic:

```rust
// Dict access - panics if key doesn't exist
map.get(&key).unwrap().clone()

// String parsing - panics if string isn't a valid number
input.parse::<i64>().unwrap()

// List indexing - panics if index out of bounds
vec[index]  // or explicit .get(i).unwrap()

// List.index() - panics if value not found
__incan_list_find_index(&vec, &value).unwrap()
```

When these panic, users see Rust stack traces instead of helpful Incan error messages:

```bash
thread 'main' panicked at 'called `Option::unwrap()` on a `None` value'
```

## Goals

1. **Clear error messages**: Users should see Incan-level errors, not Rust panics
2. **Source locations**: Errors should reference Incan source lines when possible
3. **Graceful degradation**: Some operations should return `None`/default instead of panicking
4. **Future-proof**: Design should support Incan-level error handling via `Result`/`Option`, `match`, and `?`

## Proposed Solution

### Phase 1: Better Panic Messages (Immediate)

Replace bare `.unwrap()` with `.unwrap_or_else()` that provides context:

```rust
// Before
map.get(&key).unwrap().clone()

// After
map.get(&key)
    .unwrap_or_else(|| panic!("KeyError: '{}' not found in dict", key))
    .clone()
```

For string parsing:

```rust
// Before
input.parse::<i64>().unwrap()

// After
input.parse::<i64>()
    .unwrap_or_else(|_| panic!("ValueError: cannot convert '{}' to int", input))
```

### Phase 2: Runtime Error Type

Introduce an Incan runtime error type that captures context:

```rust
// In generated prelude
#[derive(Debug)]
pub enum IncanError {
    KeyError { key: String, context: &'static str },
    ValueError { value: String, expected: &'static str },
    IndexError { index: usize, len: usize },
    TypeError { found: &'static str, expected: &'static str },
}

impl std::fmt::Display for IncanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IncanError::KeyError { key, context } => 
                write!(f, "KeyError: '{}' not found{}", key, context),
            // ... etc
        }
    }
}
```

### Phase 3: Optional Error Recovery

For operations where returning `None`/default makes sense:

```incan
# Dict.get() returns Option
value = my_dict.get("key")  # Returns Option[T]

# With default
value = my_dict.get("key", default="fallback")
```

Generated as:

```rust
let value = my_dict.get("key").cloned();  // Option<T>
let value = my_dict.get("key").cloned().unwrap_or("fallback".to_string());
```

### Phase 4: Incan-Level Error Handling (no `try/except`)

Incan should **not** adopt Python-style `try/except` blocks. Instead, error handling stays explicit and type-driven:

- Fallible operations return `Result[T, IncanError]` (or `Option[T]` where absence is expected)
- Callers handle errors via `match` (or propagate with `?`)

Example (handle a parse failure by falling back to a default):

```incan
# Incan uses a match expression to handle errors, instead of a try-except block (like Python)
match int(user_input):
    Ok(v) => value = v
    Err(_) => value = 0
```

Generated as:

```rust
let value = match user_input.parse::<i64>() {
    Ok(v) => v,
    Err(_) => 0,
};
```

> Note: this is where Incan is deliberately more rust-like than Python.

## Specific Improvements

### Dict Access

- **`d[key]`**
    - **Current**:

```rust
.get(...).expect("KeyError: key not found in dict").clone()
```

    - **Proposed**:

```rust
.get(...).unwrap_or_else(|| panic!("KeyError: key not found in dict")).clone()
```

    - **Note**: string-literal keys include the key in the message today.

- **`d.get(key)`**
    - **Proposed**:

```rust
.get(&k).cloned() // -> Option<V>
```

- **`d.get(key, default)`**
    - **Proposed**:

```rust
.get(&k).cloned().unwrap_or(default)
```

### String Parsing

- **`int(s)`**
    - **Current**: depends on input form
        - string literal:

```rust
.parse::<i64>().expect("ValueError: ...")
```

        - known `String` variable:

```rust
.parse::<i64>().unwrap_or_else(|_| panic!("ValueError: ...", s))
```

        - non-string:

```rust
(x as i64)
```

    - **Proposed**: parse strings with a `ValueError` message; cast non-strings for performance.

- **`float(s)`**
    - **Current**: depends on input form
        - string literal:

```rust
.parse::<f64>().expect("ValueError: ...")
```

        - known `String` variable:

```rust
.parse::<f64>().unwrap_or_else(|_| panic!("ValueError: ...", s))
```

        - non-string:

```rust
(x as f64)
```

    - **Proposed**: parse strings with a `ValueError` message; cast non-strings for performance.

### List Operations

- **`list[i]`**
    - **Current** (panics with Rust bounds message today):

```rust
vec[(i as usize)].clone()
```

    - **Proposed**:

```rust
vec.get(i as usize)
    .cloned()
    .unwrap_or_else(|| panic!("IndexError: index {} out of range for list of length {}", i, vec.len()))
```

- **`list.index(v)`**
    - **Current**:

```rust
__incan_list_find_index(&vec, &v).expect("ValueError: value not found in list")
```

    - **Proposed**:

```rust
__incan_list_find_index(&vec, &v).unwrap_or_else(|| panic!("ValueError: value not found in list"))
```

## Implementation Plan

1. **Phase 1**: Update codegen to emit better panic messages
2. **Phase 2**: Add IncanError type to generated prelude
3. **Phase 3**: Add `Dict.get(key, default=...)` **and introduce a new safe list API**
    (e.g. `List.get(index, default=...)` or `List.at(index)` returning `Option[T]`)
    and update codegen to use Rust's `Vec::get`.

## Alternatives Considered

### Always Return Result

Make all fallible operations return `Result<T, IncanError>`. Rejected because:

- Requires pervasive `?` or `.unwrap()` in generated code
- Doesn't match Python/Incan semantics where these operations "just work"
- Would require pervasive `Result` handling (via `match`/`?`) to be usable

### Silent Defaults

Return default values instead of panicking (e.g., `0` for missing int parse). Rejected because:

- Hides bugs
- Doesn't match Python semantics (Python raises exceptions)
- Makes debugging harder

## References

- Python's exception model
- Rust's `unwrap_or_else` pattern
- [RFC 013: Rust Crate Dependencies][RFC 013] — related codegen concerns

--8<-- "_snippets/rfcs_refs.md"
