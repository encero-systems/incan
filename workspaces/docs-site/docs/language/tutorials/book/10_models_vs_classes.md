# 10. Models vs classes

Incan has two “types with fields”:

- `model`: data-first (great for DTOs, configs, payloads)
- `class`: behavior-first (methods + possible mutation)

!!! info "This is not just a naming convention"
    `model` and `class` are **fundamentally different**:

    - **Models** define data shapes and support schema-focused features like field metadata/aliases.
    - **Classes** define behavior and support inheritance and method overrides.

## Choosing between `model` and `class`

Start with a `model` when:

- you’re defining a **data shape** (DTOs, configs, payloads)
- you care about **wire/schema mapping** (field aliases/metadata)

Start with a `class` when:

- you’re defining an **object with behavior** (methods are the primary API)
- you need **mutable state** (`mut self`) or **inheritance** (`extends`)

!!! tip "Coming from Python?"
    A `model` is closest to a Python `@dataclass` (or a Pydantic `Basemodel`) in spirit: you declare fields, and you get
    a constructor automatically.

    In Incan you **don’t** write an `__init__`/init method. You construct values with keyword arguments matching the declared
    fields (as shown below), and you can give fields default values in the declaration when needed. For validation or alternate 
    construction, write a separate helper/factory function (often returning `Result`).

## A `model` (data-first)

### A simple model

```incan
model User:
    name: str
    age: int

def main() -> None:
    u = User(name="Alice", age=30)
    println(f"{u.name} age={u.age}")
```

### How models work (what users trip over)

#### Construction is field-based (named args + defaults)

Models are constructed by naming fields. Defaults make a field optional at construction time.

```incan
model Config:
    host: str = "localhost"
    port: int = 8080

def main() -> None:
    c1 = Config()                 # ok (all defaults)
    c2 = Config(port=3000)        # ok
    c3 = Config(host="0.0.0.0")   # ok
```

#### Schema-safe field names (aliases) are model-only

When a wire format uses keywords like `"type"` or `"from"`, keep a safe canonical name in code and map the wire key with
an alias.

```incan
@derive(Serialize, Deserialize)
model Account:
    type_ as "type": str

def demo(a: Account) -> str:
    # In code, use the canonical name.
    return a.type_
```

Classes do **not** support field metadata/aliases. If you need schema mapping, use a `model` (or embed a model in a class).
For the full alias behavior (including where aliases can be written in code), see: [Models](../../explanation/models_and_classes/models.md).

## A `class` (behavior-first)

### A simple class

```incan
class Counter:
    value: int

    def increment(mut self) -> None:
        self.value += 1

def main() -> None:
    c = Counter(value=0)
    c.increment()
    c.increment()
    println(f"value={c.value}")  # outputs: value=2
```

## Try it

1. Create a `model Product` with `name` and `price`.
2. Create a `class Cart` with a list of products and a method to compute a total.
3. Print the total.

??? example "One possible solution"

    ```incan
    model Product:
        name: str
        price: float

    class Cart:
        items: list[Product]

        def total(self) -> float:
            total = 0.0
            for item in self.items:
                total = total + item.price
            return total

    def main() -> None:
        cart = Cart(items=[
            Product(name="Book", price=10.0),
            Product(name="Pen", price=2.5),
        ])
        println(f"total={cart.total()}")
    ```

## Where to learn more

This chapter covers the basics. For the full feature surface—model schema mapping (aliases/metadata), derives
(serialization/validation), reflection (`__fields__()`), and class features like inheritance and trait composition—see the
guides and reference pages below.

- Full guide and decision table: [Models & Classes](../../explanation/models_and_classes/index.md)
- Models (deep dive): [Models](../../explanation/models_and_classes/models.md)
- Classes (deep dive): [Classes](../../explanation/models_and_classes/classes.md)
- Derives (reference): [Derives: Serialization](../../reference/derives/serialization.md) and [Derives: Validation](../../reference/derives/validation.md)
- Reflection (reference): [Reflection](../../reference/reflection.md)
- Error handling (deep dive): [Error handling](../../explanation/error_handling.md)

## Next

Back: [9. Enums and better `match`](09_enums.md)

Next chapter: [11. Traits and derives](11_traits_and_derives.md)
