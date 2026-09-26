# Strings and bytes

| Type | Holds | Generated Rust |
| --- | --- | --- |
| `str` | Unicode text | `String` |
| `bytes` | A sequence of bytes | `Vec<u8>` |

Both have frozen forms for constants, `FrozenStr` and `FrozenBytes`. `FrozenStr` has every `str` method and also `is_empty() -> bool`; `FrozenBytes` has `decode` and also `len() -> int` and `is_empty() -> bool`.

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
| `split(separator: str) -> list[str]` | Pieces between occurrences of `separator`, in order. The separator may be omitted, in which case the result is a one-item list holding the receiver; use `split_whitespace()` for Python's `split()` with no argument. |
| `split_whitespace() -> list[str]` | Pieces separated by runs of Unicode whitespace; no empty pieces. |
| `join(parts: list[str]) -> str` | `parts` concatenated with the receiver between each pair: `", ".join(names)`. |
| `contains(needle: str) -> bool` | Whether `needle` occurs in the receiver. |
| `startswith(prefix: str) -> bool` | Whether the receiver starts with `prefix`. |
| `endswith(suffix: str) -> bool` | Whether the receiver ends with `suffix`. |
| `len() -> int` | The number of Unicode scalars; `len(s)` is the same value. |
| `to_string() -> str` | The receiver itself. |
| `encode(encoding: str = "utf-8") -> bytes` | The text's UTF-8 bytes. `encoding` accepts `"utf-8"` and `"utf8"`, compared case-insensitively with `_` read as `-`. A literal label naming any other codec is a compile-time error; a run-time label naming another codec raises `ValueError`. |

Membership uses the method, not the `in` operator: `s.contains("x")`.

## `bytes` methods

`len(b)` is the number of bytes.

| Signature | Contract |
| --- | --- |
| `decode(encoding: str = "utf-8", errors: str = "strict") -> Result[str, ValidationError]` | `Ok` of the bytes read as UTF-8 text. `encoding` follows the same rule as `str.encode`, except that a run-time label naming another codec returns `Err` with `code` `unknown-encoding`. `errors` is `"strict"`, which returns `Err` with `code` `invalid-utf8` at the first malformed sequence (the message names its byte offset), or `"replace"`, which substitutes U+FFFD for each malformed sequence and always returns `Ok`; any other literal policy is a compile-time error, any other run-time policy returns `Err` with `code` `unknown-errors-policy`. |

## F-strings

An f-string interpolates any expression between `{` and `}`. A format spec after `:` selects the formatting:

| Spec | Formatting |
| --- | --- |
| `{value}` | The value's [display](#display) text |
| `{value:?}` | `Debug`: the value's structure, for example `Point { x: 10, y: 20 }` |

See [String representation](./derives/string_representation.md) for how a type provides `Display` and `Debug`.

## Display

`print(value)`, `println(value)`, `str(value)` and an f-string `{value}` part display a value under one rule: all four render the same text for the same value.

| Value | Displayed text |
| --- | --- |
| `str` | The text itself, unquoted |
| `int` and the exact-width integers | Decimal digits |
| `bool` | `true` or `false` |
| `float` | Python's spelling, below |
| A type that defines `__str__` | What `__str__` returns |
| A value enum | The variant's value |
| Tuple | `(10, 20)` |
| `list` | `[1, 2, 3]` |
| `dict` | `{"a": 1}` |
| `set` | `{1, 2}` |
| `Option` | `Some(1)` or `None` |
| `Result` | `Ok(2)` or `Err("bad")` |

Inside a tuple, list, dict, set, `Option` or `Result`, a `str` element or payload is quoted (`["a", "b"]`), and the entry order of a set or a dict is unspecified.

A `float` renders as Python spells it: always visibly a float. An integral value keeps its decimal point (`100.0`, never `100`), the shortest digits that round-trip are used (`1.5`, `0.30000000000000004`), positional notation holds while the magnitude is at least `1e-4` and below `1e16` (`10000000000.0`) and switches to an exponent with an explicit sign and at least two digits outside that range (`1e+16`, `1.5e-07`), and the non-finite values are `inf`, `-inf`, and `nan`. The exact `f32`/`f64` carriers keep Rust's own `Display`.

A value with no printed form is refused at check time with `INCAN-T0103`, in every display position: a union value (`int | str`), a `Generator`, a function, `bytes`, and a model or class whose type defines no `__str__` and adopts neither `Display` nor `Error`. A member bound by narrowing a union displays as its own type (see [Union types](./union_types.md)).

```incan
model Point:
    x: int
    y: int

def main() -> None:
    items: list[int] = [1, 2, 3]
    print(items)                # [1, 2, 3]
    println(str(items))         # [1, 2, 3]
    println(f"{items}")         # [1, 2, 3]
    maybe: Option[int] = None
    println(maybe)              # None
    point = Point(x=1, y=2)
    println(f"{point:?}")       # Point { x: 1, y: 2 }
    println(point)              # refused: INCAN-T0103
    println(str(point))         # refused: INCAN-T0103
    println(f"{point}")         # refused: INCAN-T0103
```

The how-to [Displaying a value with no printed form](../how-to/error_messages.md#displaying-a-value-with-no-printed-form) covers the refused cases.

## See also

- [String processing](../how-to/string_processing.md) — recipes for splitting, joining, cleaning and encoding text.
- [Binary-text encoding](../how-to/binary_text_encoding.md) — moving bytes through text formats and decoding at boundaries.
- [Rust types for Python developers](../how-to/rust_types_for_python_devs.md) — how `str` and `bytes` differ from Python's.
- [Strings and formatting (tutorial)](../tutorials/book/07_strings_and_formatting.md)
