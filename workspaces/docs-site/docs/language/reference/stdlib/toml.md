# std.toml

Read project manifests and write generated TOML documents with `std.toml`. The module uses the existing Rust `toml` parser through ordinary interop; its public value access, error coordinates, and document workflow are authored in Incan.

## Typed documents

Use `@derive(toml)` (the module’s `TomlSerialize` and `TomlDeserialize` traits) to adopt serialization and owned deserialization on your models, then decode directly from the original source. This retains source spans for structural errors such as a string where an integer is required.

```incan
from std.toml import deserialize, serialize_pretty
from std import toml

@derive(toml)
model Project:
    name: str
    version: str

project = deserialize[Project]('name = "demo"\nversion = "0.1.0"')?
text = serialize_pretty(project)?
```

`deserialize[T](source)` returns `Result[T, TomlError]`. `serialize(value)` writes compact TOML; `serialize_pretty(value)` writes expanded tables and arrays. Both return `Result[str, TomlError]`. Unsupported document roots, including scalar values, return a serialization error. Generated lock models can contain nested models and lists of models; these serialize as tables and arrays of tables.

## Dynamic values

`parse(source)` and `TomlValue.parse(source)` return a table-backed document. `kind()` returns a `TomlKind` value: `String`, `Integer`, `Float`, `Boolean`, `Datetime`, `Array`, or `Table`. Use `kind().as_str()` for its stable lowercase spelling. TOML does not contain null values.

Use `get(key)` for tables and `get_index(index)` for arrays. Both return `Option[TomlValue]`; negative or out-of-range array indices return `None`. `as_int`, `as_float`, `as_bool`, `as_str`, `as_array`, and `as_table` return an optional payload without coercion. Lookups copy the entire selected subtree; returned arrays and tables are owned copies as well. Retain a lookup result when traversing it repeatedly.

`as_datetime()` returns `Option[TomlDatetime]`. Its `as_str()` retains the canonical offset datetime, local datetime, date, or time spelling. Local values are not assigned an implicit timezone. `to_toml()` serializes a dynamic document without converting date/time values into strings.

## Diagnostics

`TomlError` carries `kind`, `detail`, `line`, `column`, `byte_start`, and `byte_end`. `message()` returns the detail. Decode errors use `TomlErrorKind.Decode`; write errors use `TomlErrorKind.Encode`.

Line and column are one-based Unicode character coordinates. Byte offsets are zero-based UTF-8 offsets with an exclusive end. Positions are optional: serialization has no input source, and deserializers may report an error without a corresponding source token.

For validation rules owned by your application, `locate(source, path)` returns `Result[Option[TomlLocation], TomlError]`. Supply a list of decoded table keys, such as `["dependencies", "my.library", "unknown_option"]`; the dot inside `my.library` remains part of that key. A location identifies the selected value, with the same line, column, and byte fields described above. Missing paths return `None`; malformed source returns a located error.

This query reparses the original text to recover spans, so use it when constructing a custom diagnostic after inspecting dynamic values. It supports table-key paths, including inline tables, and does not interpret path elements as array indices.

## Formatting is not preserved

Parsing and serializing preserve TOML values, including date/time kinds. They do not preserve comments, key order, or authored formatting. This surface supports reading manifests and writing generated locks. Editing an authored manifest in place without losing its formatting requires a separate document-preserving editor; that API is deferred beyond this feature.
