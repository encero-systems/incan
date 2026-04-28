# Enums in Incan

Enums in Incan are **algebraic data types** (ADTs): a type with a **closed set** of variants, where each variant can carry
different data.

You use enums when a value can be **one of a few well-defined shapes** and you want the compiler to enforce that you handle every case.

Enums can also own behavior. Put methods and associated functions in the enum body when the behavior belongs to the closed set itself, and use `with TraitName` when the enum should participate in the same trait-based protocols as models and classes.

??? info "Coming from Python?"
    Python’s `Enum` is mainly “named constants”. When Python code needs variants *with data* it often ends up using class hierarchies and `isinstance(...)` checks, which are not exhaustive and are easy to break during refactors.

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

## Declaration shape

The optional `with` clause belongs on the enum header:

```incan
enum ResultState with Describable:
    Success
    Failure(str)
```

For generic enums, put `with` after the type parameters:

```incan
enum Maybe[T] with Describable:
    Some(T)
    None
```

For value enums, put `with` after the backing type:

```incan
enum Environment(str) with Describable:
    Development = "development"
    Production = "production"
```

Inside the enum body, declare variants first and then methods. After the first method declaration, the rest of the body is method territory; do not add more variants below methods. That keeps the visual shape predictable: data cases first, behavior second.

---

## Value enums

Use a value enum when each variant needs one canonical external `str` or `int` representation:

```incan
enum Environment(str):
    Development = "development"
    Production = "production"

enum HttpStatus(int):
    Ok = 200
    NotFound = 404
```

Value enum variants are still enum values, not primitive values. `Environment.Production` has type `Environment`, not `str`, and `HttpStatus.NotFound` has type `HttpStatus`, not `int`.

Value enums gain two helper methods that cover both directions between typed enum variants and their external values, `value` and `from_value`; `value()` returns the backing primitive type. `from_value(...)` accepts the backing primitive type and returns `Option[Enum]` so unknown external values are explicit.

For example:

```incan
def status_code(status: HttpStatus) -> int:
    return status.value()

def parse_status(code: int) -> Option[HttpStatus]:
    return HttpStatus.from_value(code)
```

Value enums are for closed value tables, not for variants that carry additional data. Each variant is a single named case with one raw `str` or `int` value:

```incan
enum Environment(str):
    Development = "development"
    Production = "production"
```

If a variant needs payload fields, use a regular enum instead:

```incan
enum JobState:
    Queued
    Running(str)      # worker id
    Failed(str, int)  # message, retry count
```

Value enums also cannot be generic. The backing value table is concrete, so every variant value must be explicit, must match the declared backing type, and must be unique within the enum.

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

## Methods and associated functions

Enum bodies may declare methods after their variants. Use instance methods when the operation depends on the selected variant:

```incan
enum Direction:
    North
    South
    East
    West

    def is_horizontal(self) -> bool:
        match self:
            Direction.East => return true
            Direction.West => return true
            _ => return false

    def opposite(self) -> Direction:
        match self:
            Direction.North => return Direction.South
            Direction.South => return Direction.North
            Direction.East => return Direction.West
            Direction.West => return Direction.East
```

Call enum methods on enum values:

```incan
dir = Direction.East

if dir.is_horizontal():
    println("moving sideways")
```

Methods may use the enum's type parameters. This is useful for small helpers on `Option`-like enums:

```incan
enum Maybe[T]:
    Some(T)
    None

    def unwrap_or(self, fallback: T) -> T:
        match self:
            Maybe.Some(value) => return value
            Maybe.None => return fallback
```

Use associated functions for constructors or helpers that belong to the enum type rather than a particular instance. An associated enum function has no `self` receiver and is called through the enum type:

```incan
enum Direction:
    North
    South
    East
    West

    def default() -> Self:
        return Direction.North
```

Call it with type-name method syntax:

```incan
dir = Direction.default()
```

You can also mark no-receiver helpers with `@staticmethod` when that makes the intent clearer:

```incan
enum Direction:
    North
    South
    East
    West

    @staticmethod
    def all() -> list[Direction]:
        return [Direction.North, Direction.South, Direction.East, Direction.West]
```

Enum methods follow the same receiver model as methods on models and classes. They do not change matching or construction semantics; variants are still the closed set of cases, and `match` remains exhaustive.

Rules to keep in mind:

- Instance methods take `self` or `mut self`.
- Associated functions take no `self` receiver and are called as `EnumName.method(...)`.
- `Self` means the declaring enum type, including active generic type arguments.
- Variants remain constructors or values; adding methods does not make an enum open-ended.

---

## Trait adoption

Enums can adopt traits with `with`:

```incan
trait Describable:
    def describe(self) -> str: ...

enum BuildState with Describable:
    Queued
    Running(str)
    Failed(str)

    def describe(self) -> str:
        match self:
            BuildState.Queued => return "queued"
            BuildState.Running(worker) => return f"running on {worker}"
            BuildState.Failed(message) => return f"failed: {message}"
```

This uses the same trait-adoption surface as models and classes. The enum must provide the required trait behavior, and values of the enum are accepted where the adopted trait is expected:

```incan
def log_state(state: Describable) -> None:
    println(state.describe())

log_state(BuildState.Queued)
```

Explicit enum adoption also satisfies generic trait bounds:

```incan
def keep_describable[T with Describable](value: T) -> T:
    return value

state = keep_describable(BuildState.Queued)
```

This matters when a library API is written against a reusable capability instead of one concrete enum type.

Value enums can adopt traits too:

```incan
trait ExternalValue:
    def external(self) -> str: ...

enum Environment(str) with ExternalValue:
    Development = "development"
    Production = "production"

    def external(self) -> str:
        return self.value()
```

Trait adoption is additive. An enum without a `with` clause behaves exactly like before, and adopting a trait does not make the enum open-ended; its variants remain closed.

Rules to keep in mind:

- Required trait methods must be implemented in the enum body.
- Trait methods with default bodies can be inherited when their requirements are satisfied.
- Generic traits use the same syntax as other adopters: `enum Lookup with Index[str, int]:`.
- Traits that require adopter fields with `@requires(...)` are usually a model/class fit; enum variant payloads are not shared fields on the enum itself.

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

Incan enums are **algebraic types** — each variant is a fixed tag, optionally carrying data. Ordinary enums are **not** key-value mappings or integer-valued constants. If you need one canonical string or integer representation per variant, use a [value enum](#value-enums).

The compiler will catch the mistake early with a targeted error message:

```incan
# Rejected: ordinary enums cannot have mapped values.
enum Categories:
    GROCERIES => Category("Groceries")   # "cannot have mapped values"

# Rejected: variants cannot contain dotted names.
enum FlowType:
    Cash.Inflow                           # "cannot contain dots"

# Rejected: ordinary enums cannot assign raw values.
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
| Behavior tied to a closed set          | ✓    |       |       |
| Trait adoption with `with`             | ✓    | ✓     | ✓     |
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

| Concept       | Description                                  |
| ------------- | -------------------------------------------- |
| `enum`        | Define a type with fixed variants            |
| Variants      | Each case of an enum, optionally with data   |
| Value enum    | Enum with canonical `str` / `int` raw values |
| Generic enum  | Enum parameterized over types: `Option[T]`   |
| Methods       | Behavior declared inside the enum body       |
| `with Trait`  | Trait adoption for enum values               |
| `match`       | Exhaustive pattern matching on enums         |
| Destructuring | Extract data from variants: `Some(x) =>`     |

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
