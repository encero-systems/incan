# Strings and bytes

| Type | Holds |
| --- | --- |
| `str` | Unicode text |
| `bytes` | A sequence of bytes |

Both have frozen forms for constants, `FrozenStr` and `FrozenBytes`. `FrozenStr` has every `str` method and also `is_empty() -> bool`; `FrozenBytes` has `decode` and also `len() -> int` and `is_empty() -> bool`.

## `str` and `FrozenStr` destinations

| Value | Accepted where this is expected | Value at the destination |
| --- | --- | --- |
| A `FrozenStr` | `str`, `FrozenStr` | The same text, as the destination's type. |
| A `const` declared `str` | `str`, `FrozenStr` | The same text; the constant is a `FrozenStr`. |
| A string literal (`"text"`) | `str`, `FrozenStr` | The literal's text, as the destination's type. |
| Any other `str` value, such as a parameter | `str` | Refused at a `FrozenStr` destination (`INCAN-T0001`). |

A destination is a return value, an argument, an annotated binding, a field, a collection element, the matching member of a union (`FrozenStr | int`, `str | int`), and the payload of `Some(...)` where an `Option` of the type is expected. `isinstance(value, str)` is `true` for a `str` and for a `FrozenStr`.

```incan
const NAME: str = "policy"
const LABEL: FrozenStr = "label"

def frozen_or_int() -> FrozenStr | int:
    return NAME             # accepted

def maybe_frozen() -> Option[FrozenStr]:
    return Some(NAME)       # accepted

def maybe_text() -> Option[str]:
    return Some(LABEL)      # accepted

def frozen(value: str) -> FrozenStr:
    return value            # refused: a str parameter is not a FrozenStr (INCAN-T0001)
```

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
| `s[start:end:step]` | A new `str` of the scalars from `start` up to but not including `end`, taking every `step`-th; a negative index counts from the end, each part is optional, and `step` defaults to `1`. A negative `step` walks backwards (`s[::-1]` reverses). `step == 0` fails with `ValueError`. |

Positions and lengths count Unicode scalars, not bytes. The same forms apply to `list[T]`.

## `str` methods

| Signature | Contract |
| --- | --- |
| `upper() -> str` | Uppercase copy. |
| `lower() -> str` | Lowercase copy. |
| `strip() -> str` | Copy without leading and trailing whitespace. There is no `lstrip` or `rstrip`. |
| `replace(old: str, new: str) -> str` | Copy with every occurrence of `old` replaced by `new`. |
| `split(separator: str) -> list[str]` | Pieces between occurrences of `separator`, in order. Without a separator, the result is a one-item list holding the receiver. |
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

An f-string interpolates any expression between `{` and `}`; the value is formatted through `Display`. The only supported format spec is `?`, which selects `Debug`. Any other format spec is refused with `INCAN-T0001`.

| Spec | Formatting |
| --- | --- |
| `{value}` | The value's display text (see [Display](#display)) |
| `{value:?}` | `Debug`: the value's structure, for example `Point { x: 10, y: 20 }` |

See [String representation](./derives/string_representation.md) for how a type provides `Display` and `Debug`.

## Display

`print(value)`, `println(value)`, `str(value)` and an f-string `{value}` part render the same text for the same value.

| Value | Displayed text |
| --- | --- |
| `str`, `FrozenStr` | The text itself, unquoted |
| `int` and the exact-width integers | Decimal digits |
| `bool` | `true` or `false` |
| `float` | Decimal digits with a decimal point or an exponent, below |
| `f32`, `f64` | The shortest digits that round-trip, below |
| A `model`, `class`, `enum` or `newtype` that defines `__str__` | What `__str__` returns |
| An enum that declares values | The variant's value |
| A type that adopts `Error` and has no `__str__` | What `message()` returns (see [Displaying an error](./stdlib_traits/error.md#displaying-an-error)) |
| Tuple | `(10, 20)` |
| `list` | `[1, 2, 3]` |
| `dict` | `{"a": 1}` |
| `set` | `{1, 2}` |
| `Option` | `Some(1)` or `None` |
| `Result` | `Ok(2)` or `Err("bad")` |
| `FrozenList`, `FrozenSet`, `FrozenDict` | As `list`, `set` and `dict`: `[1, 2, 3]`, `{1, 2}`, `{"a": 1}` |

- Inside a tuple, list, dict, set, frozen collection, `Option` or `Result`, every element or payload displays as its `{value:?}` structure: a `str` is quoted (`["a", "b"]`), a `float` keeps its decimal point and uses an unsigned exponent from `1e16` up and below `1e-4` (`[100.0, 1e16]`), a model or class shows its fields even when its type defines `__str__` (`[Point { x: 1, y: 2 }]`), and an enum value shows its variant (`[Red]`). The entry order of a set or a dict is unspecified.
- A `float` always shows a decimal point or an exponent. An integral value keeps its decimal point (`100.0`); the shortest digits that round-trip are used (`1.5`, `0.30000000000000004`); positional notation holds while the magnitude is at least `1e-4` and below `1e16` (`10000000000.0`), and outside that range the value uses an exponent with an explicit sign and at least two digits (`1e+16`, `1.5e-07`); the non-finite values are `inf`, `-inf` and `nan`.
- An `f32` or `f64` displays the shortest digits that round-trip in positional notation, with no forced decimal point (`100`, `1.5`); its non-finite values are `inf`, `-inf` and `NaN`.

Refused in every display position (`INCAN-T0103`):

- a union value (`int | str`); a member bound by narrowing displays as its own type (see [Union types](./union_types.md#narrowing));
- a `Generator`;
- a function;
- `bytes`, `FrozenBytes`;
- a `model`, `class`, `enum` or `newtype` value whose type provides no `Display`: it defines no `__str__`, is not an enum that declares values, and does not adopt `Error`. `@derive(Display)` provides nothing.

```incan
model Point:
    x: int
    y: int

enum Color:
    Red

enum Level(str):
    WARN = "warn"

def main() -> None:
    items: list[int] = [1, 2, 3]
    print(items)              # accepted
    println(str(items))       # accepted
    println(f"{Level.WARN}")  # accepted
    point = Point(x=1, y=2)
    println(f"{point:?}")     # accepted
    println(point)            # refused: Point defines no __str__ (INCAN-T0103)
    println(str(Color.Red))   # refused: Color declares no values (INCAN-T0103)
```

| Expression | Output |
| --- | --- |
| `print(items)`, `str(items)`, `f"{items}"` | `[1, 2, 3]` |
| `f"{Level.WARN}"` | `warn` |
| `f"{point:?}"` | `Point { x: 1, y: 2 }` |

### `Display` bounds

A type argument for a type parameter bounded by `Display` (`def show[T with Display](value: T)`) satisfies the bound when it provides `Display` by the rule above. `int`, `float`, `bool`, `str` and `FrozenStr` satisfy it.

- Refused (`INCAN-T0103`): a type argument whose values have no printed form, as listed above.
- Refused (`INCAN-T0001`): a tuple, list, dict, set, `Option` or `Result` type argument. Each displays its structure in a display position but does not provide `Display`.

```incan
model Point:
    x: int

enum Level(str):
    WARN = "warn"

def show[T with Display](value: T) -> str:
    return f"{value}"

def main() -> None:
    a = show(1)               # accepted
    b = show(Level.WARN)      # accepted
    c = show(Point(x=1))      # refused: Point defines no __str__ (INCAN-T0103)
    d = show([1, 2])          # refused: list does not provide Display (INCAN-T0001)
```

## See also

- [String processing](../how-to/string_processing.md) — recipes for splitting, joining, cleaning and encoding text.
- [Displaying a value with no printed form](../how-to/error_messages.md#displaying-a-value-with-no-printed-form) — what to display instead when a value is refused with `INCAN-T0103`.
- [Binary-text encoding](../how-to/binary_text_encoding.md) — moving bytes through text formats and decoding at boundaries.
- [Rust types for Python developers](../how-to/rust_types_for_python_devs.md) — how `str` and `bytes` differ from Python's.
- [Strings and formatting (tutorial)](../tutorials/book/07_strings_and_formatting.md)
