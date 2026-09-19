# Strings and bytes

| Type | Holds | Generated Rust |
| --- | --- | --- |
| `str` | Unicode text | `String` |
| `bytes` | A sequence of bytes | `Vec<u8>` |

Both have frozen forms, `FrozenStr` and `FrozenBytes`, which offer the same methods.

## Literals

| Literal | Value |
| --- | --- |
| `"text"`, `'text'` | A `str`; the quote style does not change the value. |
| `"""text"""` | A multi-line `str`; line breaks inside the quotes are part of the value. |
| `f"…{expr}…"` | An f-string: a `str` with each `{expr}` replaced by the formatted value of `expr`. |
| `b"…"` | A `bytes` literal. Only ASCII characters and the escapes below are accepted; a non-ASCII character is a compile-time error. |

Escape sequences in `bytes` literals:

| Escape | Byte |
| --- | --- |
| `\n` | 0x0A |
| `\t` | 0x09 |
| `\r` | 0x0D |
| `\\` | 0x5C |
| `\0` | 0x00 |
| `\xNN` | The byte with hexadecimal value `NN` |

## Indexing and slicing

| Form | Result |
| --- | --- |
| `s[i]` | The Unicode scalar at position `i`; negative `i` counts from the end. Out of range raises `IndexError: string index out of range`. |
| `s[start:end:step]` | A new `str` of the scalars selected by the Python slice rules; each part is optional and `step` defaults to `1`. A negative `step` walks backwards (`s[::-1]` reverses). `step == 0` raises `ValueError: slice step cannot be zero`. |

Positions and lengths count Unicode scalars, not bytes. The same forms apply to `list[T]`.

## `str` methods

| Signature | Contract |
| --- | --- |
| `upper() -> str` | Uppercase copy. |
| `lower() -> str` | Lowercase copy. |
| `strip() -> str` | Copy without leading and trailing whitespace. There is no `lstrip` or `rstrip`. |
| `replace(old: str, new: str) -> str` | Copy with every occurrence of `old` replaced by `new`. |
| `split(separator: str) -> list[str]` | Pieces between occurrences of `separator`, in order. |
| `split_whitespace() -> list[str]` | Pieces separated by runs of Unicode whitespace; no empty pieces. |
| `join(parts: list[str]) -> str` | `parts` concatenated with the receiver between each pair: `", ".join(names)`. |
| `contains(needle: str) -> bool` | Whether `needle` occurs in the receiver. |
| `startswith(prefix: str) -> bool` | Whether the receiver starts with `prefix`. |
| `endswith(suffix: str) -> bool` | Whether the receiver ends with `suffix`. |
| `len() -> int` | The number of Unicode scalars; `len(s)` is the same value. |
| `is_empty() -> bool` | Whether `len()` is zero. |
| `to_string() -> str` | The receiver itself. |
| `encode(encoding: str = "utf-8") -> bytes` | The text's UTF-8 bytes. `encoding` accepts `"utf-8"` and `"utf8"`, compared case-insensitively with `_` read as `-`. A literal label naming any other codec is a compile-time error; a run-time label naming another codec raises `ValueError`. |

Membership uses the method, not the `in` operator: `s.contains("x")`.

## `bytes` methods

| Signature | Contract |
| --- | --- |
| `len() -> int` | The number of bytes; `len(b)` is the same value. |
| `decode(encoding: str = "utf-8", errors: str = "strict") -> str` | The bytes read as UTF-8 text. `encoding` follows the same rule as `str.encode`. `errors` is `"strict"`, which raises `ValueError` on the first malformed sequence, or `"replace"`, which substitutes U+FFFD for each malformed sequence; any other literal policy is a compile-time error, any other run-time policy raises `ValueError`. |

## F-strings

An f-string interpolates any expression between `{` and `}`; the value is formatted through `Display`. A format spec after `:` selects another formatting:

| Spec | Formatting |
| --- | --- |
| `{value}` | `Display` |
| `{value:?}` | `Debug`: the value's structure, for example `Point { x: 10, y: 20 }` |

See [String representation](./derives/string_representation.md) for how a type provides `Display` and `Debug`.

## See also

- [String processing](../how-to/string_processing.md) — recipes for splitting, joining, cleaning and encoding text.
- [Binary-text encoding](../how-to/binary_text_encoding.md) — moving bytes through text formats and decoding at boundaries.
- [Rust types for Python developers](../how-to/rust_types_for_python_devs.md) — how `str` and `bytes` differ from Python's.
- [Strings and formatting (tutorial)](../tutorials/book/07_strings_and_formatting.md)
