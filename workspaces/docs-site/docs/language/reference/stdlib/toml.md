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

With `from std import toml`, `@derive(toml)` supplies both traits for a model. No additional `@rust.derive` is required. `TomlDatetime` already implements both traits. Use the methods on `TomlValue` to serialize dynamic documents; `TomlValue` does not implement these model codec traits.

## `TomlValue`

A parsed value with its original source context and one of the seven [TOML kinds](#tomlkind). TOML has no null value. Parsed document roots are tables; nested values can have any kind.

### Parsing and serialization

| API | Return type | Contract |
| --- | --- | --- |
| `TomlValue.parse(source: str)` | `Result[TomlValue, TomlError]` | Static equivalent of the module-level `parse`. |
| `value.to_toml()` | `Result[str, TomlError]` | Serialize the value as a document. Scalar document roots produce an error with kind `TomlErrorKind.Encode`. |
| `value.to_toml_pretty()` | `Result[str, TomlError]` | Serialize with expanded formatting, with the same document-shape restrictions as `to_toml`. |
| `value.clone()` | `TomlValue` | Copy the selected tree and share its original source text. |
| `value.location()` | `Option[TomlLocation]` | Return the original value span when available, without reparsing the document. |

### Typed lookup

| API | Return type | Contract |
| --- | --- | --- |
| `value.get[T](path: str)` | `Result[T, TomlError]` | Read a required value at a dotted table path. Missing values produce `Missing`; incompatible values or intermediate containers produce `Type`; malformed paths produce `InvalidPath`. |
| `value.get_optional[T](path: str)` | `Result[Option[T], TomlError]` | Return `Ok(None)` only when the path is absent. A present invalid value or an incompatible intermediate container is an error. |
| `value.get_key[T](key: str)` | `Result[T, TomlError]` | Read one required literal table key. An empty key and keys containing dots are valid. A non-table receiver produces `Type`. |

All three methods declare `T with (DeserializeOwned, RustSerialize)`, where the bounds are the imported Rust traits `serde::de::DeserializeOwned` and `serde::Serialize`. Supported values include `str`, `int`, `float`, `bool`, `TomlDatetime`, lists, string-keyed dictionaries, and models whose members are supported. Apply `@derive(toml)` to models; no Rust annotations are needed. Every member of a collection is validated. Unsupported native trait implementations fail at compilation rather than becoming runtime lookup errors.

Typed lookup is strict about TOML kinds, including inside collections: integers do not satisfy `float`, and datetime values do not satisfy `str`. Selected model decoding validates the model's fields. Errors retain the document path, including array indices such as `project.targets[2]`, and source coordinates when available. Decoded values own their data.

#### Path syntax

`get`, `get_optional`, and `get_value` split their `path` argument on dots. Each component is a decoded table key and must be nonempty. Empty paths, leading or trailing dots, and consecutive dots produce `InvalidPath`, even when an earlier key is absent. Paths are relative to the receiving value.

`get[str]("project.name")` always traverses `project` and then `name`. It never falls back to a literal key named `project.name`; use `get_key[str]("project.name")` for that key. There is no quoting, escaping, or array-index syntax in lookup paths. Brackets in a component are literal key characters. Read a typed list or use dynamic `get_index` to access an array element.

### Dynamic inspection

| API | Return type | Contract |
| --- | --- | --- |
| `value.kind()` | `TomlKind` | Return the value's runtime kind. |
| `value.get_value(path: str)` | `Result[TomlValue, TomlError]` | Read a required dynamic snapshot using the same path and error rules as `get`. |
| `value.get_value_key(key: str)` | `Result[TomlValue, TomlError]` | Read one required literal table member. |
| `value.get_index(index: int)` | `Result[TomlValue, TomlError]` | Read an array element. Negative or out-of-range indices produce `Missing`; a non-array receiver produces `Type`. |

Successful dynamic lookups copy the selected subtree and share the original source text and its coordinates. They do not mutate the document. `TomlValue` is not a typed `get[T]` target; use `get_value` for dynamic inspection.

### Value extraction

| API | Return type | Accepted kind |
| --- | --- | --- |
| `value.as_int()` | `Result[int, TomlError]` | `TomlKind.Integer` |
| `value.as_float()` | `Result[float, TomlError]` | `TomlKind.Float`, including TOML infinity and NaN |
| `value.as_bool()` | `Result[bool, TomlError]` | `TomlKind.Boolean` |
| `value.as_str()` | `Result[str, TomlError]` | `TomlKind.String` |
| `value.as_datetime()` | `Result[TomlDatetime, TomlError]` | `TomlKind.Datetime` |
| `value.as_array()` | `Result[list[TomlValue], TomlError]` | `TomlKind.Array` |
| `value.as_table()` | `Result[Dict[str, TomlValue], TomlError]` | `TomlKind.Table` |

Every extractor returns an error with kind `Type` for a different kind. No extractor coerces between kinds: an integer does not satisfy `as_float`, and a datetime does not satisfy `as_str`. Extracted arrays and tables own copies of their member values, including nested contents.

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

The source span returned by `locate` or `TomlValue.location`.

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
| `kind` | `TomlErrorKind` | Parse, lookup, type, or serialization category. |
| `detail` | `str` | Diagnostic text. |
| `path` | `Option[str]` | Document path for a lookup or extraction error, including array indices. An empty string identifies the root; non-lookup errors have `None`. |
| `line` | `Result[int, TomlError]` | One-based starting line, when a source span is available. |
| `column` | `Result[int, TomlError]` | One-based Unicode character column, when available. |
| `byte_start` | `Result[int, TomlError]` | Zero-based UTF-8 start offset, when available. |
| `byte_end` | `Result[int, TomlError]` | Exclusive zero-based UTF-8 end offset, when available. |

`error.message() -> str` returns `"{path}: {detail}"` for a nonempty path, and `detail` otherwise.

Coordinates follow the same conventions as `TomlLocation`. Decode errors can lack a corresponding source token; their position fields are then `None`. Missing values and invalid paths have no corresponding value span. Serialization errors always have `None` positions because there is no input document.

## `TomlErrorKind`

| Variant | `as_str()` result | Operations |
| --- | --- | --- |
| `TomlErrorKind.Decode` | `"decode"` | `parse`, `deserialize`, and `locate` |
| `TomlErrorKind.Encode` | `"serialize"` | `serialize`, `serialize_pretty`, and the `TomlValue.to_toml` methods |
| `TomlErrorKind.Missing` | `"missing"` | Required lookup of an absent key or array index |
| `TomlErrorKind.InvalidPath` | `"invalid_path"` | Malformed dotted lookup path |
| `TomlErrorKind.Type` | `"type"` | Typed lookup, extraction, or traversal through an incompatible container |

`kind.as_str() -> str` returns the stable spelling listed above.

## Serialization limits

Serialization preserves TOML values, including datetime kinds. It does not preserve comments, original key order, or authored formatting. Pretty serialization has the same limitation. This module has no API for editing an authored document while preserving those details.
