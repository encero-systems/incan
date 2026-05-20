# std.json reference

`std.json` provides `JsonValue`, Incan's dynamic JSON value type for payloads whose full shape is not known at compile time.

Use typed models with `std.serde.json` when the schema is stable. Use `JsonValue` when part or all of the payload is exploratory, mixed-shape, or intentionally open.

Common imports: `from std.json import JsonValue, JsonError`. For task-based examples, see [Work with dynamic JSON](../../how-to/dynamic_json.md).

## Types

| Type | Contract |
| --- | --- |
| `JsonValue` | Dynamic JSON value. |
| `JsonKind` | Runtime kind enum: null, bool, int, float, string, array, object. |
| `JsonError` | Error value returned by fallible JSON APIs. |
| `JsonErrorKind` | Error category enum: parse, type, key, index, number. |

## Top-Level Functions

| API | Returns |
| --- | --- |
| `parse(source: str)` | `Result[JsonValue, JsonError]` |
| `loads(source: str)` | `Result[JsonValue, JsonError]` |
| `dumps(value: JsonValue)` | `Result[str, JsonError]` |
| `dumps_pretty(value: JsonValue)` | `Result[str, JsonError]` |

`loads` is an alias for `parse`. `dumps` and `dumps_pretty` delegate to the corresponding `JsonValue` serialization methods.

## Constructors

| API | JSON kind |
| --- | --- |
| `JsonValue.null()` | null |
| `JsonValue.bool(value: bool)` | boolean |
| `JsonValue.int(value: int)` | number mapped to Incan `int` |
| `JsonValue.float(value: float)` | `Result[JsonValue, JsonError]` for finite numbers mapped to Incan `float` |
| `JsonValue.str(value: str)` | string |
| `JsonValue.string(value: str)` | string |
| `JsonValue.array(values: list[JsonValue])` | array |
| `JsonValue.object(entries: Dict[str, JsonValue])` | object |

`JsonValue.float(...)` returns an error for NaN and infinities because JSON has no spelling for those values.

## Serialization

| API | Returns |
| --- | --- |
| `JsonValue.parse(source: str)` | `Result[JsonValue, JsonError]` |
| `JsonValue.loads(source: str)` | `Result[JsonValue, JsonError]` |
| `value.to_json()` | `Result[str, JsonError]` |
| `value.to_pretty_json()` | `Result[str, JsonError]` |
| `value.dumps()` | `Result[str, JsonError]` |
| `value.debug()` | `Result[str, JsonError]` |

`JsonValue.loads` is an alias for `JsonValue.parse`. `value.dumps()` and `value.debug()` delegate to compact JSON serialization.

## Kind And Predicates

| API | Returns |
| --- | --- |
| `value.kind()` | `JsonKind` |
| `value.kind_name()` | `str` |
| `value.is_null()` | `bool` |
| `value.is_bool()` | `bool` |
| `value.is_int()` | `bool` |
| `value.is_float()` | `bool` |
| `value.is_number()` | `bool` |
| `value.is_str()` | `bool` |
| `value.is_array()` | `bool` |
| `value.is_object()` | `bool` |

`JsonKind.as_str()` returns the stable string spelling for a kind.

## Extraction

| Optional API | Required API |
| --- | --- |
| `value.as_bool() -> Option[bool]` | `value.expect_bool() -> Result[bool, JsonError]` |
| `value.as_int() -> Option[int]` | `value.expect_int() -> Result[int, JsonError]` |
| `value.as_float() -> Option[float]` | `value.expect_float() -> Result[float, JsonError]` |
| `value.as_str() -> Option[str]` | `value.expect_str() -> Result[str, JsonError]` |
| `value.as_array() -> Option[list[JsonValue]]` | `value.expect_array() -> Result[list[JsonValue], JsonError]` |
| `value.as_object() -> Option[Dict[str, JsonValue]]` | `value.expect_object() -> Result[Dict[str, JsonValue], JsonError]` |

Optional extractors return `None` for kind mismatches. Required extractors return `JsonErrorKind.Type`.

## Object Access

| API | Returns |
| --- | --- |
| `value["key"]` | `Option[JsonValue]` |
| `value.get(key: str)` | `Option[JsonValue]` |
| `value.require(key: str)` | `Result[JsonValue, JsonError]` |
| `value.require_key(key: str)` | `Result[JsonValue, JsonError]` |
| `value.contains_key(key: str)` | `bool` |
| `value.keys()` | `list[str]` |
| `value.values()` | `list[JsonValue]` |
| `value.items()` | `list[tuple[str, JsonValue]]` |

Object lookup returns `Some(value)` when a key exists, including when the value is JSON null. It returns `None` when the receiver is not an object or the key is missing. Required lookup returns `JsonErrorKind.Type` for non-objects and `JsonErrorKind.Key` for missing keys.

## Object Mutation

| API | Returns |
| --- | --- |
| `value.set(key: str, value: JsonValue)` | `Result[None, JsonError]` |
| `value.put(key: str, value: JsonValue)` | `Result[None, JsonError]` |
| `value.remove(key: str)` | `Result[Option[JsonValue], JsonError]` |
| `value.merge(other: JsonValue)` | `Result[None, JsonError]` |

Object mutation APIs return `JsonErrorKind.Type` when the receiver is not an object. `merge` also requires the argument to be an object.

## Array Access

| API | Returns |
| --- | --- |
| `value[index]` | `Option[JsonValue]` |
| `value.at(index: int)` | `Option[JsonValue]` |
| `value.require_index(index: int)` | `Result[JsonValue, JsonError]` |
| `value.len()` | `int` |
| `value.is_empty()` | `bool` |

Array lookup returns `Some(value)` for in-bounds non-negative indices. It returns `None` for non-arrays, negative indices, and out-of-range indices. Required lookup returns `JsonErrorKind.Type` for non-arrays and `JsonErrorKind.Index` for invalid indices.

## Array Mutation

| API | Returns |
| --- | --- |
| `value.push(value: JsonValue)` | `Result[None, JsonError]` |
| `value.append(value: JsonValue)` | `Result[None, JsonError]` |
| `value.extend(values: list[JsonValue])` | `Result[None, JsonError]` |
| `value.insert(index: int, value: JsonValue)` | `Result[None, JsonError]` |
| `value.remove_at(index: int)` | `Result[Option[JsonValue], JsonError]` |

Array mutation APIs return `JsonErrorKind.Type` when the receiver is not an array. `insert` rejects indices outside `0..len`.

## Traversal

| API | Returns |
| --- | --- |
| `value.pointer(path: str)` | `Result[Option[JsonValue], JsonError]` |
| `value.require_pointer(path: str)` | `Result[JsonValue, JsonError]` |
| `value.children()` | `list[JsonValue]` |
| `value.descendants()` | `list[JsonValue]` |

`pointer(...)` uses JSON Pointer syntax, not JSONPath. The empty path `""` addresses the receiver. Object and array paths use slash-separated segments such as `/items/0/name`. Pointer escaping follows RFC 6901: `~0` decodes to `~`, and `~1` decodes to `/`.

Array pointer segments are non-negative decimal indices. Empty segments, `-`, leading-zero multi-digit indices, non-digits, out-of-range indices, and indices larger than Incan's signed 64-bit `int` range do not resolve.

`pointer(...)` returns `Ok(None)` for unresolved paths and `Err(JsonError)` for malformed pointer syntax. `require_pointer(...)` returns `JsonErrorKind.Key` when the path does not resolve.

## Equality And Cloning

| API | Returns |
| --- | --- |
| `value.clone()` | `JsonValue` |
| `value.equals(other: JsonValue)` | `bool` |

## Numeric Mapping

JSON numbers are mapped by their lexical form:

| JSON spelling | Incan kind |
| --- | --- |
| integer-like JSON number | `JsonKind.Int` |
| fraction or exponent JSON number | `JsonKind.Float` |

The lexical contract matches `std.math.is_int_like` and `std.math.is_float_like`. Unsupported or out-of-range numeric payloads produce `JsonErrorKind.Number`.

## Model Interop

`JsonValue` is supported as a dynamic field inside `@derive(json)` models. It serializes and deserializes as ordinary JSON, not as a wrapper object.

## Errors

| API | Returns |
| --- | --- |
| `error.kind()` | `JsonErrorKind` |
| `error.kind_name()` | `str` |
| `error.detail()` | `str` |
| `error.message()` | `str` |

| Error kind | Used for |
| --- | --- |
| `JsonErrorKind.Parse` | JSON parse failures. |
| `JsonErrorKind.Type` | Runtime kind mismatches. |
| `JsonErrorKind.Key` | Missing object keys and pointer failures. |
| `JsonErrorKind.Index` | Invalid array indices. |
| `JsonErrorKind.Number` | Non-finite, unsupported, or out-of-range numbers. |
