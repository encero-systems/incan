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

    In Incan, `main` is the program entry point when you run a file (e.g. `incan run ...`), so you don’t need an `__name__` guard - it's implicit in Incan. It is, however, still good practice to keep “do work” code inside `main`, and keep other files as imported helper modules.

## Variadic arguments

Most functions should name every parameter explicitly. Use variadic arguments when the API really wants "zero or more of the same kind of thing" and making callers build a list or dictionary by hand would make ordinary calls noisier.

The mental model is:

- The caller may write many arguments.
- The function receives one typed container.
- The annotation is the item type, not the container type.

### `*args`: many positional values, one list

Use `*name: T` when the extra positional values are all the same conceptual kind. Inside the function, `name` is a `List[T]`.

```incan
def sum_all(*values: int) -> int:
    mut total: int = 0
    for value in values:
        total = total + value
    return total

def main() -> None:
    println(f"sum={sum_all(1, 2, 3)}")
```

This is useful for helpers like `sum_all(1, 2, 3)`, `log("started", "ready")`, or future library APIs that accept any number of already-packaged values.

### `**kwargs`: many named values, one dictionary

Use `**name: T` when the function intentionally accepts an open set of same-typed named options. Inside the function, `name` is a `Dict[str, T]`.

```incan
def count_headers(path: str, **headers: str) -> int:
    return len(headers)

def main() -> None:
    count = count_headers("/status", accept="json", trace="enabled")
    println(f"headers={count}")
```

This is a good fit for boundary-style APIs: HTTP headers, labels, metadata, or adapter options where the valid keys may come from another system. It is not a replacement for normal parameters when the names are known and required.

!!! tip "Coming from Python?"
    Incan's spelling is intentionally familiar, but the types are stricter. Python `*args` collects a tuple and `**kwargs` collects a dict. Incan `*values: int` collects a `List[int]`, and `**headers: str` collects a `Dict[str, str]`.

    Python often uses `**kwargs` as a flexible "anything goes" escape hatch. Incan does not: every captured keyword value must match the declared value type.

    Python also unpacks arbitrary iterables and mappings at runtime. Incan keeps the same surface spelling, but the
    compiler must know what the unpacked value can provide. A `List[int]` can feed a positional rest parameter, while an
    inline `[1, 2]` can also prove the two values needed by a fixed call like `point(*[1, 2])`.

### Combining both forms

You can combine `*args` and `**kwargs` when the API has both repeated positional data and open named metadata:

```incan
def summarize(title: str, *values: int, **labels: str) -> int:
    mut total: int = 0
    for value in values:
        total = total + value
    return total

def main() -> None:
    extra = [2, 3]
    labels = {"source": "demo"}
    total = summarize("numbers", 1, *extra, kind="example", **labels)
    println(f"total={total}")
```

### Unpacking existing values

If you already have a list, use `*extra` to feed it into a positional rest parameter. If you already have a dictionary, use `**labels` to feed it into a keyword rest parameter.

- For rest calls, `*extra` requires a callee with a `*` rest parameter and `extra` must be compatible with `List[T]`.
- For rest calls, `**labels` requires a callee with a `**` rest parameter and `labels` must be compatible with `Dict[str, T]`.
- For fixed-parameter calls, the compiler must prove the unpacked length or key set before it can lower the call.

This works through function values too, as long as the value comes from a rest-aware function:

```incan
def collect(prefix: str, *items: int, **labels: str) -> int:
    return len(items) + len(labels)

def main() -> None:
    f = collect
    xs = [1, 2]
    labels = {"kind": "demo"}
    count = f("event", 0, *xs, **labels)
    println(f"count={count}")
```

Unpacking is intentionally static. It can also bind ordinary fixed parameters when the compiler can prove the unpacked shape:

```incan
def point(x: int, y: int) -> int:
    return x + y

def route(path: str, method: str) -> str:
    return f"{method} {path}"

def main() -> None:
    println(f"point={point(*[1, 2])}")
    println(route(**{"path": "/status", "method": "GET"}))
```

A plain `List[int]` variable does not prove a fixed length, and a plain `Dict[str, str]` variable does not prove that specific fixed keys exist. Use those values with rest parameters, or keep the fixed call explicit.

The same spelling works when building new collections. Use `*` in a list literal and `**` in a dictionary literal:

```incan
def main() -> None:
    middle = [2, 3]
    values = [1, *middle, 4]

    defaults = {"trace": "off"}
    headers = {**defaults, "trace": "enabled"}

    println(f"values={len(values)} headers={len(headers)}")
```

`[**labels]` is not valid because `**` is for mapping or keyword unpacking, not list expansion. `{*items}` is not valid as dictionary spread; dictionary spread uses `**items`.

### When not to use variadic arguments

Prefer ordinary parameters when the function has a small, known contract:

```incan
def connect(host: str, port: int) -> str:
    return f"{host}:{port}"
```

Prefer a model when options have different value types or deserve documentation:

```incan
model RequestOptions:
    timeout_ms: int
    retry: bool

def request(path: str, options: RequestOptions) -> int:
    return options.timeout_ms
```

Prefer packaging repeated heterogeneous data into one type before making it variadic:

```incan
model Label:
    name: str
    value: str

def emit(*labels: Label) -> int:
    return len(labels)
```

Normal parameters must come before rest parameters, `**kwargs` must be last, and each function can have at most one `*args` and one `**kwargs` parameter. For the full binding and lowering rules, see [Functions and calls](../../reference/functions.md).

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
- Rest parameters and call binding: [Functions and calls](../../reference/functions.md)
- Function scoping and name lookup: [Scopes & Name Resolution](../../explanation/scopes_and_name_resolution.md)
- Closures and higher-order patterns: [Closures](../../explanation/closures.md)

## Next

Back: [2. Values, variables, and types](02_values_variables_and_types.md)

Next chapter: [4. Control flow](04_control_flow.md)
