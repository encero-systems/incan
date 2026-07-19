# 4. Control flow

<div class="inc-book-progress" aria-label="Chapter 4 of 13"><div class="inc-book-progress__meta"><strong>Chapter 4 of 13</strong><span>Control flow</span></div><div class="inc-book-progress__bar" aria-hidden="true"><span style="--inc-progress: 30.8%"></span></div></div>

Control flow is how you branch and loop.

## `if` / `elif` / `else`

```incan
def describe(n: int) -> str:
    if n < 0:
        return "negative"
    elif n == 0:
        return "zero"
    else:
        return "positive"
```

Use ordinary `if` when the condition is a boolean expression.

## `if let` (do something only when a pattern matches)

Use `if let` when you care about one success branch and want the non-match case to do nothing.

```incan
def greet(user: Option[User]) -> None:
    if let Some(u) = user:
        println(f"hello {u.name}")
```

This is shorter than a full `match` when the only interesting case is the successful one.

```incan
def greet(user: Option[User]) -> None:
    match user:
        case Some(u): println(f"hello {u.name}")
        case None: pass
```

Use `match` instead when both branches matter. In v1, `if let` is single-arm only and does not accept `elif` or `else`.

The success branch can use pattern alternation when several patterns share the same body:

```incan
enum JobStatus:
    Running
    Completed
    Cancelled

def log_done(status: JobStatus) -> None:
    if let Completed | Cancelled = status:
        println("done")
```

## `match` (pattern matching)

`match` is the main way to branch on enums like `Result` and `Option`:

```incan
def validate_port(port: int) -> Result[int, str]:
    if port < 1 or port > 65535:
        return Err("port must be between 1 and 65535")
    return Ok(port)

def main() -> None:
    result = validate_port(8080)

    match result:
        case Ok(port): println(f"port={port}")
        case Err(e): println(f"error: {e}")
```

Use `|` inside a `match` arm when alternatives should share the same body:

```incan
enum PortLookup:
    Cached(int)
    Fresh(int)
    Failed(str)

match lookup_port(raw):
    case Cached(port) | Fresh(port): println(f"port={port}")
    case Failed(e): println(f"error: {e}")
```

Alternatives that bind names must bind the same names with the same types. `Cached(port) | Fresh(port)` is valid because both payloads have the same type; `Some(value) | None` is rejected because only one alternative binds `value`.

!!! tip "Coming from Rust?"
    Incan also supports a more Rust-like match-arm style using `=>`:

    --8<-- "_snippets/language/examples/match_arms_rust_style.md"

    This is equivalent to the `case ...:` form; pick whichever reads best to you.

## `while let` (loop while one pattern keeps matching)

Use `while let` when the loop should continue only while one pattern keeps matching.

```incan
async def consume(rx: Receiver[str]) -> None:
    while let Some(msg) = await rx.recv():
        println(f"received {msg}")
```

This is the compact form of:

```incan
async def consume(rx: Receiver[str]) -> None:
    while True:
        match await rx.recv():
            case Some(msg): println(f"received {msg}")
            case None: break
```

## `for` loops

Incan supports Python-like `for` loops:

```incan
def main() -> None:
    items = ["Alice", "Bob", "Cara"]
    for name in items:
        println(name)
```

You can break early:

```incan
for name in items:
    if name == "Bob":
        break
```

## `while` loops

Use `while` when the condition should be re-checked before each iteration:

```incan
def countdown(start: int) -> None:
    mut current = start
    while current > 0:
        println(current)
        current -= 1
```

## `loop:` and `break <value>`

Use `loop:` for explicit infinite loops and for loops that need to return a value:

```incan
def find_value(flag: bool) -> int:
    return loop:
        if flag:
            break 42
        break 7
```

`break <value>` completes the surrounding `loop:` expression. For `for` and `while`, use plain `break`.

## Try it

1. Write a function `classify(n: int) -> str` using `if/elif/else`.
2. Use `if let` on an `Option[User]` and print the user's name only when present.
3. Use `match` on a `Result` and print either the value or the error.
4. Write a `while let` loop that consumes messages until a channel closes.
5. Loop over a list and stop early with `break`.
6. Write a `loop:` expression that returns an `int` with `break <value>`.

??? example "One possible solution"

    ```incan
    --8<-- "_snippets/language/examples/verified_control_flow_solution.incn"
    ```

## Where to learn more

- Control flow overview: [Control flow](../../explanation/control_flow.md)
- Enums (often used with `match`): [Enums](../../explanation/enums.md)
- Error handling (deep dive on `Result`/`Option`): [Error Handling](../../explanation/error_handling.md)

<nav class="inc-prev-next" aria-label="Book chapter navigation"><a href="../03_functions/"><small>Previous chapter</small><strong>← 3. Functions</strong></a><a href="../05_modules_and_imports/"><small>Next chapter</small><strong>5. Modules and imports →</strong></a></nav>
