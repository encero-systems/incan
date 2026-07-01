# Strings and bytes

Incan has two core “string-like” types:

- `str`: text (Unicode). The current backend compiles this to Rust `String`.
- `bytes`: binary data. The current backend compiles this to Rust `Vec<u8>`.

`str` is for user-facing text. `bytes` is for file/network/crypto data where you want raw bytes.

!!! note "Coming from Python?"
    Method names are intentionally familiar (`upper`, `lower`, `strip`, `split`, `replace`, …), but Incan is statically typed and `contains()` is a method (rather than Python’s `in` operator).

!!! note "Coming from Rust?"
    Incan hides most ownership/borrowing details. You write `str` and the compiler handles Rust interop details in generated code.

## Quick reference

| Incan                | Notes                             |
| -------------------- | --------------------------------- |
| `s.upper()`          | Uppercase string                  |
| `s.lower()`          | Lowercase string                  |
| `s.strip()`          | Trim whitespace on both sides     |
| `s.split(",")`       | Split into `list[str]`            |
| `", ".join(xs)`      | Join `list[str]` with a separator |
| `s.contains("x")`    | Substring check (`bool`)          |
| `s.replace("a", "b")`| Replace all occurrences           |

## Indexing and slicing

Incan supports Python-style indexing and slicing for strings and lists.

- **Indexing**: `s[i]`
    - Supports **negative indices** (e.g. `s[-1]` is the last character).
    - Indexing is based on **Unicode scalars** (Rust `char`), not bytes.
    - Out-of-range panics with `IndexError: string index out of range`.
- **Slicing**: `s[start:end:step]`
    - Each component is optional (e.g. `s[:3]`, `s[1:]`, `s[::2]`, `s[::-1]`).
    - `step` defaults to `1`.
    - `step == 0` panics with `ValueError: slice step cannot be zero`.
    - Negative `step` is supported (e.g. `s[::-1]` reverses a string).

```incan
def main() -> None:
    s = "héllo"
    print(s[1])     # "é"
    print(s[-1])    # "o"
    print(s[1:4])   # "éll"
    print(s[::2])   # "hlo"
    print(s[::-1])  # "olléh"
```

The same slicing rules apply to `list[T]`:

```incan
def main() -> None:
    xs: list[int] = [1, 2, 3]
    for x in xs[::-1]:
        print(x)
```

## Common string methods

### Case conversion

```incan
text = "Hello World"
println(text.upper())  # HELLO WORLD
println(text.lower())  # hello world
```

### Whitespace trimming

```incan
padded = "  hello  "
println(padded.strip())  # "hello"
```

> Note: Incan currently only has `strip()` (both sides). Python’s `lstrip()` / `rstrip()` are not yet implemented.

### Splitting strings

```incan
csv = "alice,bob,carol"
names = csv.split(",")  # ["alice", "bob", "carol"]

first = names[0]  # "alice"
```

### Joining strings

```incan
names = ["alice", "bob", "carol"]
result = ", ".join(names)  # "alice, bob, carol"
```

> Note: The separator is the receiver and the list is the argument: `", ".join(names)`.

### Substring check

```incan
sentence = "the quick brown fox"

if sentence.contains("quick"):
    println("Found it!")
```

### String replacement

```incan
text = "hello world"
result = text.replace("world", "incan")  # "hello incan"
```

## F-strings (formatted strings)

Incan supports Python-style f-strings:

```incan
name = "Alice"
age = 30
println(f"Name: {name}, Age: {age}")
```

### Debug formatting

Use `:?` for debug output (shows type structure):

```incan
model Point:
    x: int
    y: int

p = Point(x=10, y=20)
println(f"Debug: {p:?}")  # Point { x: 10, y: 20 }
```

See [String Representation](./derives/string_representation.md) for details on Debug vs Display formatting.

## String literals

```incan
# Single or double quotes
s1 = "hello"
s2 = 'hello'

# Multiline strings (triple quotes)
multi = """
This is a
multiline string
"""

# F-strings
formatted = f"Value: {x}"
```

## Bytes (binary data)

The `bytes` type represents binary data as a sequence of bytes (current backend: Rust `Vec<u8>`).

### Byte string literals

Use the `b"..."` prefix for byte strings:

```incan
# ASCII byte string
data = b"Hello"

# Hex escapes for arbitrary bytes
binary = b"\x00\x01\x02\xff"

# Common escapes
newline = b"\n"
tab = b"\t"
null = b"\0"
```

Supported escape sequences:

| Escape | Meaning                       |
| ------ | ----------------------------- |
| `\n`   | Newline                       |
| `\t`   | Tab                           |
| `\r`   | Carriage return               |
| `\\`   | Backslash                     |
| `\0`   | Null byte                     |
| `\xNN` | Hex byte (e.g. `\xff` = 255)  |

> Note: Byte strings only accept ASCII characters. Non-ASCII characters produce an error.

### Type annotation

```incan
def process_binary(data: bytes) -> bytes:
    return data
```

!!! note "Coming from Python?"
    Python’s `bytes` is immutable. Incan’s `bytes` currently lowers to Rust `Vec<u8>`, which is mutable.

### When to use `bytes` vs `str`

| Use case                    | Type    |
| --------------------------- | ------- |
| Text, user-facing content   | `str`   |
| File contents (text)        | `str`   |
| Binary files (images, etc.) | `bytes` |
| Network protocols           | `bytes` |
| Cryptographic operations    | `bytes` |
| Raw file I/O                | `bytes` |

## See also

- [String Representation](./derives/string_representation.md) — Debug and Display formatting
- [String processing (How-to)](../how-to/string_processing.md) — practical recipes (split/join/cleaning)
- [Strings and formatting (Tutorial)](../tutorials/book/07_strings_and_formatting.md)
- [File I/O (How-to)](../how-to/file_io.md) — reading text vs bytes
- [Examples: Strings][examples-strings] — string method examples
- [Examples: Bytes I/O][examples-bytes-io] — binary data examples

[examples-strings]: https://github.com/encero-systems/incan/blob/main/examples/simple/strings.incn
[examples-bytes-io]: https://github.com/encero-systems/incan/blob/main/examples/advanced/bytes_io.incn
