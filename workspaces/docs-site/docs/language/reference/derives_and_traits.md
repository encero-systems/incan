# Derives and traits (reference)

This page specifies derives (the automatic derives, the derive catalog and `@derive(...)`), the method decorators `@staticmethod` and `@classmethod`, generic methods, inferred bounds and call-site type arguments, and trait authoring, including the `mut` parameters of trait methods. Each derive's own contract is on its derive page, listed in the [derive catalog](#derive-catalog).

## Automatic derives

These derives belong to every declaration of the kind, without `@derive(...)`:

| Construct | Automatic derives |
| --- | --- |
| `model` | `Debug`, `Clone` |
| `class` | `Debug`, `Clone` |
| `enum` | `Debug`, `Clone`; `PartialEq` when every payload type is a number, `bool`, `str` or `bytes`, or a `list`, `set`, `dict`, `Option`, `Result` or tuple of those |
| `newtype` | `Debug` when the underlying type implements it; `Clone` as listed below; `Copy` when the underlying type is `Copy` |

- An automatic derive satisfies a generic bound on the same trait.
- Writing an automatic derive in `@derive(...)` is accepted and adds nothing.
- A newtype carries `Clone` when its underlying type is a `Copy` type; a builtin scalar; a builtin collection, `Option`, `Result`, tuple, `model`, `class` or `enum` whose type arguments implement `Clone`; a newtype that carries `Clone`; a standard-library type that implements `Clone`, such as `Mutex[T]` over such a `T`; or one of the newtype's own type parameters, in which case the newtype is `Clone` for type arguments that are. Over any other underlying type, including a `rust::` type and a `rusttype`, it carries `Clone` only through `@derive(Clone)` or `@rust.derive(Clone)`.
- A `model` or `class` field, or an `enum` payload, whose type does not implement both `Clone` and `Debug` is refused at the declaration (`INCAN-T0113`). `JoinHandle[T]` and `RaceArm[R]` implement neither; `Receiver[T]`, `OneshotSender[T]` and `OneshotReceiver[T]` lack `Clone`; a newtype lacks what its underlying type lacks unless it derives it; a `list`, `dict`, `set`, `Option`, `Result`, tuple or generic declaration lacks what a type argument lacks.

```incan
import std.async
from std.async.task import JoinHandle

type Handle = newtype JoinHandle[int]   # accepted: a newtype carries only the derives its underlying type has

model Pending:
    handle: JoinHandle[int]   # refused: JoinHandle[int] implements neither Clone nor Debug (INCAN-T0113)

model Wrapped:
    handle: Handle            # refused: Handle carries neither Clone nor Debug (INCAN-T0113)
```

## Derive catalog

| Derive | Provides | Dunder | Reference |
| --- | --- | --- | --- |
| `Debug` | `{value:?}` | — | [String representation](derives/string_representation.md#debug) |
| `Display` | `{value}`, `str(value)`, `print(value)` | `__str__` | [String representation](derives/string_representation.md#display) |
| `Eq` | `==`, `!=`; with `Hash`, use as a `set` element or `dict` key | `__eq__`, `__ne__` | [Comparison](derives/comparison.md#eq) |
| `PartialEq` | `==`, `!=` | `__eq__`, `__ne__` | [Comparison](derives/comparison.md#partialeq) |
| `Ord` | `<`, `<=`, `>`, `>=` | `__lt__`, `__le__`, `__gt__`, `__ge__` | [Comparison](derives/comparison.md#ord) |
| `PartialOrd` | `<`, `<=`, `>`, `>=` | `__lt__`, `__le__`, `__gt__`, `__ge__` | [Comparison](derives/comparison.md#partialord) |
| `Hash` | with `Eq`, use as a `set` element or `dict` key | — | [Comparison](derives/comparison.md#hash) |
| `Clone` | `.clone()` | — | [Copying and Default](derives/copying_default.md#clone) |
| `Copy` | copying on assignment and argument passing | — | [Copying and Default](derives/copying_default.md#copy) |
| `Default` | `Type.default()` | — | [Copying and Default](derives/copying_default.md#default) |
| `json`, `Serialize`, `Deserialize` | JSON serialization | — | [Serialization](derives/serialization.md) |
| `Validate` | `Type.new(...)`, validated construction | — | [Validation](derives/validation.md) |
| `Descriptor` | registry descriptors | — | [Registry](stdlib/registry.md#descriptor-contract) |

`Debug`, `Display`, `Eq`, `Ord`, `Hash`, `Clone`, `Copy` and `Default` are also traits under `std.derives.*` (see [`std.derives`](stdlib/derives.md)). A model or class also provides `__class_name__()` and `__fields__()` without a derive (see [Reflection](reflection.md)).

## `@derive(...)`

- `@derive(...)` names derives from the catalog. `json` needs `from std.serde import json`, and `Serialize` and `Deserialize` need their import from `std.serde.json`.
- `@derive(...)` also names a trait imported by name (`from codec import Encode`, then `@derive(Encode)`), or a module that declares `__derives__` (`import codec`, then `@derive(codec)`, which names each trait in the module's `__derives__` list). A model or class adopts each such trait as a `with` clause adopts it: the trait's methods, default methods included, are called on the type (`item.tag()`), and the type satisfies a bound on the trait.
- `Eq` implies `PartialEq`, and `Ord` implies `PartialOrd`, `Eq` and `PartialEq`.
- `@rust.derive(...)` names Rust derives (see [Decorators](language.md#decorators)).
- The dunders that define what a derive provides, and the pairs that conflict, are in [Custom behavior](derives/custom_behavior.md).

Each of these is refused (`INCAN-T0001`):

| Declaration | Rule |
| --- | --- |
| `@derive(Debg)` | The name is not a derive. |
| `@derive(User)` where `User` is a model, class, enum or function | The name is not a derive. |
| `@derive(module)` for a module that declares no `__derives__` | The module provides no derives. |
| `@derive(Eq)` on a type that defines `__eq__` | See [Custom behavior](derives/custom_behavior.md). |
| `@derive(Copy)` on a type with a field that is not `Copy` | See [Copy](derives/copying_default.md#copy). |

## Decorators (`@staticmethod`, `@classmethod`, `@requires`) {#decorators-staticmethod-requires}

`@derive(...)` is specified [above](#derive). `@rust.extern`, `@rust.allow(...)` and user-defined decorators are in [Decorators](language.md#decorators).

### `@staticmethod`

- Applies to methods of `class`, `model`, `enum` and `newtype` declarations.
- The method has no `self` or `mut self` parameter; one is refused (`INCAN-T0001`).
- It is called on the type, `TypeName.method(...)`. A call through an instance is refused (`INCAN-T0001`).
- A static method and a field may share a name: `TimeDelta.days(7)` calls the method and `delta.days` reads the field.
- A generic static method may return `Self`; `Box[int].make(1)` fixes the owner's type arguments.
- It combines with `@rust.extern` for Rust-backed static methods.

```incan
class Temperature:
    pub celsius: float

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
    pub value: T

    @classmethod
    def make(cls, value: T) -> Self:
        return cls(value=value)

def main() -> None:
    boxed = Box[int].make(1)   # accepted: Self is Box[int]
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

    def swap[T with Clone](self, value: T) -> T:   # accepted: T is scoped to swap
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

A bound that a function's or method's body needs is inferred: `Display` for a value formatted in an f-string, and `Clone` for an element of a `list[T]` read by index, a slice of a `list[T]`, or a value of a `dict[K, V]` read by key. Neither the declaration nor a generic caller declares it.

```incan
def first[K](items: list[K]) -> K:                   # accepted
    return items[0]

def rest[K](items: list[K]) -> list[K]:              # accepted
    return items[1:]

def lookup[V](table: dict[str, V], key: str) -> V:   # accepted
    return table[key]

def head[T](items: list[T]) -> T:                    # accepted
    return first(items)
```

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
def pair[A, B](first: A, second: B) -> tuple[A, B]:
    return (first, second)

def main() -> None:
    a = pair(1, "x")              # accepted: A and B inferred
    b = pair[int, str](1, "x")    # accepted
    c = pair[int, _](1, "x")      # accepted: B inferred
    d = pair[int](1, "x")         # refused: pair has two type parameters (INCAN-T0001)
    e = len[int]([1])             # refused: brackets on a builtin call (INCAN-T0001)
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
- A default method's body resolves the names it uses in the trait's module: a type, an enum variant or a function that the trait's module declares or imports, private functions included, whether or not the adopting module imports it.
- A trait imported through a module that re-exports it (`pub from shapes import Measured` in `geometry`, then `from geometry import Measured`) is the trait its declaring module declares: `with Measured` adopts `shapes.Measured`, its default methods included.

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

def main() -> None:
    source = Source(name="events")
    by_name = source.read("latest")   # accepted: selects Reader[str]
    by_index = source.read(0)         # accepted: selects Reader[int]
    by_key = source.read(key=0)       # accepted: selects Reader[int]
```

#### Dispatch from an expected return type

When the arguments do not select one instantiation, the expected result type does: an annotated binding, a parameter the result is passed to, or an annotated return. With no expected type, the call is refused as ambiguous (`INCAN-T0001`).

```incan
def main() -> None:
    reading = Reading(value=1)
    as_float: float = reading.convert()   # accepted: selects Convert[float]
    as_int: int = reading.convert()       # accepted: selects Convert[int]
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

def main() -> None:
    encoded = encode[JsonFormat, Event](Event(message="created"), JsonFormat(name="json"))   # accepted
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

def main() -> None:
    token: Token = Token.Number(1)
    text: str = token.label()   # accepted
    code: int = token.label()   # accepted
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
    def parse(self, value: str) -> str:
        return value

    def parse(self, value: int) -> str:              # refused: same name without distinct instantiations (INCAN-T0001)
        return str(value)

trait ReadsInt:
    def read(self, value: int) -> int: ...

trait ReadsStr:
    def read(self, value: str) -> str: ...

model Source with ReadsInt, ReadsStr:
    def read(self, value: int) -> int:
        return value

    def read(self, value: str) -> str:               # refused: unrelated traits share the name without `for` targets (INCAN-T0001)
        return value
```

### `@requires(...)` (adopter contract)

`@requires(field_a: TypeA, field_b: TypeB)` on a trait names fields that every model or class adopter declares, with compatible types.

- An adopter that lacks a required field, or declares it with an incompatible type, is refused (`INCAN-T0001`).
- A default method of the trait reads `self.field` only for a field named in `@requires(...)`; another field is refused (`INCAN-T0001`). Changing a field needs `mut self`; a change through `self` is refused (`INCAN-T0102`).
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

### `mut` parameters of trait methods

A `mut` parameter of a trait method, required or default, and of each method that implements it, follows the rules of [`mut` parameters](functions.md#mut-parameters):

- A parameter of type `int`, `float` or `bool`, or an alias of one, is the method's own copy.
- A parameter of any other type, except a Rust type, `*args` and `**kwargs`, shows the method's changes to the caller. The body does not rebind it or hold it in another name or value (`INCAN-T0001`). The argument of a call that changes it is a `mut` binding or parameter, `self` in a `mut self` method, a field of one of those, or a temporary; another argument is refused (`INCAN-T0117`).
- A method call through a type parameter's bound, on `self` in a default method, or on a trait-typed value changes such a parameter, whichever method runs.

```incan
trait Grower:
    def grow(self, mut items: list[int]) -> int:
        items.append(1)
        return len(items)

    def size(self, mut items: list[int]) -> int:
        return len(items)

model Plant with Grower:
    id: int

def run[T with Grower](grower: T, items: list[int]) -> int:
    return grower.size(items)            # refused: a call through a bound changes the parameter (INCAN-T0117)

def main() -> None:
    plant = Plant(id=1)
    mut kept: list[int] = [1, 2]
    fixed: list[int] = [1, 2]
    println(plant.grow(kept))            # accepted
    println(plant.grow([5]))             # accepted
    println(plant.grow(fixed))           # refused: fixed is not declared mut (INCAN-T0117)
    println(plant.size(fixed))           # accepted: size does not change the parameter
```

## See also

- [The Incan Book: Traits and derives](../tutorials/book/11_traits_and_derives.md)
- [Derives and traits (explanation)](../explanation/derives_and_traits.md)
- [How derives work](../explanation/how_derives_work.md)
- [Why call-site type arguments exist](../explanation/call_site_type_arguments.md)
- [Error handling (explanation)](../explanation/error_handling.md)
