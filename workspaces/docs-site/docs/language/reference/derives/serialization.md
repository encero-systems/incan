# Derives: Serialization (Reference)

This page documents `Serialize` and `Deserialize` for JSON.

See also:

- [Derives & traits](../derives_and_traits.md)
- [Error handling](../../explanation/error_handling.md)

---

## Module derive

Import `std.serde`'s `json` module when a type should adopt both JSON traits through one derive:

```incan
from std.serde import json

@derive(json)
model User:
    name: str
    age: int

def encode[T with json.Serialize](value: T) -> str:
    return value.to_json()
```

`@derive(json)` adopts `json.Serialize` and `json.Deserialize`, forwards the required Rust serde derives, and makes the adopted traits available to method lookup and generic bounds. Import from `std.serde.json` directly when you want only one side of the protocol.

---

## Generic bounds

A type parameter bounded by `std.serde.json`'s `Serialize` or `Deserialize` accepts exactly the types that adopt that trait, through `@derive(json)`, `@derive(Serialize)` / `@derive(Deserialize)`, or `with Serialize` / `with Deserialize`. Any other type argument is refused at check time. The bound means the same under every spelling of the trait: the imported name (`Serialize`), an alias (`Serialize as JsonSerialize`), and the module-qualified name (`json.Serialize`).

| Bound | Provides, for `value: T` |
| --- | --- |
| `T with Serialize` | `value.to_json() -> str` and `json_stringify(value) -> str` |
| `T with Deserialize` | `T.from_json(input: str) -> Result[T, str]` |

The bound holds wherever the type parameter is declared: on a function, a method, a model or a class. A model or class whose parameter carries the bound may derive or adopt the same trait, and `json_stringify` accepts a value that contains the parameter, such as a `list[T]` field.

```incan
from std.serde.json import Deserialize, Serialize

@derive(Serialize, Deserialize)
model Payload:
    value: int

@derive(Serialize)
model Envelope[T with Serialize]:
    payload: T

def encode[T with Serialize](value: T) -> str:
    return value.to_json()          # encode(Payload(value=1)) is {"value":1}

def stringify[T with Serialize](value: T) -> str:
    return json_stringify(value)    # stringify(Payload(value=1)) is {"value":1}

def decode[T with Deserialize](text: str) -> Result[T, str]:
    return T.from_json(text)        # decode[Payload]("{\"value\":2}") is Ok(Payload(value=2))

# Envelope(payload=Payload(value=3)).to_json() is {"payload":{"value":3}}
# encode(1) is refused: int does not adopt Serialize
```

---

## Serialize

- **Derive**: `@derive(json)` for both JSON directions, or `@derive(Serialize)` after importing `Serialize` directly
- **API**: `json_stringify(value) -> str`
- **Trait import**: `from std.serde.json import Serialize` + `with Serialize` gives a default `.to_json()` implementation

```incan
from std.serde.json import Serialize

@derive(Serialize)
model User:
    name: str
    age: int

def main() -> None:
    u = User(name="Alice", age=30)
    println(json_stringify(u))
```

```incan
from std.serde.json import Serialize

model User with Serialize:
    name: str
    age: int

def main() -> None:
    println(User(name="Alice", age=30).to_json())
```

---

## Deserialize

- **Derive**: `@derive(json)` for both JSON directions, or `@derive(Deserialize)` after importing `Deserialize` directly
- **API**: `T.from_json(input: str) -> Result[T, str]`
- **Trait import**: `with Deserialize` still requires either an imported `@derive(Deserialize)` or an explicit `from_json()` implementation

```incan
from std.serde.json import Deserialize

@derive(Deserialize)
model User:
    name: str
    age: int

def main() -> None:
    result: Result[User, str] = User.from_json("{\"name\":\"Alice\",\"age\":30}")
```

---

## Schema-safe field names (models only)

If your JSON schema uses keys that are not valid Incan identifiers (or are keywords like `type`), represent them using a `model` field alias and choose a schema-safe canonical field name (e.g. `type_`).

```incan
from std.serde import json

@derive(json)
model Account:
    type_ as "type": str
```

When `json` is derived, the alias is used as the JSON key (`"type"`). The canonical identifier (`type_`) remains the stable field name in code. See [Models: Using aliases in code](../../explanation/models_and_classes/models.md#using-aliases-in-code).

`class` does not support field metadata/aliases, so class JSON keys always match the canonical field names.

## Enums

Ordinary enums support `@derive(json)` just like models:

```incan
from std.serde import json

@derive(json)
enum Status:
    Pending
    Active
    Completed

@derive(json)
enum ApiResponse:
    Success(str)
    Error(int, str)
```

Value enums serialize and deserialize through their declared raw value rather than the variant name:

```incan
from std.serde import json

@derive(json)
enum Environment(str):
    Development = "development"
    Production = "production"

@derive(json)
enum HttpStatus(int):
    Ok = 200
    NotFound = 404
```

`Environment.Production` serializes as `"production"` and deserializes only from known raw values. `HttpStatus.NotFound` serializes as `404`.

When a model references an enum in its fields, the compiler automatically propagates JSON serde derives to the enum:

```incan
from std.serde import json

@derive(json)
enum Priority:
    Low
    Medium
    High

@derive(json)
model Task:
    name: str
    priority: Priority  # Priority automatically gets serde derives
```

---

## Newtypes

Newtypes also support `@derive(json)`:

```incan
from std.serde import json

@derive(json)
newtype UserId(int)

@derive(json)
newtype Email(str)
```

Newtypes serialize to/from their underlying type's JSON representation.

When a newtype defines the canonical `from_underlying` validation hook or primitive constraints, JSON deserialization reconstructs through that checked ingress. Invalid JSON values return `Err` instead of creating an invalid nominal value.

---

## Dynamic JSON fields

Use `std.json.JsonValue` when one model field should remain dynamic while the rest of the model is typed:

```incan
from std.serde import json
from std.json import JsonValue

@derive(json)
model ApiResponse:
    status: int
    data: JsonValue
```

`JsonValue` fields serialize and deserialize as their contained JSON value. They do not add a wrapper object around the field.

---

## Type mappings (Incan → JSON)

| Incan             | JSON             |
| ----------------- | ---------------- |
| `str`             | string           |
| `int`             | number           |
| `float`           | number           |
| `bool`            | boolean          |
| `List[T]`         | array            |
| `Dict[str, T]`    | object           |
| `Option[T]`       | value or `null`  |
| `JsonValue`       | dynamic JSON value |
| `model` / `class` | object           |
| ordinary `enum`   | variant encoding |
| value `enum`      | backing `str` / `int` |
| `newtype`         | underlying type  |
