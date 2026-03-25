# Closures (Arrow Functions)

Incan supports anonymous functions using arrow syntax, inspired by Rust and JavaScript (rather than Python).

## Incan Arrow Syntax

```incan
# No parameters (parentheses required)
get_value = () => 42

# Single parameter (parentheses required)
double = (x) => x * 2

# Multiple parameters  
add = (x, y) => x + y

# With expressions
is_positive = (n) => n > 0
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
ops = [double, (x) => x + 1]   # mix named functions and closures
```

Both forms are interchangeable wherever a function type is expected. Use a direct reference when the function already exists; use a closure for short inline logic.

## When to use closures

Closures are ideal for:

- Short inline functions passed to higher-order functions
- Callbacks
- Simple transformations in comprehensions or map/filter operations

```incan
# Good use of closure
numbers = [1, 2, 3, 4, 5]
doubled = numbers.map((x) => x * 2)
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

Closure parameters use type inference — the compiler determines types from context:

```incan
# Types inferred from usage
add = (x, y) => x + y
result = add(3, 4)  # x and y inferred as int
```

For explicit typing, use a named function instead.

!!! note "Why Not Python's `lambda`?"
    Python's lambda syntax has limitations:

    ```python
    # Python lambda - single expression only, awkward syntax
    add = lambda x, y: x + y
    square = lambda x: x ** 2
    ```

    We deliberately chose **not** to include Python-style `lambda` in Incan for several reasons:

    1. **Readability** — `lambda x, y: x + y` is less clear than `(x, y) => x + y`
    2. **Consistency with Rust** — Incan compiles to Rust, which uses `|x, y| expr` closures
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
