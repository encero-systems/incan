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
2. Use `match` on a `Result` and print either the value or the error.
3. Loop over a list and stop early with `break`.
4. Write a `loop:` expression that returns an `int` with `break <value>`.

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

        # 2) match on Result
        match parse_port("8080"):
            Ok(port) => println(f"port={port}")
            Err(e) => println(f"error={e}")

        # 3) loop over a list and stop early with break
        items = ["Alice", "Bob", "Cara"]
        for name in items:
            if name == "Bob":
                break
            println(name)

        # 4) loop expression with break value
        value = loop:
            if len(items) > 0:
                break 42
            break 0
        println(value)
    ```

## Where to learn more

- Control flow overview: [Control flow](../../explanation/control_flow.md)
- Enums (often used with `match`): [Enums](../../explanation/enums.md)
- Error handling (deep dive on `Result`/`Option`): [Error Handling](../../explanation/error_handling.md)

## Next

Back: [3. Functions](03_functions.md)

Next chapter: [5. Modules and imports](05_modules_and_imports.md)
