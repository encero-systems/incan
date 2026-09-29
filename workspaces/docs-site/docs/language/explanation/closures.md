# Closures (Arrow Functions)

Incan supports anonymous functions using arrow syntax, inspired by Rust and JavaScript (rather than Python).

## Incan Arrow Syntax

```incan
# No parameters (parentheses required)
get_value = () => 42

# Single parameter (parentheses required)
double: (int) -> int = (x) => x * 2

# Multiple parameters
add: (int, int) -> int = (x, y) => x + y

# With expressions
is_positive: (int) -> bool = (n) => n > 0
```

## Named function references

Named functions defined with `def` are also first-class values — you can pass them by name without wrapping in a closure:

```incan
def double(x: int) -> int:
    return x * 2

def apply(f: Callable[int, int], x: int) -> int:
    return f(x)

result = apply(double, 5)      # pass by name — no closure needed
transform = double             # store in a variable
ops: list[(int) -> int] = [double, (x) => x + 1]   # mix named functions and closures
```

Both forms are interchangeable wherever a function type is expected, as long as the closure captures nothing. A closure that reads a local of the function around it carries that value with it, so it has a type of its own. It fits where it is called, such as the callback of a `Result` combinator or a parameter its function only calls, and where a slot keeps that type, such as a new local or a function's one `return`. A slot that holds a plain function pointer has no room for the captured values, so it refuses the closure ([Closures that capture local values](../reference/functions.md#closures-that-capture-local-values)). Use a direct reference when the function already exists; use a closure for short inline logic.

## When to use closures

Closures are ideal for:

- Short inline functions passed to higher-order functions
- Callbacks
- Simple transformations in comprehensions or map/filter operations

```incan
# Good use of closure
numbers = [1, 2, 3, 4, 5]
doubled = numbers.iter().map((x) => x * 2).collect()
```

For complex logic with multiple statements, prefer named functions:

```incan
# Better as a named function
def process_user(user: User) -> Result[str, str]:
    if not user.is_active:
        return Err("User inactive")
    # ... more logic
    return Ok(user.name)
```

## Type Inference

A closure's parameters carry no annotations. Each takes its type from the function type the closure's context gives it: the annotation of the binding it is assigned to, the parameter it is passed to, or the callback of an iterator method, whose parameter is the iterator's element:

```incan
double: (int) -> int = (x) => x * 2                    # x is int: the annotation
result = apply((x) => x + 1, 5)                        # x is int: apply's parameter is (int) -> int
lengths = names.iter().map((name) => len(name))        # name is str: names is a list[str]
total = numbers.iter().fold(0, (acc, n) => acc + n)    # acc and n are int: the start value and the element
```

A generic function's function-typed parameter gives its types once the other arguments fix the type parameters: with `def apply_twice[T](f: (T) -> T, value: T) -> T`, `apply_twice((x) => x + 1, 3)` checks `x` as `int`.

A closure bound to a new name without an annotation has no such context, so its parameters have no type to check an operation against. Annotate the binding with a function type, or use a named function.

!!! note "Why Not Python's `lambda`?"
    Python's lambda syntax has limitations:

    ```python
    # Python lambda - single expression only, awkward syntax
    add = lambda x, y: x + y
    square = lambda x: x ** 2
    ```

    We deliberately chose **not** to include Python-style `lambda` in Incan for several reasons:

    1. **Readability** — `lambda x, y: x + y` is less clear than `(x, y) => x + y`
    2. **Backend alignment** — the current backend lowers closures to Rust-style closure expressions
    3. **Modern syntax** — Arrow functions are familiar from JavaScript, TypeScript, and Rust
    4. **Visual distinction** — The `=>` arrow clearly separates parameters from body

    *Comparison*

    | Python               | Incan             | Rust (generated) |
    | -------------------- | ----------------- | ---------------- |
    | `lambda: 42`         | `() => 42`        | `\|\| 42`        |
    | `lambda x: x * 2`    | `(x) => x * 2`    | `\|x\| x * 2`    |
    | `lambda x, y: x + y` | `(x, y) => x + y` | `\|x, y\| x + y` |

    *Key Differences from Python*

    1. **Parentheses always required** — Even single parameters: `(x) => x + 1`, not `x => x + 1`
    2. **Arrow syntax** — Uses `=>` instead of `:`
    3. **No `lambda` keyword** — The parentheses and arrow are sufficient

## How a closure captures outer locals

A closure reads an outer local as the value that local holds when the closure is constructed. When code after the closure also needs that local, such as a later statement of the same block or the next pass of an enclosing loop, the closure receives its own snapshot of the value, so a later change to the outer binding does not reach the closure. For the same reason a closure does not change a local it reads: the change would land in the closure's copy, not in the local, so it is refused. The exact rule is in [Closure captures](../reference/functions.md#closure-captures).
