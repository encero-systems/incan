# 6. Errors (Result/Option and `?`)

<div class="inc-book-progress" aria-label="Chapter 6 of 13"><div class="inc-book-progress__meta"><strong>Chapter 6 of 13</strong><span>Result, Option, and ?</span></div><div class="inc-book-progress__bar" aria-hidden="true"><span style="--inc-progress: 46.2%"></span></div></div>

Incan uses explicit error values (`Result` and `Option`) instead of Python-style exceptions.

## `Result[T, E]`

Use `Result` for operations that can fail.

```incan
def read_username() -> Result[str, str]:
    name = input("Name: ").strip()
    if len(name) == 0:
        return Err("name must not be empty")
    return Ok(name)
```

Handle it with `match`:

```incan
def main() -> None:
    match read_username():
        case Ok(name): println(f"hello, {name}")
        case Err(e): println(f"error: {e}")
```

## `Option[T]`

Use `Option` for “value may be absent”:

`Option[T]` has two variants:

- `Some(value)` — value is present
- `None` — value is absent

You create a present value by wrapping it: `Some(x)`.

```incan
def first(items: List[str]) -> Option[str]:
    if len(items) == 0:
        return None
    return Some(items[0])
```

You handle it with `match` by destructuring `Some(...)`.

```incan
def main() -> None:
    match first(["a", "b"]):
        case Some(x): println(f"first={x}")
        case None: println("empty")
```

!!! tip "Coming from Python?"
    In Python typing, you’d usually express “may be missing” as:

    ```python
    from typing import Optional

    def first(items: list[str]) -> Optional[str]:
        return items[0] if items else None
    ```

    In Python, `Optional[T]` is mostly a type-hinting/tooling concept. In Incan, `Option[T]` is an explicit enum that the compiler can reason about and enforce.

## Propagating errors with `?`

The `?` operator returns early on `Err`, otherwise unwraps the `Ok` value:

```incan
def greet_user() -> Result[None, str]:
    name = read_username()?
    println(f"hello, {name}")
    return Ok(None)
```

## Structured errors (recommended)

Prefer structured errors over strings when the caller should branch on error kinds:

```incan
enum NameError:
    Empty

def normalize(name: str) -> Result[str, NameError]:
    cleaned = name.strip()
    if len(cleaned) == 0:
        return Err(NameError.Empty)
    return Ok(cleaned.lower())
```

## Try it

1. Write `def safe_div(a: float, b: float) -> Result[float, str]`.
2. Write `def first(items: List[str]) -> Option[str]` and handle `None` vs `Some(...)` with `match`.
3. Write `def greeting_for(name: str) -> Result[str, str]` that uses `?` to propagate a validation error from a `normalize_name` helper.

??? example "One possible solution"

    ```incan
    --8<-- "_snippets/language/examples/verified_errors_solution.incn"
    ```

The builtin `float(text)` conversion raises a runtime conversion error for malformed text; it does not return `Result`. The exercise therefore uses an explicitly fallible helper so the `?` example matches the type system.

## What to learn next

- Choosing between plain values, `Option`, `Result`, and panics: [Fallible and infallible paths](../fallible_and_infallible_paths.md)
- Results, Options, and the `?` operator (deep dive): [Error Handling](../../explanation/error_handling.md)
- Common error message patterns: [Error Messages](../../how-to/error_messages.md)

<nav class="inc-prev-next" aria-label="Book chapter navigation"><a href="../05_modules_and_imports/"><small>Previous chapter</small><strong>← 5. Modules and imports</strong></a><a href="../07_strings_and_formatting/"><small>Next chapter</small><strong>7. Strings and formatting →</strong></a></nav>
