# Error handling recipes

This page is a practical companion to [Error handling](../explanation/error_handling.md). It focuses on common patterns and “what to do when…”.

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
    Io(str)
    Parse(str)

def load_config(path: Path) -> Result[Config, ConfigError]:
    content = path.read_bytes().map_err(ConfigError.Io)?
    cfg = parse_binary_config[Config](content).map_err(|e| ConfigError.Parse(e))?
    return Ok(cfg)
```

Tip: do this once at the boundary, not at every call site.

## Pattern: Turn `Option[T]` into `Result[T, E]` (`ok_or`)

Use this when “missing” should become a recoverable error.

```incan
def require_user(id: int) -> Result[User, AppError]:
    return users.get(id).ok_or(AppError.NotFound(f"user {id}"))
```

## Pattern: Do something only on success (`if let`)

Use `if let` when you care about exactly one successful shape and want the non-match case to do nothing.

```incan
def maybe_log_user(id: int) -> None:
    if let Some(user) = users.get(id):
        println(f"loaded {user.name}")
```

This is usually clearer than spelling the same idea as a full `match` with an empty fallback arm.

## Pattern: Defaults (`unwrap_or` / `unwrap_or_else`)

Use defaults only when there is a genuinely safe fallback.

```incan
timeout = match settings.get("timeout_secs"):
    Some(value) => str(value)
    None => "2.0"
```

`dict.get(...)` borrows the stored value. `str(value)` materializes an owned string before it leaves the match arm.

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

## Pattern: Retry inside a fallible iterator

`FallibleIterator` has no generic retry adapter. Repeating a failed poll is safe only when the source knows whether its cursor advanced, whether the operation is idempotent, which errors are transient, and how attempts and backoff are counted.

A remote paginator that supports retry should therefore accept an explicit application or domain policy. Its own `__next__` implementation keeps the logical cursor unchanged while retrying, advances it only after a successful page, and emits one final error when the policy declines another attempt. `inspect_err`, `map_err`, `collect`, `fold`, and `for ...?:` outside that source observe only the final emitted error. Instrumentation for individual attempts belongs inside the paginator or policy.

The following design sketch is application code, not a shipped `std.http` API. The transport and wait callbacks make every effect visible, while the paginator alone owns cursor safety:

```incan
from std.derives.collection import FallibleIterator

model Record:
    id: str
    active: bool


model FetchError:
    detail: str
    transient: bool


model RetryPolicy:
    max_attempts: int
    backoff_ms: int

    def should_retry(self, error: FetchError, attempt: int) -> bool:
        return error.transient and attempt < self.max_attempts

    def delay_ms(self, attempt: int) -> int:
        return self.backoff_ms * attempt


model RemotePage:
    records: list[Record]
    next_cursor: Option[str]


model RetryingPages with FallibleIterator[RemotePage, FetchError]:
    cursor: str
    done: bool
    retry: RetryPolicy
    fetch: (str) -> Result[RemotePage, FetchError]
    wait: (int) -> None

    def __next__(mut self) -> Result[Option[RemotePage], FetchError]:
        if self.done:
            return Ok(None)

        mut attempt = 1
        while true:
            request_page = self.fetch
            match request_page(self.cursor):
                Ok(page) =>
                    match page.next_cursor:
                        Some(next_cursor) => self.cursor = next_cursor
                        None => self.done = true
                    return Ok(Some(page))
                Err(error) =>
                    if not self.retry.should_retry(error, attempt):
                        return Err(error)
                    wait_before_retry = self.wait
                    wait_before_retry(self.retry.delay_ms(attempt))
                    attempt += 1
```

No retry is magical here. A failed call leaves `self.cursor` untouched. The policy classifies retryable failures and limits attempts, the injected `wait` callback owns backoff, and only a successful page advances or closes the cursor. If the policy declines, `__next__` emits the final `FetchError`; outer combinators and `for ...?:` observe exactly that one error.

The adapters and terminals are specified in [Collection protocols](../reference/stdlib_traits/collection_protocols.md#fallibleiterator-fallible-iteration).

## Pattern: Errors with recoverable payloads

Sometimes the error should carry a value that would otherwise be lost (e.g. sending on a closed channel).

```incan
import std.async

match await tx.send(msg):
    Ok(_) => println("Sent!")
    Err(e) =>
        save_for_retry(e.value)
```

## Pattern: Define an error type with `Error`

Adopt `Error` on a model and implement `message()`; the model then serves as the `E` of a `Result`:

```incan
from std.traits.error import Error

model AgeValidationError with Error:
    field: str
    msg: str

    def message(self) -> str:
        return f"Validation failed for '{self.field}': {self.msg}"

def validate_age(age: int) -> Result[int, AgeValidationError]:
    if age < 0:
        return Err(AgeValidationError(field="age", msg="cannot be negative"))
    return Ok(age)
```

## Pattern: Record the cause with `source()`

When an error wraps a lower-level failure, keep the cause in a field and return it from `source()`:

```incan
from std.traits.error import Error

model DatabaseError with Error:
    query: str
    cause: Option[str]

    def message(self) -> str:
        return f"Database query failed: {self.query}"

    def source(self) -> Option[str]:
        return self.cause
```

## “Don’t do this”: `unwrap()` on user input

`unwrap()` and `panic()` are for “should never happen” paths (tests, invariants, internal compiler bugs), not expected failures.

Prefer `Result` and handle the error where you can recover.

## See also

- [Error handling (concepts)](../explanation/error_handling.md)
- [The Error trait (stdlib)](../reference/stdlib_traits/error.md)
