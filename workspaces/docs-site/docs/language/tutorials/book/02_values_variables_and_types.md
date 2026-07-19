# 2. Values, variables, and types

<div class="inc-book-progress" aria-label="Chapter 2 of 13"><div class="inc-book-progress__meta"><strong>Chapter 2 of 13</strong><span>Values, variables, and types</span></div><div class="inc-book-progress__bar" aria-hidden="true"><span style="--inc-progress: 15.4%"></span></div></div>

This chapter introduces basic values and types.

If you get stuck on terminology, see: [Glossary](../../reference/glossary.md).

## Basic values

Common built-in types:

- `int` (ordinary integers)
- `float` (ordinary binary floating point)
- `bool` (`true` / `false`)
- `str` (strings)

Incan also has exact numeric types for data and Rust interop boundaries, such as `i32`, `u64`, `usize`, `f32`, and `f64`, plus fixed-scale decimal types such as `decimal[10, 2]`. You usually do not need those in beginner code, but they matter when another system owns the width or precision.

For the full rules, see [Choosing numeric types](../../how-to/choosing_numeric_types.md), [Numeric semantics](../../reference/numeric_semantics.md), and [Why numeric types work this way](../../explanation/numeric_types.md).

## Variables

Assign a value to a name:

!!! tip "Coming from Python?"
    Incan supports Python-style f-strings for string interpolation:

    ```incan
    name = "Incan"
    println(f"hello, {name}")
    ```

    For more examples and formatting details, see: [Strings](../../reference/strings.md).

```incan
def main() -> None:
    name = "Incan"
    answer = 42
    println(f"{name}: {answer}")
```

## Function arguments and return types

Incan uses type annotations on function arguments and return types:

```incan
def add(a: int, b: int) -> int:
    return a + b
```

## Try it

1. Create a `str`, `int`, `float`, and `bool` variable and print them.
2. Use an f-string with multiple `{...}` interpolations.
3. Call `add(10, 32)` and print the result.

??? example "One possible solution"

    ```incan
    def add(a: int, b: int) -> int:
        return a + b

    def main() -> None:
        name = "Incan"
        count = 3
        ratio = 0.5
        enabled = true

        println(f"name={name} count={count} ratio={ratio} enabled={enabled}")
        println(f"add={add(10, 32)}")
    ```

<nav class="inc-prev-next" aria-label="Book chapter navigation"><a href="../01_hello_world/"><small>Previous chapter</small><strong>← 1. Hello world</strong></a><a href="../03_functions/"><small>Next chapter</small><strong>3. Functions →</strong></a></nav>
