# std.json reference

`std.json` provides `JsonValue`, Incan's dynamic JSON value type for payloads whose full shape is not known at compile time.

Use typed models with `std.serde.json` when the schema is stable. Use `JsonValue` when part or all of the payload is exploratory, mixed-shape, or intentionally open.

Import with:

```incan
from std.json import JsonValue
```

## Parse and Serialize

```incan
from std.json import JsonValue

def main() -> None:
    match JsonValue.parse('{"status":200,"data":{"name":"Ada"}}'):
        case Ok(value):
            match value.to_json():
                case Ok(text):
                    println(text)
                case Err(err):
                    println(err.message())
        case Err(err):
            println(err.message())
```

| API | Returns |
| --- | --- |
| `JsonValue.parse(source: str)` | `Result[JsonValue, JsonError]` |
| `JsonValue.loads(source: str)` | `Result[JsonValue, JsonError]` |
| `value.to_json()` | `Result[str, JsonError]` |
| `value.to_pretty_json()` | `Result[str, JsonError]` |
| `value.dumps()` | `Result[str, JsonError]` |
| `dumps(value: JsonValue)` | `Result[str, JsonError]` |
| `dumps_pretty(value: JsonValue)` | `Result[str, JsonError]` |

## Constructors

| API | JSON kind |
| --- | --- |
| `JsonValue.null()` | null |
| `JsonValue.bool(value: bool)` | boolean |
| `JsonValue.int(value: int)` | number mapped to Incan `int` |
| `JsonValue.float(value: float)` | `Result[JsonValue, JsonError]` for finite JSON numbers mapped to Incan `float` |
| `JsonValue.str(value: str)` | string |
| `JsonValue.string(value: str)` | string |
| `JsonValue.array(values: list[JsonValue])` | array |
| `JsonValue.object(entries: Dict[str, JsonValue])` | object |

`JsonValue.float(...)` returns an error for NaN and infinities because JSON has no spelling for those values.

## Shape Inspection

`value.kind()` returns `JsonKind`. Use `value.kind().as_str()` when string matching is enough.

| Predicate | Meaning |
| --- | --- |
| `value.is_null()` | JSON null |
| `value.is_bool()` | JSON boolean |
| `value.is_int()` | JSON number mapped to Incan `int` |
| `value.is_float()` | JSON number mapped to Incan `float` |
| `value.is_number()` | any JSON number |
| `value.is_str()` | JSON string |
| `value.is_array()` | JSON array |
| `value.is_object()` | JSON object |

## Extraction

Extraction helpers return `Option[...]` when the requested shape may be absent and `Result[..., JsonError]` when the caller wants a required shape.

| Optional API | Required API |
| --- | --- |
| `value.as_bool()` | `value.expect_bool()` |
| `value.as_int()` | `value.expect_int()` |
| `value.as_float()` | `value.expect_float()` |
| `value.as_str()` | `value.expect_str()` |
| `value.as_array()` | `value.expect_array()` |
| `value.as_object()` | `value.expect_object()` |

## Checked Indexing

Direct indexing is checked and optional:

```incan
from std.json import JsonError, JsonValue

def first_item_name(source: str) -> Result[str, JsonError]:
    data = JsonValue.parse(source)?
    name = data.require_pointer("/items/0/name")?
    return name.expect_str()

def main() -> None:
    match first_item_name('{"items":[{"name":"Ada"}]}'):
        case Ok(text):
            println(text)
        case Err(err):
            println(err.message())
```

`value["key"]` returns `Option[JsonValue]` for object lookup. It returns `Some(value)` when the key exists, including when that value is JSON null. It returns `None` when the receiver is not an object or the key is missing.

`value[index]` returns `Option[JsonValue]` for array lookup. It returns `Some(value)` for an in-bounds non-negative index and `None` for non-arrays, negative indices, and out-of-range indices.

Use `get(key)` and `at(index)` for named optional helpers. Use `require(key)`, `require_key(key)`, and `require_index(index)` for JSON-specific errors.
For nested required paths, prefer `require_pointer(path)?` over stacking optional lookups by hand.

## Object Helpers

| API | Returns |
| --- | --- |
| `value.get(key: str)` | `Option[JsonValue]` |
| `value.require(key: str)` | `Result[JsonValue, JsonError]` |
| `value.require_key(key: str)` | `Result[JsonValue, JsonError]` |
| `value.contains_key(key: str)` | `bool` |
| `value.keys()` | `list[str]` |
| `value.values()` | `list[JsonValue]` |
| `value.items()` | `list[tuple[str, JsonValue]]` |
| `value.set(key: str, value: JsonValue)` | `Result[None, JsonError]` |
| `value.put(key: str, value: JsonValue)` | `Result[None, JsonError]` |
| `value.remove(key: str)` | `Result[Option[JsonValue], JsonError]` |
| `value.merge(other: JsonValue)` | `Result[None, JsonError]` |

## Array and Traversal Helpers

| API | Returns |
| --- | --- |
| `value.at(index: int)` | `Option[JsonValue]` |
| `value.require_index(index: int)` | `Result[JsonValue, JsonError]` |
| `value.len()` | `int` |
| `value.is_empty()` | `bool` |
| `value.push(value: JsonValue)` | `Result[None, JsonError]` |
| `value.append(value: JsonValue)` | `Result[None, JsonError]` |
| `value.extend(values: list[JsonValue])` | `Result[None, JsonError]` |
| `value.insert(index: int, value: JsonValue)` | `Result[None, JsonError]` |
| `value.remove_at(index: int)` | `Result[Option[JsonValue], JsonError]` |
| `value.pointer(path: str)` | `Result[Option[JsonValue], JsonError]` |
| `value.require_pointer(path: str)` | `Result[JsonValue, JsonError]` |
| `value.children()` | `list[JsonValue]` |
| `value.descendants()` | `list[JsonValue]` |

`pointer(...)` uses JSON Pointer syntax such as `""`, `/items/0`, and `/items/0/name`. It is not JSONPath.

## Numeric Classification

JSON numeric values map to Incan `int` or `float` under the same JSON-compatible lexical contract exposed by `std.math.is_int_like` and `std.math.is_float_like`. Integer-like JSON numbers become `int`; JSON numbers with a fractional or exponent part become `float`. Unsupported or out-of-range numeric payloads produce `JsonError`.

## Model Interop

`JsonValue` can be used as a dynamic field inside `@derive(json)` models:

```incan
from std.serde import json
from std.json import JsonValue

@derive(json)
model Envelope:
    status: int
    data: JsonValue
```

The dynamic field serializes and deserializes as ordinary JSON rather than as a wrapper object.

## Errors

`JsonError` exposes `kind()`, `kind_name()`, `detail()`, and `message()`. Error kinds include parse, type, key, index, and number errors.
