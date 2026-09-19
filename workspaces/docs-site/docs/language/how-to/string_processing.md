# String processing

This page collects practical recipes for working with `str` and `bytes`.

## Split and index safely

```incan
line = "alice,30,engineer"
parts = line.split(",")

name = parts[0]
age = parts[1]
role = parts[2]
```

> Note: Indexing panics if out of range. If you need fallible parsing, validate the length before indexing (or use a
> `Result`-returning helper function).

## Build strings

```incan
words = ["hello", "world"]
sentence = " ".join(words)
println(sentence.upper())  # HELLO WORLD
```

## Clean input

```incan
raw_input = "  user@example.com  "
email = raw_input.strip().lower()
println(email)  # user@example.com
```

`strip()` trims both ends; there is no one-sided variant, so trim and then split when only one side matters.

## Test for a substring

`contains` is a method; the `in` operator does not apply to strings.

```incan
sentence = "the quick brown fox"

if sentence.contains("quick"):
    println("Found it!")
```

## Replace text

```incan
text = "hello world"
println(text.replace("world", "incan"))  # hello incan
```

## Move between text and bytes

`encode` gives the UTF-8 bytes of a `str`; `decode` reads UTF-8 bytes back. Keep `decode` strict at the point where bytes enter the program, and use `errors="replace"` only for text you copy for display:

```incan
payload: bytes = "héllo".encode()
println(len(payload))                      # 6
println(payload.decode())                  # héllo
println(b"\xff".decode(errors="replace"))  # U+FFFD replacement character
```

Strict decoding of malformed input raises `ValueError`, the same failure `int("x")` raises. Other codecs are not built into `str` and `bytes`: files use `std.fs` (`read_text` / `write_text`), and hexadecimal, base64 and similar text formats live in [`std.encoding`](binary_text_encoding.md).

## See also

- [Strings and bytes (Reference)](../reference/strings.md) — every `str` and `bytes` method with its contract
- [Strings and formatting (Tutorial)](../tutorials/book/07_strings_and_formatting.md)
- [File I/O (How-to)](file_io.md)
