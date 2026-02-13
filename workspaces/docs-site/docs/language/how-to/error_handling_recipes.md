# Error handling recipes

This page is a practical companion to [Error handling](../explanation/error_handling.md). It focuses on common patterns
and “what to do when…”.

!!! tip "Coming from Python?"
    In Python, many failures surface as **exceptions** (`try`/`except`) and “propagation” is implicit.

    In Incan, common failures are modeled explicitly with `Result[T, E]`:

    - Use **`?`** to *propagate* an `Err(...)` to your caller (similar to “let it raise”, but typed).
    - Use **`match`** to *handle/recover* locally (similar to `try`/`except`, but without hidden control flow).
    - Avoid **`unwrap()`** on user/config/network inputs; it’s closer to `assert False` than “normal error handling”.

## Pattern: Handle vs propagate

Use `?` when you want to propagate failures to the caller.

Use `match` when you want to **recover**, **retry**, **branch**, or **attach context**.

```incan
def load_user(id: int) -> Result[User, AppError]:
    # Propagate: caller decides what to do.
    user = fetch_user(id).map_err(AppError.Network)?
    return Ok(user)

def load_user_or_guest(id: int) -> User:
    # Recover: we decide locally.
    match load_user(id):
        Ok(user) => return user
        Err(_) => return User.guest()
```

## Pattern: Convert errors at module boundaries (`map_err`)

Convert low-level errors into a stable, module-level error type.

```incan
enum ConfigError:
    Io(IoError)
    Parse(str)

def load_config(path: Path) -> Result[Config, ConfigError]:
    content = path.read_text().map_err(ConfigError.Io)?
    cfg = parse_toml[Config](content).map_err(|e| ConfigError.Parse(e))?
    return Ok(cfg)
```

Tip: do this once at the boundary, not at every call site.

## Pattern: Turn `Option[T]` into `Result[T, E]` (`ok_or`)

Use this when “missing” should become a recoverable error.

```incan
def require_user(id: int) -> Result[User, AppError]:
    return users.get(id).ok_or(AppError.NotFound(f"user {id}"))
```

## Pattern: Defaults (`unwrap_or` / `unwrap_or_else`)

Use defaults only when there is a genuinely safe fallback.

```incan
timeout = settings.get("timeout_secs").unwrap_or("2.0")
```

If computing the default is expensive, prefer `unwrap_or_else(...)` when available.

## Pattern: Retry a fallible operation

Keep retries local and explicit.

```incan
def fetch_with_retry(url: str, attempts: int) -> Result[str, NetworkError]:
    mut last_err: Option[NetworkError] = None

    for _ in range(attempts):
        match fetch(url):
            Ok(body) => return Ok(body)
            Err(e) =>
                last_err = Some(e)
                continue

    return Err(last_err.unwrap_or(NetworkError("unreachable")))
```

## Pattern: Errors with recoverable payloads

Sometimes the error should carry a value that would otherwise be lost (e.g. sending on a closed channel).

```incan
import std.async

match await tx.send(msg):
    Ok(_) => println("Sent!")
    Err(e) =>
        save_for_retry(e.value)
```

## “Don’t do this”: `unwrap()` on user input

`unwrap()` and `panic()` are for “should never happen” paths (tests, invariants, internal compiler bugs), not expected
failures.

Prefer `Result` and handle the error where you can recover.

## See also

- [Error handling (concepts)](../explanation/error_handling.md)
- [The Error trait (stdlib)](../reference/stdlib_traits/error.md)
