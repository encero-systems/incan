# `std.toml` reference

`std.toml` parses TOML documents, serializes typed values, and reports source locations. Its functions accept and return text; they do not perform file I/O. For a worked example, see [Read and write TOML data](../../how-to/toml.md).

## Module functions

| Function | Return type | Contract |
| --- | --- | --- |
| `parse(source: str)` | `Result[TomlValue, TomlError]` | Parse a complete document into a table-valued root. Syntax errors have kind `TomlErrorKind.Decode`. |
| `deserialize[T with TomlDeserialize](source: str)` | `Result[T, TomlError]` | Decode the original text as `T`. Syntax errors, missing required fields, and incompatible field types have kind `TomlErrorKind.Decode`. |
| `serialize[T with TomlSerialize](value: T)` | `Result[str, TomlError]` | Serialize a supported document with default formatting. Unsupported document shapes, including scalar roots, produce an error with kind `TomlErrorKind.Encode`. |
| `serialize_pretty[T with TomlSerialize](value: T)` | `Result[str, TomlError]` | Serialize with expanded formatting. Supported shapes and errors are the same as for `serialize`. |
| `locate(source: str, path: list[str])` | `Result[Option[TomlLocation], TomlError]` | Find the source span of a value reached through table keys. Missing keys or unavailable spans return `Ok(None)`; malformed source produces an error with kind `TomlErrorKind.Decode`. |

The `source` arguments contain TOML text, not filenames. Deserialization reads that original text so structural errors can retain source locations. Nested models and lists of models serialize as tables and arrays of tables.

### `locate` paths

Each `path` element is one decoded table key. For example, `["dependencies", "my.library", "version"]` addresses a key literally named `my.library` inside `dependencies`, followed by its `version` key. Dots within an element are not path separators.

Table-key paths include inline tables. Array-index traversal is not supported. Returned spans identify values, not key tokens. `locate` reparses `source` for each call; it does not retain a parsed document between calls.

## Serialization traits

| Trait | Contract |
| --- | --- |
| `TomlSerialize` | Required by `serialize` and `serialize_pretty`. |
| `TomlDeserialize` | Required by `deserialize`. Decoded values own their data and do not borrow the input text. |

With `from std import toml`, `@derive(toml)` supplies both traits for a model. No additional `@rust.derive` is required. `TomlValue` and `TomlDatetime` already implement both traits.

## `TomlValue`

A dynamic value with one of the seven [TOML kinds](#tomlkind). TOML has no null value. Parsed document roots are tables; nested values can have any kind.

### Parsing and serialization

| API | Return type | Contract |
| --- | --- | --- |
| `TomlValue.parse(source: str)` | `Result[TomlValue, TomlError]` | Static equivalent of the module-level `parse`. |
| `value.to_toml()` | `Result[str, TomlError]` | Serialize the value as a document. Scalar document roots produce an error with kind `TomlErrorKind.Encode`. |
| `value.clone()` | `TomlValue` | Copy the value, including nested array and table contents. |

### Kind and lookup

| API | Return type | Contract |
| --- | --- | --- |
| `value.kind()` | `TomlKind` | Return the value's runtime kind. |
| `value.get(key: str)` | `Option[TomlValue]` | Return a table member. A missing key or non-table receiver returns `None`. |
| `value.get_index(index: int)` | `Option[TomlValue]` | Return an array member. A negative or out-of-range index, or a non-array receiver, returns `None`. |

Successful lookups return owned snapshots and copy the entire selected subtree. They do not borrow or mutate the original document.

### Value extraction

| API | Return type | Accepted kind |
| --- | --- | --- |
| `value.as_int()` | `Option[int]` | `TomlKind.Integer` |
| `value.as_float()` | `Option[float]` | `TomlKind.Float`, including TOML infinity and NaN |
| `value.as_bool()` | `Option[bool]` | `TomlKind.Boolean` |
| `value.as_str()` | `Option[str]` | `TomlKind.String` |
| `value.as_datetime()` | `Option[TomlDatetime]` | `TomlKind.Datetime` |
| `value.as_array()` | `Option[list[TomlValue]]` | `TomlKind.Array` |
| `value.as_table()` | `Option[Dict[str, TomlValue]]` | `TomlKind.Table` |

Every extractor returns `None` for a different kind. No extractor coerces between kinds: an integer does not satisfy `as_float`, and a datetime does not satisfy `as_str`. Extracted arrays and tables own copies of their member values, including nested contents.

## `TomlKind`

| Variant | `as_str()` result |
| --- | --- |
| `TomlKind.String` | `"string"` |
| `TomlKind.Integer` | `"integer"` |
| `TomlKind.Float` | `"float"` |
| `TomlKind.Boolean` | `"boolean"` |
| `TomlKind.Datetime` | `"datetime"` |
| `TomlKind.Array` | `"array"` |
| `TomlKind.Table` | `"table"` |

`kind.as_str() -> str` returns the stable spelling listed above.

## `TomlDatetime`

A TOML offset datetime, local datetime, local date, or local time. These forms retain their distinct value semantics through serialization. Local forms have no implicit timezone.

| API | Return type | Contract |
| --- | --- | --- |
| `datetime.as_str()` | `str` | Return the canonical text for the retained datetime form, including an explicit offset when present. |
| `datetime.clone()` | `TomlDatetime` | Copy the datetime value. |

## `TomlLocation`

The source span returned by `locate`.

| Public field | Type | Contract |
| --- | --- | --- |
| `line` | `int` | One-based line containing the start of the value. |
| `column` | `int` | One-based Unicode character position within that line. |
| `byte_start` | `int` | Zero-based UTF-8 offset of the start of the value. |
| `byte_end` | `int` | Exclusive zero-based UTF-8 end offset. |

`TomlLocation` implements `Eq`; equality compares its location fields. Character columns and byte offsets use different units and can differ for non-ASCII input.

## `TomlError`

The error returned by parsing, deserialization, source-location lookup, and serialization.

| Public field | Type | Contract |
| --- | --- | --- |
| `kind` | `TomlErrorKind` | Decode or serialization category. |
| `detail` | `str` | Diagnostic text. |
| `line` | `Option[int]` | One-based starting line, when a source span is available. |
| `column` | `Option[int]` | One-based Unicode character column, when available. |
| `byte_start` | `Option[int]` | Zero-based UTF-8 start offset, when available. |
| `byte_end` | `Option[int]` | Exclusive zero-based UTF-8 end offset, when available. |

`error.message() -> str` returns `error.detail`.

Coordinates follow the same conventions as `TomlLocation`. Decode errors can lack a corresponding source token; their position fields are then `None`. Serialization errors always have `None` positions because there is no input document.

## `TomlErrorKind`

| Variant | `as_str()` result | Operations |
| --- | --- | --- |
| `TomlErrorKind.Decode` | `"decode"` | `parse`, `deserialize`, and `locate` |
| `TomlErrorKind.Encode` | `"serialize"` | `serialize`, `serialize_pretty`, and `TomlValue.to_toml` |

`kind.as_str() -> str` returns the stable spelling listed above.

## Serialization limits

Serialization preserves TOML values, including datetime kinds. It does not preserve comments, original key order, or authored formatting. Pretty serialization has the same limitation. This module has no API for editing an authored document while preserving those details.
