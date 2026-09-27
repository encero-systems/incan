# Derives: Serialization (reference)

This page specifies JSON serialization: the `json` derive, `Serialize` and `Deserialize`, field names, and how each type maps to JSON. The derive catalog is in [Derives and traits](../derives_and_traits.md).

## Spellings

| Spelling | Import | Provides |
| --- | --- | --- |
| `@derive(json)` | `from std.serde import json` | `Serialize` and `Deserialize` |
| `@derive(Serialize)`, or `with Serialize` | `from std.serde.json import Serialize` | `Serialize` |
| `@derive(Deserialize)`, or `with Deserialize` | `from std.serde.json import Deserialize` | `Deserialize` |

After `from std.serde import json`, the traits are also named `json.Serialize` and `json.Deserialize`. Each spelling applies to a `model`, `class`, `enum` or `newtype`.

## Serialize

- **Provides**: `value.to_json() -> str`, `json_stringify(value) -> str`, and `T with Serialize` bounds.
- **Provided by**: `@derive(json)`, `@derive(Serialize)`, or adopting `Serialize`.
- **Behavior**: the value is written as JSON by the [type mapping](#type-mapping).
- **Requires**: every field type serializes.

## Deserialize

- **Provides**: `T.from_json(input: str) -> Result[T, str]`, and `T with Deserialize` bounds.
- **Provided by**: `@derive(json)` or `@derive(Deserialize)`; adopting `Deserialize` and defining `from_json`.
- **Behavior**: the input is read by the [type mapping](#type-mapping). Input that does not match returns `Err`.
- **Requires**: every field type deserializes. A type that adopts `Deserialize` without deriving it defines `from_json`, or it is refused (`INCAN-T0001`).

```incan
from std.serde import json

@derive(json)
model User:
    name: str
    age: int

def encode[T with json.Serialize](value: T) -> str:
    return value.to_json()

def main() -> None:
    text = encode(User(name="Alice", age=30))               # accepted
    parsed: Result[User, str] = User.from_json(text)        # accepted
```

## Field names

- A model field's JSON key is its alias when it declares one (`type_ as "type": str`, or `type_ [alias="type"]: str`), and its name otherwise.
- A class field's JSON key is its name.

## Type mapping

| Type | JSON |
| --- | --- |
| `str` | string |
| `int`, the exact-width integers, `float`, `f32`, `f64` | number |
| `bool` | `true` or `false` |
| `list[T]`, `set[T]`, a tuple | array |
| `dict[str, T]` | object |
| `Option[T]` | the value, or `null` for `None` |
| `std.json.JsonValue` | the JSON value it holds |
| `model`, `class` | object with one key per field |
| `enum` that declares values | the variant's value: `enum Environment(str)` with `Production = "production"` writes `"production"`, and `enum HttpStatus(int)` with `NotFound = 404` writes `404` |
| Any other `enum` | a variant without a payload as its name (`"Pending"`); a variant with one payload as an object from its name to the payload (`{"Success": "ok"}`); a variant with several payloads as an object from its name to an array of them (`{"Error": [404, "missing"]}`) |
| `newtype` | its underlying value |

- A newtype with a `from_underlying` hook or constraints deserializes through them: a value they refuse returns `Err`.
- An `enum` or `newtype` that a serializing `model` or `class` names as a field type serializes the same way, without its own derive.
