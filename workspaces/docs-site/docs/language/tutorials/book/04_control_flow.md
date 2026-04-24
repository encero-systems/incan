# 4. Control flow

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

## `if let` (do something only when one pattern matches)

Use `if let` when you care about exactly one successful pattern and want the non-match case to do nothing.

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

## `match` (pattern matching)

`match` is the main way to branch on enums like `Result` and `Option`:

```incan
def main() -> None:
    result = parse_port("8080")

    match result:
        case Ok(port): println(f"port={port}")
        case Err(e): println(f"error: {e}")
```

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

## Try it

1. Write a function `classify(n: int) -> str` using `if/elif/else`.
2. Use `if let` on an `Option[User]` and print the user's name only when present.
3. Use `match` on a `Result` and print either the value or the error.
4. Write a `while let` loop that consumes messages until a channel closes.
5. Loop over a list and stop early with `break`.

??? example "One possible solution"

    ```incan
    # 1) classify function
    def classify(n: int) -> str:
        if n < 0:
            return "negative"
        elif n == 0:
            return "zero"
        else:
            return "positive"

    def main() -> None:
        println(classify(-1))  # negative
        println(classify(0))   # zero
        println(classify(2))   # positive

        # 2) if let on Option
        maybe_name = Some("Danny")
        if let Some(name) = maybe_name:
            println(name)

        # 3) match on Result
        match parse_port("8080"):
            Ok(port) => println(f"port={port}")
            Err(e) => println(f"error={e}")

        # 4) while let on a sequence of optional values
        def next_value(values: list[Option[int]], idx: int) -> Option[int]:
            if idx < len(values):
                return values[idx]
            return None

        values = [Some(1), Some(2), None]
        idx = 0
        while let Some(value) = next_value(values, idx):
            println(value)
            idx += 1

        # 5) loop over a list and stop early with break
        items = ["Alice", "Bob", "Cara"]
        for name in items:
            if name == "Bob":
                break
            println(name)
    ```

## Where to learn more

- Control flow overview: [Control flow](../../explanation/control_flow.md)
- Enums (often used with `match`): [Enums](../../explanation/enums.md)
- Error handling (deep dive on `Result`/`Option`): [Error Handling](../../explanation/error_handling.md)

## Next

Back: [3. Functions](03_functions.md)

Next chapter: [5. Modules and imports](05_modules_and_imports.md)
