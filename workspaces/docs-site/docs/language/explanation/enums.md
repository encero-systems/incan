# Enums in Incan

Enums in Incan are **algebraic data types** (ADTs): a type with a **closed set** of variants, where each variant can carry
different data.

You use enums when a value can be **one of a few well-defined shapes** and you want the compiler to enforce that you handle
every case.

??? info "Coming from Python?"
    Python’s `Enum` is mainly “named constants”. When Python code needs variants *with data* it often ends up using class
    hierarchies and `isinstance(...)` checks, which are not exhaustive and are easy to break during refactors.

    Here’s one representative before/after:

    **Python (common workaround)**:

    ```python
    class Shape:
        pass

    class Circle(Shape):
        def __init__(self, radius: float):
            self.radius = radius

    class Rectangle(Shape):
        def __init__(self, width: float, height: float):
            self.width = width
            self.height = height

    def area(shape: Shape) -> float:
        if isinstance(shape, Circle):
            return 3.14159 * shape.radius * shape.radius
        elif isinstance(shape, Rectangle):
            return shape.width * shape.height
        raise ValueError("unknown shape")
    ```

    > Note: Python 3.10+ has `match`/`case`, but it still won’t enforce exhaustiveness the way Incan does.

    **Incan (enum + exhaustive `match`)**:

    ```incan
    enum Shape:
        Circle(float)
        Rectangle(float, float)

    def area(shape: Shape) -> float:
        match shape:
            Circle(r) => return 3.14159 * r * r
            Rectangle(w, h) => return w * h
    ```

    If you add a new variant later, the compiler will point you at the `match` sites that must be updated.

??? info "Coming from Rust?"
    This is the same concept as Rust’s `enum` + exhaustive `match` (sum types with payload-carrying variants).

    Differences are mostly surface syntax:
    - Variants are declared in an indented block under `enum` (no braces/commas).
    - Construction uses Incan’s syntax (e.g. `Status.Active` / `Message.Move(1, 2)`), rather than Rust’s `Type::Variant(...)`.

## A motivating example

```incan
enum Shape:
    Circle(float)          # radius
    Rectangle(float, float)  # width, height
    Triangle(float, float)   # base, height

def area(shape: Shape) -> float:
    match shape:
        Circle(r) => return 3.14159 * r * r
        Rectangle(w, h) => return w * h
        Triangle(b, h) => return 0.5 * b * h
```

If you add a new variant later, the compiler will point you at the `match` sites that must be updated.

## Basic syntax

### Simple Enum (No Data)

```incan
enum Status:
    Pending
    Active
    Completed
    Cancelled
```

Usage:

```incan
status = Status.Active

match status:
    Pending => println("Waiting...")
    Active => println("In progress")
    Completed => println("Done!")
    Cancelled => println("Aborted")
```

### Enum with Data (Variants)

Each variant can carry different types and amounts of data:

```incan
enum Message:
    Quit                           # No data
    Move(int, int)                 # Two ints (x, y)
    Write(str)                     # A string
    ChangeColor(int, int, int)     # RGB values
```

Usage:

```incan
msg = Message.Move(10, 20)

match msg:
    Quit => println("Goodbye")
    Move(x, y) => println(f"Moving to ({x}, {y})")
    Write(text) => println(f"Message: {text}")
    ChangeColor(r, g, b) => println(f"RGB({r}, {g}, {b})")
```

---

## Generic enums

Enums can be generic — parameterized over types:

```incan
enum Option[T]:
    Some(T)
    None

enum Result[T, E]:
    Ok(T)
    Err(E)
```

> Note: These are Incan's built-in types for handling optional values and errors.

### Custom Generic Enum

```incan
enum Tree[T]:
    Leaf(T)
    Node(Tree[T], Tree[T])

# A binary tree of integers
tree = Node(
    Leaf(1),
    Node(Leaf(2), Leaf(3))
)
```

---

## Pattern matching

The `match` expression is how you work with enums. It's exhaustive — the compiler ensures you handle all variants.

### Basic Match

```incan
enum Direction:
    North
    South
    East
    West

def describe(dir: Direction) -> str:
    match dir:
        North => return "Going up"
        South => return "Going down"
        East => return "Going right"
        West => return "Going left"
```

### Extracting Data

```incan
enum ApiResponse:
    Success(str, int)        # (data, status_code)
    Error(str)               # error message
    Loading

def handle(response: ApiResponse) -> None:
    match response:
        Success(data, code) =>
            println(f"Got {code}: {data}")
        Error(msg) =>
            println(f"Failed: {msg}")
        Loading =>
            println("Please wait...")
```

### Wildcard Pattern

Use `_` to match any remaining variants:

```incan
match status:
    Active => println("Working on it")
    _ => println("Not active")  # Matches Pending, Completed, Cancelled
```

> **Warning**: Wildcards can hide bugs when you add new variants. Prefer explicit matches.

### Guards

Add conditions to patterns:

```incan
enum Temperature:
    Celsius(float)
    Fahrenheit(float)

def describe(temp: Temperature) -> str:
    match temp:
        Celsius(c) if c > 30 => return "Hot (Celsius)"
        Celsius(c) if c < 10 => return "Cold (Celsius)"
        Celsius(_) => return "Moderate (Celsius)"
        Fahrenheit(f) if f > 86 => return "Hot (Fahrenheit)"
        Fahrenheit(f) if f < 50 => return "Cold (Fahrenheit)"
        Fahrenheit(_) => return "Moderate (Fahrenheit)"
```

---

## Common patterns

For practical recipes (state machines, commands, error types, expression trees), see:

- [Modeling with enums](../how-to/modeling_with_enums.md)

---

## Derives and docstrings

Enums support `@derive(...)` decorators and docstrings:

```incan
@derive(Serialize, Deserialize)
enum Status:
    """Represents the current state of a task."""
    Pending
    Active
    Completed
```

For serialization details, see [Derives: Serialization](../reference/derives/serialization.md).

---

## Common pitfall: enums are not lookup tables

Incan enums are **algebraic types** — each variant is a fixed tag, optionally carrying data.
They are **not** key-value mappings or integer-valued constants.

The compiler will catch the mistake early with a targeted error message:

```incan
# ✗ These are all rejected with clear diagnostics:
enum Categories:
    GROCERIES => Category("Groceries")   # "cannot have mapped values"

enum FlowType:
    Cash.Inflow                           # "cannot contain dots"

enum Color:
    Red = 1                               # "cannot have assigned values"
```

**Instead**, use plain variants for the enum and a separate model for rich data:

```incan
enum CategoryKey:
    Groceries
    Utilities

model Category:
    key: CategoryKey
    description: str

def all_categories() -> list[Category]:
    return [
        Category(key=CategoryKey.Groceries, description="Food items"),
        Category(key=CategoryKey.Utilities, description="Gas, electric"),
    ]
```

---

## Enums vs models vs classes

| Use Case                               | Enum | Model | Class |
| -------------------------------------- | ---- | ----- | ----- |
| Fixed set of variants                  | ✓    |       |       |
| Data that can be one of several shapes | ✓    |       |       |
| Exhaustive handling required           | ✓    |       |       |
| Simple data container (DTO, config)    |      | ✓     |       |
| Serialization (`@derive`)              | ✓    | ✓     |       |
| Validation and defaults                |      | ✓     |       |
| Inheritance/polymorphism needed        |      |       | ✓     |
| Mutable state with methods             |      |       | ✓     |
| Open extension (new types later)       |      |       | ✓     |

```incan
# Enum: closed set, exhaustive matching
enum PaymentMethod:
    CreditCard(str, str)     # number, expiry
    PayPal(str)              # email
    BankTransfer(str, str)   # account, routing

# Model: data-first, serialization
@derive(Serialize, Deserialize)
model PaymentRequest:
    method: PaymentMethod
    amount: float
    currency: str = "USD"

# Class: behavior-first, inheritance
class PaymentProcessor:
    def process(self, amount: float) -> Result[Receipt, Error]:
        ...
```

See also: [Models and Classes Guide](./models_and_classes/index.md)

---

## Built-in enums

Incan provides these enums in the standard library:

### Option[T]

Represents an optional value:

```incan
enum Option[T]:
    Some(T)
    None
```

See: [Error Handling Guide](./error_handling.md)

### Result[T, E]

Represents success or failure:

```incan
enum Result[T, E]:
    Ok(T)
    Err(E)
```

See: [Error Handling Guide](./error_handling.md)

### Ordering

Comparison result:

```incan
enum Ordering:
    Less
    Equal
    Greater
```

## Summary

| Concept       | Description                                |
| ------------- | ------------------------------------------ |
| `enum`        | Define a type with fixed variants          |
| Variants      | Each case of an enum, optionally with data |
| Generic enum  | Enum parameterized over types: `Option[T]` |
| `match`       | Exhaustive pattern matching on enums       |
| Destructuring | Extract data from variants: `Some(x) =>`   |

Enums are one of Incan's most powerful features — use them for:

- Modeling states and state machines
- Error types with rich context
- Command/message types
- Any "one of these things" scenario

The compiler guarantees you handle all cases, eliminating a whole class of bugs caused by missing or forgotten cases.

---

## See Also

- [Error Handling](./error_handling.md) — Using `Result` and `Option`
- Match expressions: see the language docs and examples in this section
- [Models and Classes](./models_and_classes/index.md) — When to use class vs enum

--8<-- "_snippets/rfcs_refs.md"
