# Derives and Traits

<!--
Link index

Use reference-style links like:
- [Debug][derive-debug]
- [Error Handling Guide][guide-error-handling]

So we can change the destination in one place if we move/rename sections.
-->

<!-- Built-in derives (anchors in this page) -->
[derive-debug]: #debug-automatic
[derive-display]: #display-custom-with-__str__
[derive-eq]: #eq-equality
[derive-ord]: #ord-ordering
[derive-hash]: #hash
[derive-clone]: #clone
[derive-copy]: #copy
[derive-default]: #default
[derive-serialize]: #serialize
[derive-deserialize]: #deserialize
[derive-validate]: #validate-models-only

<!-- Related sections (anchors in this page) -->
[auto-derives]: #automatic-derives

<!-- Other docs -->
[traits-doc]: #traits-authoring
[guide-error-handling]: ../explanation/error_handling.md

This page is the reference for derives, dunder overrides, method decorators, generic methods and trait authoring.

## Automatic derives

These derives are part of every declaration of the kind, without `@derive(...)`:

| Construct | Automatic derives                                                                                                  |
| --------- | ------------------------------------------------------------------------------------------------------------------ |
| `model`   | `Debug`, `Clone`                                                                                                   |
| `class`   | `Debug`, `Clone`                                                                                                   |
| `enum`    | `Debug`, `Clone`; `PartialEq` when every payload type is a number, `bool`, `str` or `bytes`, or a `list`, `set`, `dict`, `Option`, `Result` or tuple of those |
| `newtype` | `Debug`; `Clone` and `Copy` when the underlying type is `Copy`                                                     |

An automatic derive satisfies a generic bound on the same trait.

## Derive catalog (quick index)

| Derive                            | Provides                              | Dunder     | Detail                                                  |
| --------------------------------- | ------------------------------------- | ---------- | ------------------------------------------------------- |
| [Debug][derive-debug]             | `{value:?}` formatting                | —          | Automatic                                               |
| [Display][derive-display]         | `{value}` formatting, `str(value)`    | `__str__`  | From `__str__`                                          |
| [Eq][derive-eq]                   | `==`, `!=`                            | `__eq__`   | Adds `PartialEq`                                        |
| [Ord][derive-ord]                 | `<`, `<=`, `>`, `>=`                  | `__lt__`   | Adds `Eq`, `PartialEq`, `PartialOrd`                    |
| [Hash][derive-hash]               | Hashing                               | `__hash__` | A `set` element or `dict` key needs `Eq` and `Hash`     |
| [Clone][derive-clone]             | `.clone()`                            | —          | Automatic for model, class and enum                     |
| [Copy][derive-copy]               | Implicit copies                       | —          | Every field type is `Copy`                              |
| [Default][derive-default]         | `Type.default()`                      | —          |                                                         |
| [json][derive-serialize]          | `Serialize` and `Deserialize`         | —          | From `std.serde`                                        |
| [Validate][derive-validate]       | `Type.new(...)` validated construction | —         | Models only                                             |

`PartialEq` and `PartialOrd` may also be requested directly with `@derive(PartialEq)` and `@derive(PartialOrd)`.

Detail pages:

- Derives: [String representation](derives/string_representation.md), [Comparison](derives/comparison.md), [Copying/default](derives/copying_default.md), [Serialization](derives/serialization.md), [Validation](derives/validation.md), [Custom behavior](derives/custom_behavior.md)
- Stdlib traits: [Overview](stdlib_traits/index.md), [Collection protocols](stdlib_traits/collection_protocols.md), [Indexing and slicing](stdlib_traits/indexing_and_slicing.md), [Callable objects](stdlib_traits/callable.md), [Awaitable values](stdlib_traits/awaitable.md), [Operator traits](stdlib_traits/operators.md), [Conversion traits](stdlib_traits/conversions.md)

## Conflicts & precedence

- A dunder defines its capability: `__str__` defines `Display`, `__eq__` defines equality, `__lt__` defines `<`, and `__hash__` defines hashing.
- A dunder and the matching `@derive(...)` on one type are refused (`INCAN-T0001`).
- An automatic derive is never written in `@derive(...)`.

## Derive refusals

Each of these is refused at check time with `INCAN-T0001`:

| Declaration | Refused because |
| --- | --- |
| `@derive(Debg)` | The name is not a derive. |
| `@derive(User)` where `User` is a model, class, enum or function | The name is not a derive. |
| `@derive(module)` for a module that declares no `__derives__` | The module provides no derives. |
| `@derive(Eq)` together with `__eq__` (and the other dunder pairs) | See [Conflicts & precedence](#conflicts-precedence). |
| `@derive(Copy)` on a type with a field that is not `Copy` | See [Copy][derive-copy]. |

## Decorators (`@staticmethod`, `@classmethod`, `@requires`) {#decorators-staticmethod-requires}

`@derive(...)` is covered [above](#derive-catalog-quick-index). `@rust.extern` and `@rust.allow(...)` are covered in the Rust interop reference, and user-defined decorators in the [language reference](language.md#decorators).

### `@staticmethod`

- Applies to methods of `class`, `model`, `enum` and `newtype` declarations.
- The method has no `self` or `mut self` parameter; one is refused (`INCAN-T0001`).
- It is called on the type, `TypeName.method(...)`, and not through an instance.
- A static method and a field may share a name: `TimeDelta.days(7)` calls the method and `delta.days` reads the field.
- A generic static method may return `Self`; `Box[int].make(1)` fixes the owner's type arguments.
- It combines with `@rust.extern` for Rust-backed static methods.

```incan
class Temperature:
    celsius: float

    @staticmethod
    def from_fahrenheit(f: float) -> Temperature:
        return Temperature(celsius=(f - 32.0) / 1.8)

def main() -> None:
    t = Temperature.from_fahrenheit(98.6)
    println(t.celsius)
```

### `@classmethod`

- A class method is called on the type, `TypeName.method(...)`, and has no `self` parameter.
- Its first parameter (named `cls` by convention) is the declaring type and can be called as its constructor.
- `Self` in its signature is the type with the call site's type arguments.

```incan
class Box[T with Clone]:
    value: T

    @classmethod
    def make(cls, value: T) -> Self:
        return cls(value=value)

def main() -> None:
    boxed = Box[int].make(1)   # Self is Box[int]
    println(str(boxed.value))
```

### `@requires(...)`

See [`@requires(...)` (adopter contract)](#requires-adopter-contract).

## Generic instance methods

- A method of a `class`, `model`, `trait`, `enum` or `newtype` may declare type parameters after its name: `def name[T, U with Trait](...)`.
- A method's type parameters are scoped to that method. The enclosing type's parameters and the method's own may appear in the same signature.
- A trait method may be generic, whether it is required or has a default body.

```incan
model Shelf[U]:
    item: U

    def swap[T with Clone](self, value: T) -> T:   # U belongs to Shelf, T to swap
        return value

trait Echo:
    def echo[T with Clone](self, value: T) -> T:
        return value
```

### Bounds of instantiated types

A bound on a type parameter of a model, class, enum or newtype (`model Stream[R with Clone]`) applies to every instantiation of that type. A function, method, model, class, enum or newtype that instantiates it with one of its own type parameters (in a parameter, return, field, payload or underlying type, or in the type of an expression in a body) must declare the same bound on that type parameter, or a bound that implies it (`Copy` implies `Clone`). Otherwise the declaration is refused with `INCAN-T0001`.

```incan
model Stream[R with Clone]:
    item: R

def consume[T with Clone](stream: Stream[T]) -> int:   # accepted
    return 1

def consume_any[T](stream: Stream[T]) -> int:          # refused: T does not declare Clone (INCAN-T0001)
    return 1

def wrap[T](value: T) -> int:                           # refused: Stream(item=value) needs T with Clone (INCAN-T0001)
    stream = Stream(item=value)
    return 1
```

A bound that a called function's or method's body needs, such as `Display` for a value formatted in an f-string, is inferred. A generic caller does not declare it.

### Call-site type arguments

Type arguments may be written at the call site, in square brackets after the function or method name:

- Function: `callee[type_args](value_args...)`
- Method: `receiver.method[type_args](value_args...)`

Rules:

- `type_args` is comma-separated. Each entry is a type or `_`.
- With brackets, the number of entries equals the callee's number of type parameters; `_` entries count. `decode_rows[Order](...)` is refused for a two-parameter callee; `decode_rows[Order, _](...)` is accepted.
- An explicit entry fixes its type parameter. A `_` entry is inferred from the value arguments. A type parameter that stays unresolved is refused (`INCAN-T0001`).
- Without brackets, every type parameter is inferred.
- Brackets are accepted on direct calls of Incan functions and methods, and on a type-associated Rust call `Type.method[T](...)` whose receiver declares only type parameters. They are refused (`INCAN-T0001`) on builtin calls such as `len[int](...)`, on functions imported from Rust, on a callee reached through a variable (`read = session.read_csv; read[Order](...)`), and on a Rust receiver with const parameters.

```incan
rows_inferred = session.read_csv(str("orders.csv"))                    # T inferred
rows_typed = session.read_csv[Order](str("orders.csv"))                # T is Order
parsed = decode_rows(str("orders.csv"))                                # T and E inferred
parsed_typed = decode_rows[Order, CsvDecodeError](str("orders.csv"))   # T and E explicit
parsed_partial = decode_rows[Order, _](str("orders.csv"))              # T explicit, E inferred
```

## Traits (authoring)

- A trait declares methods. A required method is a signature with no body (`def render(self) -> str`, or with `: ...`); a default method has a body.
- A `model`, `class`, `enum`, `newtype` or `rusttype` adopts a trait with `with TraitName`. A trait adopts other traits the same way, and adoption is transitive: an adopter of `OrderedCollection[T]` also adopts `Collection[T]`.
- A trait is never constructed: `TraitName(...)` is refused (`INCAN-T0001`).
- A trait used as a type (`values: Collection[int]`) accepts any adopter of that trait instantiation.
- An enum adopter declares the trait's required methods in its body.
- `@requires(...)` names fields of model and class adopters.
- When two adopted traits require the same method name, each method names its trait with `for TraitName` (see [Method-level trait targets](#method-level-trait-targets)).
- A bound `T with Trait[...]` is satisfied by a type that adopts that trait instantiation, directly or through a supertrait.

```incan
trait Describable:
    def describe(self) -> str:
        return "An object"

class Product with Describable:
    name: str

trait Collection[T]:
    def first(self) -> T

trait OrderedCollection[T] with Collection[T]:
    def sorted(self) -> Self

def first_item(values: Collection[int]) -> int:
    return values.first()

def require_ordering[T with OrderedCollection[int]](values: T) -> T:
    return values
```

### Multiple instantiations of one generic trait

- A type may adopt one generic trait several times with different type arguments: `with Convert[int], Convert[float]`.
- Each method that shares a name satisfies a different instantiation.
- Two identical instantiations are refused (`INCAN-T0001`).
- Two methods with the same name are refused (`INCAN-T0001`) unless each satisfies a distinct instantiation of one generic trait, or names its trait with `for TraitName`.

```incan
trait Convert[T]:
    def convert(self) -> T: ...

model Reading with Convert[int], Convert[float]:   # accepted
    value: int

    def convert(self) -> int:
        return self.value

    def convert(self) -> float:
        return 1.0
```

#### Dispatch from argument types

A call selects the instantiation whose method parameter types match the argument types, named arguments included.

```incan
trait Reader[T]:
    def read(self, key: T) -> str: ...

model Source with Reader[str], Reader[int]:
    name: str

    def read(self, key: str) -> str:
        return key

    def read(self, key: int) -> str:
        return str(key)

source = Source(name="events")
by_name = source.read("latest")   # Reader[str]
by_index = source.read(0)         # Reader[int]
by_key = source.read(key=0)       # Reader[int]
```

#### Dispatch from an expected return type

When the arguments do not select one instantiation, the expected result type does: an annotated binding, a parameter the result is passed to, or an annotated return. With no expected type, the call is refused as ambiguous (`INCAN-T0001`).

```incan
reading = Reading(value=1)
as_float: float = reading.convert()   # Convert[float]
as_int: int = reading.convert()       # Convert[int]
value = reading.convert()             # refused: no expected result type (INCAN-T0001)
```

#### Generic bounds with trait type arguments

A bound may carry trait type arguments. `T with Serializable[F]` requires `T` to adopt `Serializable` instantiated with `F`.

```incan
trait Serializable[F]:
    def serialize(self, format: F) -> bytes: ...

model JsonFormat:
    name: str

model Event with Serializable[JsonFormat]:
    message: str

    def serialize(self, format: JsonFormat) -> bytes:
        return b"{}"

def encode[F, T with Serializable[F]](value: T, format: F) -> bytes:
    return value.serialize(format)

bytes = encode[JsonFormat, Event](Event(message="created"), JsonFormat(name="json"))   # accepted
```

#### Enum adopters

An enum adopts generic traits under the same rules as a model or class.

```incan
trait Label[T]:
    def label(self) -> T: ...

enum Token with Label[str], Label[int]:
    Identifier(str)
    Number(int)

    def label(self) -> str:
        return "token"

    def label(self) -> int:
        return 1

token: Token = Token.Number(1)
text: str = token.label()
code: int = token.label()
```

#### Method-level trait targets

`def name(params) for TraitName -> Return:` declares which adopted trait the method satisfies. The target is not part of the return type.

```incan
trait ToInt:
    def convert(self) -> int: ...

trait ToStr:
    def convert(self) -> str: ...

type Value = newtype int with ToInt, ToStr:
    def convert(self) for ToInt -> int:
        return self.0

    def convert(self) for ToStr -> str:
        return str(self.0)
```

#### Rejected cases

```incan
model BadReading with Convert[int], Convert[int]:   # refused: identical instantiations (INCAN-T0001)
    value: int

model Parser:
    def parse(self, value: str) -> str: ...
    def parse(self, value: int) -> str: ...          # refused: same name without distinct instantiations (INCAN-T0001)

trait ReadsInt:
    def read(self, value: int) -> int: ...

trait ReadsStr:
    def read(self, value: str) -> str: ...

model Source with ReadsInt, ReadsStr:
    def read(self, value: int) -> int: ...
    def read(self, value: str) -> str: ...           # refused: unrelated traits share the name without `for` targets (INCAN-T0001)
```

### `@requires(...)` (adopter contract)

`@requires(field_a: TypeA, field_b: TypeB)` on a trait names fields that every model or class adopter declares, with compatible types.

- An adopter that lacks a required field, or declares it with an incompatible type, is refused (`INCAN-T0001`).
- A default method of the trait reads `self.field` only for a field named in `@requires(...)`; changing one needs `mut self`.
- An adopter of a subtrait meets the `@requires(...)` of every supertrait.

```incan
@requires(name: str)
trait Loggable:
    def log(self, msg: str) -> None:
        println(f"[{self.name}] {msg}")

class Service with Loggable:   # accepted: declares name: str
    name: str

@requires(count: int)
trait Counter:
    def bump(mut self) -> None:
        self.count += 1
```

## Debug (Automatic)

- Automatic on every model, class, enum and newtype.
- `{value:?}` renders it; no dunder overrides it.

```incan
model Point:
    x: int
    y: int

def main() -> None:
    p = Point(x=10, y=20)
    println(f"{p:?}")  # Point { x: 10, y: 20 }
```

## Display (Custom with `__str__`)

- `{value}`, `str(value)` and `print`/`println` render `Display`.
- A type has `Display` when it defines `__str__(self) -> str`.

```incan
model User:
    name: str
    email: str

    def __str__(self) -> str:
        return f"{self.name} <{self.email}>"

def main() -> None:
    u = User(name="Alice", email="alice@example.com")
    println(f"{u}")    # Alice <alice@example.com>
    println(f"{u:?}")  # User { name: "Alice", email: "alice@example.com" }
```

## Eq (Equality)

- `@derive(Eq)` provides `==` and `!=` by comparing every field.
- `__eq__(self, other: Self) -> bool` defines equality instead.

```incan
model User:
    id: int
    name: str

    def __eq__(self, other: User) -> bool:
        return self.id == other.id
```

## Ord (ordering)

- `@derive(Ord)` provides `<`, `<=`, `>` and `>=`, comparing fields in declaration order.
- `__lt__(self, other: Self) -> bool` defines `<`. `__le__`, `__gt__` and `__ge__` define `<=`, `>` and `>=`; an operator whose dunder is missing is refused (`INCAN-T0001`).

```incan
model Task:
    priority: int
    name: str

    def __lt__(self, other: Task) -> bool:
        return self.priority < other.priority
```

## Hash

- `@derive(Hash)` makes a type hashable. A `set` element type or `dict` key type needs both `Eq` and `Hash`.
- `__hash__(self) -> int` defines hashing instead. When `a == b`, `a.__hash__() == b.__hash__()` must hold.

```incan
@derive(Eq, Hash)
model UserId:
    id: int
```

## Clone

- `.clone()` returns a deep copy.
- Automatic on every model, class and enum, and on a newtype whose underlying type is `Copy`.

## Copy

- A `Copy` value is copied, not moved, when it is assigned or passed.
- `@derive(Copy)` requires every field type to be `Copy`.

## Default

- `@derive(Default)` provides `Type.default()`. Each field takes its declared default, or its type's default when it declares none.
- A normal construction takes each omitted field's declared default; a field without a default is required.
- `T with Default` is a generic bound; `T.default()` constructs the value.

```incan
@derive(Default)
model Settings:
    theme: str = "dark"
    font_size: int = 14

def make[T with Default]() -> T:
    return T.default()

def main() -> None:
    a = Settings()                 # accepted: every omitted field has a default
    b = Settings(font_size=16)     # accepted
    c = Settings.default()         # accepted
    d: Settings = make()           # accepted
```

## Serialize

- `@derive(Serialize)` (from `std.serde.json`) provides `json_stringify(value) -> str`.
- A type that adopts `std.serde.json.Serialize` (`with Serialize`) provides `value.to_json() -> str`.
- `@derive(json)` (from `std.serde`) adopts both `Serialize` and `Deserialize`. After `from std.serde import json`, the traits are also named `json.Serialize` and `json.Deserialize`.

```incan
from std.serde.json import Serialize

@derive(Serialize)
model User:
    name: str
    age: int

model Account with Serialize:
    id: int

def main() -> None:
    println(json_stringify(User(name="Alice", age=30)))
    println(Account(id=1).to_json())
```

```incan
from std.serde import json

@derive(json)
model User:
    name: str
    age: int

def encode[T with json.Serialize](value: T) -> str:
    return value.to_json()
```

## Deserialize

- `@derive(Deserialize)` (from `std.serde.json`) provides `T.from_json(input: str) -> Result[T, str]`.
- A type that adopts `Deserialize` with `with Deserialize` has `@derive(Deserialize)` or defines `from_json(input: str)`.

```incan
from std.serde.json import Deserialize

@derive(Deserialize)
model User:
    name: str
    age: int

def main() -> None:
    result: Result[User, str] = User.from_json("{\"name\":\"Alice\",\"age\":30}")
```

## Validate (Models only)

- `@derive(Validate)` applies to models. The model defines `validate(self) -> Result[Self, E]`.
- `TypeName.new(...)` constructs the model and returns `validate`'s result.
- `TypeName(...)` on a `Validate` model is refused (`INCAN-T0001`).

```incan
@derive(Validate)
model EmailUser:
    email: str

    def validate(self) -> Result[EmailUser, str]:
        if "@" not in self.email:
            return Err("invalid email")
        return Ok(self)

def make_user(email: str) -> Result[EmailUser, str]:
    return EmailUser.new(email=email)
```

See [Derives: Validation](derives/validation.md).

## Reflection (automatic)

Every model and class provides:

- `__fields__() -> FrozenList[FieldInfo]`
- `__class_name__() -> str`

Field metadata (`[alias="..."]`, `[description="..."]`) applies to models only. For a class, `FieldInfo.alias` and `FieldInfo.description` are `None` and `FieldInfo.wire_name` equals `FieldInfo.name`. `FieldInfo` needs an import only where the type is written. See [Reflection (Reference)](reflection.md) for `FieldInfo`.

```incan
model User:
    name: str

def main() -> None:
    u = User(name="Alice")
    println(u.__class_name__())                  # User
    println([f.name for f in u.__fields__()])    # ["name"]
```

## Standard-library derive traits

`Clone`, `Default`, `Debug`, `Eq`, `Ord` and `Hash` are traits declared in the standard library under `std.derives.*`. See [Standard library reference: `std.derives.*`](stdlib/derives.md).

## See also

- [The Incan Book: Traits and derives](../tutorials/book/11_traits_and_derives.md)
- [Derives and traits (explanation)](../explanation/derives_and_traits.md)
- [How derives work](../explanation/how_derives_work.md)
- [Why call-site type arguments exist](../explanation/call_site_type_arguments.md)
- [Error Handling Guide][guide-error-handling]
