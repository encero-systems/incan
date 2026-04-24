# Understanding Incan Error Messages

Incan’s compiler errors are designed to be **actionable**: they show you what went wrong,
where it happened, and what to do next.

## Reading Error Output

```bash
type error: Type mismatch: expected 'Result[str, str]', found 'str'
  --> examples/demo.incn:8:5
    |
  8 |     return "Alice"
    |     ^^^^^^^^^^^^^^
  = note: In Incan, functions that can fail return Result[T, E]
  = hint: Wrap the value with Ok(...) to return success
  = hint: Or use Err(...) to return an error
```

**Components:**

- **Error type**: `type error`, `syntax error`, `warning`, `lint`
- **Message**: What went wrong
- **Location**: File, line, and column
- **Source**: The problematic code with underline
- **Notes**: Background explanation (why this matters)
- **Hints**: How to fix it

## Common Errors and Solutions

### Result Type Mismatches

**Error:**

```bash
Type mismatch: expected 'Result[User, Error]', found 'User'
```

**Problem:** Function returns `Result` but you're returning a plain value.

**Solution:**

```incan hl_lines="3 7"
# Wrong
def get_user(id: int) -> Result[User, Error]:
    return User(name="Alice")  # ❌ Missing Ok()

# Right
def get_user(id: int) -> Result[User, Error]:
    return Ok(User(name="Alice"))  # ✅
```

---

**Error:**

```bash
Type mismatch: expected 'str', found 'Result[str, Error]'
```

**Problem:** Using a `Result` where a plain value is expected.

**Solution:**

```incan hl_lines="4 8"
# Wrong
def greet() -> str:
    name = get_name()  # Returns Result[str, Error]
    return name  # ❌ name is still Result

# Right - use ? to unwrap
def greet() -> Result[str, Error]:
    name = get_name()?  # ✅ Unwraps Ok, returns Err early
    return Ok(f"Hello, {name}")

# Or handle explicitly
def greet_safe() -> str:
    match get_name():
        case Ok(name): return f"Hello, {name}"
        case Err(_): return "Hello, stranger"
```

### The `?` Operator

**Error:**

```bash
Cannot use '?' on type 'int' - expected Result[T, E]
```

**Problem:** The `?` operator only works on `Result` types.

**Solution:**

```incan hl_lines="3 7"
# Wrong
def double(x: int) -> int:
    return x?  # ❌ x is not a Result

# Right
def double(x: int) -> int:
    return x * 2  # ✅ No ? needed
```

### Mutability Errors

**Error:**

```bash
Cannot mutate 'count' - variable is immutable
```

**Problem:** Trying to change a variable declared without `mut`.

**Why:** Incan variables are immutable by default for safety and clarity.

**Solution:**

```incan hl_lines="4 9"
# Wrong
def counter() -> int:
    let count = 0
    count = count + 1  # ❌ count is immutable
    return count

# Right
def counter() -> int:
    mut count = 0      # ✅ Declare as mutable
    count = count + 1
    return count
```

### Self Mutation

**Error:**

```bash
Cannot mutate self - method takes immutable self
```

**Problem:** Method modifies `self` but doesn't declare `mut self`.

**Solution:**

```incan hl_lines="5 8"
model Counter:
    value: int
    
    # Wrong
    def increment(self) -> None:
        self.value += 1  # ❌ self is immutable
    
    # Right
    def increment(mut self) -> None:  # ✅ mut self
        self.value += 1
```

### Missing Traits

**Error:**

```bash
Type 'Point' does not implement trait 'Eq'
```

**Problem:** Using `==` on a type without equality support.

**Solution:**

```incan
# Add @derive(Eq) to enable ==
@derive(Eq)
model Point:
    x: int
    y: int

# Now this works
if p1 == p2:
    println("Same point!")
```

**Common derives:**

| Need                          | Derive              |
| ----------------------------- | ------------------- |
| `==`, `!=`                    | `@derive(Eq)`       |
| `<`, `>`, `<=`, `>=`          | `@derive(Ord)`      |
| Use in `Set` or as `Dict` key | `@derive(Hash, Eq)` |
| `.clone()` method             | `@derive(Clone)`    |
| `{:?}` debug format           | `@derive(Debug)`    |

> See [Derives & Traits](../reference/derives_and_traits.md) for more information on Derives and Traits

### Option Handling

**Error:**

```bash
Type mismatch: expected 'bool', found 'Option[User]'
```

**Problem:** Using `Option` directly in a condition.

**Solution:**

```incan hl_lines="3 7 12"
# Wrong
let user = find_user(id)
if user:  # ❌ Can't use Option as bool
    ...

# Right - use pattern matching control flow
if let Some(u) = user:  # ✅
    println(u.name)

# Or check explicitly
if user.is_some():
    let u = user.unwrap()
    println(u.name)
```

When you need to keep iterating while a pattern keeps matching, use `while let` instead of a `while true` plus `match` / `break` ladder:

```incan
mut current = find_user(id)
while let Some(user) = current:
    println(user.name)
    current = next_user(user)
```

## Philosophy: Explicit is Better

Incan intentionally requires explicit handling of:

1. **Errors** - No silent failures; use `Result` and `?`
2. **Nullability** - No `None` surprises; use `Option`
3. **Mutation** - Immutable by default; explicit `mut`

This catches bugs at compile time rather than runtime.

## Getting Help

If an error message is unclear:

1. Read the **hints** - they often show the exact fix
2. Check the **notes** - they explain the underlying concept
3. Look at [Error Handling](../explanation/error_handling.md) for `Result`/`Option` patterns
4. See [Derives & Traits](../reference/derives_and_traits.md) for trait requirements

## Next Steps

- [Error Handling](../explanation/error_handling.md) - Working with `Result` types
- [Derives & Traits](../reference/derives_and_traits.md) - Drop trait for custom cleanup
- [File I/O](file_io.md) - Reading, writing, and path handling
- [Async Programming](async_programming.md) - Async/await with Tokio
- [Imports & Modules](imports_and_modules.md) - Module system, imports, and built-in functions
- [Rust Interop](rust_interop.md) - Using Rust crates directly from Incan
- [Web Framework](../tutorials/web_framework.md) - Building web apps with Axum
