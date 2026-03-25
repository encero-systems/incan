# 3. Functions

Functions are named, reusable blocks of code.

## Defining a function

Function parameters and return types are explicit:

```incan
def add(a: int, b: int) -> int:
    return a + b
```

- Parameters use `name: Type`.
- Return type uses `-> Type`.
- Use `-> None` for “returns nothing”.

## Calling a function

### Program entry point: `main`

Most runnable programs define a `main` function. When you run a file with `incan run ...`, execution starts at:

- `def main() -> None:`
- `-> None` means it doesn’t return a value.

In order to run an incan file, you must define a `main` function:

```incan
def main() -> None:
    total = add(2, 3)
    println(f"total={total}")
```

!!! tip "Coming from Python?"
    In Python, a common pattern is:

    ```python
    if __name__ == "__main__":
        main()
    ```

    In Incan, `main` is the program entry point when you run a file (e.g. `incan run ...`), so you don’t need an
    `__name__` guard - it's implicit in Incan. It is, however, still good practice to keep “do work” code inside `main`,
    and keep other files as imported helper modules.

## Docstrings

Use docstrings to describe intent (especially for public helpers):

```incan
def normalize_name(name: str) -> str:
    """
    Normalize a user name for consistent comparisons.
    """
    return name.strip().lower()
```

## Multiple returns (with `Result`)

Many “can fail” functions return `Result[T, E]` instead of throwing exceptions:

```incan
def parse_port(s: str) -> Result[int, str]:
    if len(s.strip()) == 0:
        return Err("port must not be empty")
    return Ok(int(s))
```

You’ll learn the `Result` pattern in Chapter 6.

## Try it

1. Write `def is_even(n: int) -> bool` and print the result for a few values.
2. Write `def greet(name: str) -> str` that trims whitespace and returns `"Hello, <name>!"`.
3. (Stretch) Write `def safe_div(a: int, b: int) -> Result[float, str]`.

??? example "One possible solution"

    ```incan
    def is_even(n: int) -> bool:
        return n % 2 == 0

    def greet(name: str) -> str:
        cleaned = name.strip()
        return f"Hello, {cleaned}!"

    def safe_div(a: float, b: float) -> Result[float, str]:
        if b == 0.0:
            return Err("division by zero")
        return Ok(a / b)

    def main() -> None:
        println(f"is_even(2)={is_even(2)}")
        println(f"is_even(3)={is_even(3)}")
        println(greet("  Alice  "))
    ```

## Functions as values

Named functions are first-class values — you can pass them by name to other functions, store them in variables, or put them in collections:

```incan
def double(x: int) -> int:
    return x * 2

def apply(f: (int) -> int, x: int) -> int:
    return f(x)

result = apply(double, 5)   # → 10
```

You'll explore this more in the [Closures](../../explanation/closures.md) chapter.

## What to learn next

- Function definitions and signatures: [Language reference (generated)](../../reference/language.md#builtin-functions)
- Function scoping and name lookup: [Scopes & Name Resolution](../../explanation/scopes_and_name_resolution.md)
- Closures and higher-order patterns: [Closures](../../explanation/closures.md)

## Next

Back: [2. Values, variables, and types](02_values_variables_and_types.md)

Next chapter: [4. Control flow](04_control_flow.md)
