# Error handling

Incan treats errors as **values**: functions that can fail return a `Result[T, E]`, and callers handle success/error **explicitly**. This makes failure modes visible in APIs, encourages structured error types, and keeps control flow clear.

## Quick start

```incan
def load_config(path: Path) -> Result[Config, AppError]:
    bytes = path.read_bytes().map_err(AppError.Io)?
    config = parse_binary_config[Config](bytes).map_err(AppError.Parse)?
    return Ok(config)
```

The key ideas:

- return `Result[T, E]` for operations that can fail
- use `?` to propagate failures
- use `match` when you want to handle success/error differently

??? info "Coming from Python?"
    Python uses exceptions, which can introduce hidden control flow (“anything can throw from anywhere”). Python 3.10+ has `match`/`case`, but it still won’t enforce exhaustiveness the way Incan does.

    A rough translation is:

    - Python: exceptions propagate implicitly
    - Incan: `?` propagates explicitly (and is visible in the return type)

    **Python (implicit propagation)**:

    
    ```python
    def load_config(path: str) -> Config:
        content = read_file(path)     # may raise IOError
        return parse_config(content)  # may raise ParseError
    ```

    **Incan (explicit propagation)**:

    ```incan
    def load_config(path: Path) -> Result[Config, AppError]:
        bytes = path.read_bytes().map_err(AppError.Io)?
        config = parse_binary_config[Config](bytes).map_err(AppError.Parse)?
        return Ok(config)
    ```

    Quick mapping:

    | Python                          | Incan                      |
    | ------------------------------- | -------------------------- |
    | `try/except` wraps code         | `match` on Result          |
    | Exceptions propagate implicitly | `?` propagates explicitly  |
    | `raise` throws exception        | `return Err(e)`            |
    | Runtime error if unhandled      | Compile error if unhandled |


    ```python title="Python `try/except`"
    try:
        config = load_config("app.toml")
    except (IOError, ValueError) as e:
        log(e)
        config = default_config()
    ```

    vs.

    ```incan title="Incan `match`"
    match load_config(Path("app.toml")):
        Ok(config) => config
        Err(e) =>
            log(e)
            default_config()
    ```

??? info "Coming from TS/JS?"
    In JS/TS, `try/catch` works, but failures are often effectively “untyped” at the boundary (anything can be thrown, and caught values are frequently `unknown`).

    In Incan, fallibility is explicit in the return type, and you handle it with `Result` + `match` (or propagate it with
    `?`).

    **JS/TS `try/catch` → Incan `Result`**:

    ```ts
    async function loadConfig(path: string): Promise<Config> {
      try {
        const content = await readFile(path); // may throw / reject
        return parseConfig(content);          // may throw
      } catch (e) {
        return defaultConfig();
      }
    }
    ```

```incan
def load_config(path: Path) -> Result[Config, AppError]:
    bytes = path.read_bytes().map_err(AppError.Io)?
    config = parse_binary_config[Config](bytes).map_err(AppError.Parse)?
    return Ok(config)

def load_config_or_default(path: Path) -> Config:
        match load_config(path):
            Ok(cfg) => return cfg
            Err(_) => return default_config()
    ```

## Core types

### `Result[T, E]`

`Result` represents an operation that can succeed (`Ok(T)`) or fail (`Err(E)`).

```incan
def divide(a: int, b: int) -> Result[int, str]:
    if b == 0:
        return Err("division by zero")
    return Ok(a / b)

def main() -> None:
    match divide(10, 2):
        Ok(value) => println(f"Result: {value}")
        Err(msg) => println(f"Error: {msg}")
```

### `Option[T]`

`Option` (optional) represents a value that may or may not exist.

```incan
def find_user(id: int) -> Option[User]:
    if id in users:
        return Some(users[id])
    return None

def main() -> None:
    match find_user(42):
        Some(user) => println(f"Found: {user.name}")
        None => println("User not found")
```

!!! info "Coming from Python?"
    In Python, `Optional[T]` is a type hint that indicates the value may be `None`. In Incan, `Option[T]` is an explicit enum that the compiler can reason about and enforce.

## The `?` operator (propagation)

The `?` operator provides concise error propagation: if the left-hand side is `Err(e)`, return early with that error; otherwise unwrap the `Ok(...)` value.

```incan
def process() -> Result[Data, Error]:
    data = fetch_data()?
    valid = validate(data)?
    return Ok(valid)
```

Rules of thumb:

- `?` can only be used in a function that returns `Result[...]` (or another compatible “fallible” type, if/when added).
- use `match` instead of `?` when you want to recover, retry, or branch based on the error.

### A common compile-time error

`?` is only allowed when the function can return an error.

```incan
def main() -> None:
    data = fetch_data()?  # error: can't use ? in a non-Result function
```

## Structured error types

Prefer structured errors over strings so callers can pattern match on failure modes and carry context.

```incan
enum ProcessError:
    NetworkFailure(str, int)     # (url, status)
    ValidationFailed(str, str)   # (field, reason)
    NotFound(str)                # resource

def process() -> Result[Data, ProcessError]:
    return Err(ProcessError.NotFound("user"))
```

## Common helpers

### Transform errors with `map_err`

Use this when you want to convert one error type into another at a boundary.

```incan
def load_config(path: Path) -> Result[Config, AppError]:
    bytes = path.read_bytes().map_err(AppError.Io)?
    config = parse_binary_config[Config](bytes).map_err(AppError.Parse)?
    return Ok(config)
```

### Provide defaults with `unwrap_or`

Use this for optional values where a default makes sense.

```incan
def get_setting(key: str) -> str:
    return settings.get(key).unwrap_or("default_value")
```

### Convert `Option[T]` to `Result[T, E]` with `ok_or`

Use this when absence should become a recoverable error.

```incan
def require_user(id: int) -> Result[User, str]:
    return find_user(id).ok_or("user not found")
```

## `unwrap()` and `panic()`

`unwrap()` extracts a value from `Option`/`Result`, and **panics** if it is `None`/`Err(...)`.

Panics are appropriate for programmer errors and “should never happen” states, but not for expected failures (like user input or network calls).

Prefer:

- `match` for explicit handling
- `unwrap_or(...)` / `unwrap_or_else(...)` when you have a reasonable fallback
- structured `Result` errors for recoverable failure

## See also

- [Example: Error Handling](https://github.com/encero-systems/incan/blob/main/examples/intermediate/error_handling.incn)
- [Fallible and infallible paths](../tutorials/fallible_and_infallible_paths.md)
- [Async programming](../how-to/async_programming.md)
- [Error handling recipes](../how-to/error_handling_recipes.md)
- [Error trait](../reference/stdlib_traits/error.md)
- [Modeling errors with enums](../how-to/modeling_with_enums.md)
