# Classes

A `class` is Incan’s **behavior-first** type: it models an object with methods, mutable state, and inheritance.

If you’re deciding between `model` and `class`, start with the [Models & classes overview](index.md). This page focuses
on how classes behave once you’ve chosen them.

## Quick start

```incan
class Counter:
    value: int  # (1)

    def get(self) -> int:  # (2)
        return self.value

    def increment(mut self) -> None:  # (3)
        self.value += 1

def main() -> None:
    c = Counter(value=0)  # (4)
    c.increment()
    println(c.get()) # (5)
    println(c.value) # (6)
```

1. Fields define object state. See [Defining fields](#defining-fields).
2. Read-only methods take `self`. See [Methods and receivers (`self` vs `mut self`)](#methods-and-receivers-self-vs-mut-self).
3. Mutating methods take `mut self`. See [Methods and receivers (`self` vs `mut self`)](#methods-and-receivers-self-vs-mut-self).
4. Construction is keyword-only (named arguments). See [Constructing a class](#constructing-a-class).
5. Running a method on an object returns the result of the method. See [Methods and receivers (`self` vs `mut self`)](#methods-and-receivers-self-vs-mut-self).
6. Member access uses canonical field names. See [Field access](#field-access).

The key ideas:

- **A `class` is behavior-first**: methods are the primary API.
- **A `class` can be stateful**: use `mut self` for methods that modify fields.
- **A `class` can inherit**: single inheritance with `extends` for behavior reuse and method overrides.
- **Schema mapping is not a class feature**: classes don’t support field aliases/metadata; use a model for wire mapping.

!!! info "Glossary"
    - **field**: a named piece of state stored on the object (e.g. `value: int`)
    - **method**: a function defined on the class (e.g. `def increment(...)`)
    - **receiver**: the first parameter of a method (`self` or `mut self`)
    - **override**: redefining a method in a child class so the child version is used
    - **composition**: building larger types by putting one value inside another (e.g. a class holding a model)

??? info "Coming from Rust?"
    Incan `class`es compile to Rust `struct`s + `impl`s, but the surface syntax is closer to Python.

    - `def m(self)` corresponds to an immutable receiver (roughly like `&self`)
    - `def m(mut self)` corresponds to a mutable receiver (roughly like `&mut self`)
    - `extends` is compile-time reuse + explicit overrides; it does **not** introduce subtyping

??? info "Coming from Python?"
    - `self` works like Python’s `self`.
    - `mut self` is explicit: methods that modify fields must take `mut self`.
    - If you’re looking for Pydantic-like “data models” (DTOs, wire formats, schema mapping), prefer `model`.

??? info "Coming from TypeScript / JavaScript?"
    - `self` is the object (roughly like JS/TS `this`).
    - Construction is keyword-only (`Point(x=..., y=...)`); there is no `constructor(...)` method.
    - `extends` does **not** introduce subtyping (a child value can’t be used where the parent type is required);
      use traits/enums for polymorphism.

## Defining fields

A `class` declares object state as typed fields.

- Syntax: `name: Type`
- Optional defaults: `name: Type = expr`

Field metadata and aliases (`[alias="..."]`, `[description="..."]`, or `as "..."`) are **not supported** on classes.

```incan
class UserService:
    repo: UserRepository
    logger_name: str
```

## Constructing a class

Class construction is **keyword-only** (named arguments).

```incan
class Point:
    x: int
    y: int

def main() -> None:
    p = Point(x=10, y=20)
```

This keeps call sites explicit and stable as you add/reorder fields.

Constructor keys are the declared field names (including inherited fields).

??? tip "Coming from Python?"
    In Incan you don’t write an `__init__`/init method for classes; the declared fields (including inherited fields)
    define the constructor keys.

Rules:

- **No positional args**: `Point(10, 20)` is not supported.
- **Unknown fields are errors**: `Point(z=1)` is a type error.
- **Duplicates are errors**: `Point(x=1, x=2)` is a type error.
- **Missing required fields are errors**: if a field has no default, you must pass it.

## Field access

Access a field with dot syntax:

```incan hl_lines="3"
def main() -> None:
    c = Counter(value=0)
    println(c.value)
```

Because classes don’t support field aliases/metadata (the way a `model` would), member access always uses the canonical
field name.

## Methods and receivers (`self` vs `mut self`)

Use `self` for read-only methods and `mut self` for methods that modify the object.

```incan
class Counter:
    value: int

    def get(self) -> int:
        return self.value

    def increment(mut self) -> None:
        self.value += 1
```

## Static methods (`@staticmethod`)

--8<-- "_snippets/language/decorators/staticmethod.md"

Another example — a utility method that doesn't need instance state:

```incan
class MathUtils:
    precision: int = 2

    @staticmethod
    def clamp(value: int, low: int, high: int) -> int:
        if value < low:
            return low
        if value > high:
            return high
        return value

def main() -> None:
    println(MathUtils.clamp(15, 0, 10))  # 10
```

See also: [Decorators reference](../../reference/derives_and_traits.md#decorators-staticmethod-requires)

## Inheritance (`extends`) and overrides

Classes support single inheritance:

```incan
class Animal:
    name: str

    def speak(self) -> str:
        return "..."

class Dog extends Animal:
    breed: str

    def speak(self) -> str:
        return "Woof!"
```

A child class constructor includes inherited fields:

```incan
def main() -> None:
    d = Dog(name="Rex", breed="Labrador")  # `name` comes from `Animal`
    println(d.speak())
```

Notes:

- Inheritance is for behavior reuse and method overrides.
- `extends` does **not** introduce subtyping: you cannot use a `Dog` value where an `Animal` value is required.
- Overrides are explicit: if you define a method with the same name in a child class, it overrides the parent method.

If you only need a larger data shape, prefer composition with models. See [Using models inside classes](#using-models-inside-classes-common-pattern).

## Trait composition (`with ...`)

Both a `class` and a `model` can implement traits:

```incan
trait Loggable:
    def log(self, msg: str) -> None: ...

class Service with Loggable:
    name: str

    def log(self, msg: str) -> None:
        println(f"[{self.name}] {msg}")
```

Traits are behavior-only (no storage). A trait default method may assume certain fields exist on the adopter; use
`@requires(...)` to declare that contract in a trait.

See:

- [Traits as language hooks (Explanation)](../traits_as_language_hooks.md)
- [Derives & traits (Reference)](../../reference/derives_and_traits.md)

## Using models inside classes (common pattern)

This is composition: it’s common to use models for data and classes for behavior.

A simple example:

```incan
model AnimalData:
    name: str
    species: str

class AnimalService:
    data: AnimalData

    def describe(self) -> str:
        return f"{self.data.name} ({self.data.species})"
```

A more complex example:

```incan
@derive(Serialize, Deserialize)
model User:
    id: int
    email: str

class UserService:
    users: list[User]

    def get_emails(self) -> list[str]:
        return [u.email for u in self.users]

def main() -> None:
    # defining the users (models)
    alice = User(id=1, email="alice@example.com")
    bob = User(id=2, email="bob@example.com")
    zephod = User(id=42, email="zephod@example.com")

    # defining the service (class)
    service = UserService(users=[alice, bob, zephod])
    
    println(service.get_emails())  # ["alice@example.com", "bob@example.com", "zephod@example.com"]
```

## Reflection helpers

Classes provide:

- `__class_name__() -> str`
- `__fields__() -> FrozenList[FieldInfo]`

Unlike models, classes do not support per-field aliases/metadata, so `FieldInfo.alias` and `FieldInfo.description` are
always `None` and `FieldInfo.wire_name == FieldInfo.name`.

See: [Reflection (Reference)](../../reference/reflection.md)

## Common pitfalls

- **Using a class for schema mapping**: if you need wire keys/aliases, use a model (and embed it in a class if needed).
- **Forgetting `mut self`**: if a method assigns to `self.field`, it must take `mut self`.
- **Using inheritance for data reuse**: prefer composition for data shapes; use `extends` for behavior reuse and overrides.
