# Understanding Rust types (coming from Python)

When you use Rust interop (`rust::...` imports), you’ll sometimes see Rust types in signatures and docs. This page explains the common ones and how they map to Incan.

## Quick mapping

| Rust type      | Incan mental model | Notes                                                              |
| -------------- | ------------------ | ------------------------------------------------------------------ |
| `Vec[T]`       | `List[T]`          | Growable list                                                      |
| `HashMap[K, V]`| `Dict[K, V]`       | Key/value map                                                      |
| `HashSet[T]`   | `Set[T]`           | Unordered unique items                                             |
| `String`       | `str`              | Owned string                                                       |
| `&str`         | `str`              | Borrowed string slice — avoid as an API type in Incan when you can |
| `Option[T]`    | `Option[T]`        | `Some(x)` or `None`                                                |
| `Result[T, E]` | `Result[T, E]`     | `Ok(x)` or `Err(e)`                                                |
| `Instant`      | “time point”       | For measuring elapsed time                                         |
| `Duration`     | “time span”        | Length of time                                                     |

Note: In Incan type annotations, `Vec[T]` is accepted as an alias for `List[T]` to mirror Rust APIs.

## Dict vs HashMap

In normal Incan code, prefer `Dict` (and literals like `{}`).

When interoperating with Rust crates, you may encounter `HashMap` because that’s what Rust APIs return.

```incan
# These are equivalent:
counts: Dict[str, int] = {}
counts: HashMap[str, int] = HashMap.new()
```

## Method naming conventions you may see

| Python habit | Incan | Generated Rust you may read | Notes |
| --- | --- | --- | --- |
| `dict.get(key)` | `d.get(key)` | `map.get(&key)` | Returns `Option` holding the stored value |
| `dict[key]` | `d[key]` | `map[&key]` | Panics if missing |
| `dict.get(key, default)` | `d.get(key).unwrap_or(default)` | `map.get(&key).copied().unwrap_or(default)` | The stored value or the default |
| `str(x)` | `str(x)` | `x.to_string()` | Convert to string |
| `len(x)` | `len(x)` | `x.len()` | Length |

## Option and Result: unwrap-like patterns

Rust APIs often return `Option`/`Result` instead of raising exceptions.

`unwrap()` is a “this must exist” assertion (it panics if missing), so prefer `unwrap_or(...)` or `match` when the value can be absent.

```incan
value = my_dict.get("key").unwrap_or(0)

match my_dict.get("key"):
    case Some(v): println(v)
    case None: println("missing")
```

## `str` and `bytes`

Method names on `str` are the familiar ones — `upper`, `lower`, `strip`, `split`, `replace`, `join` — but Incan is statically typed and substring membership is a method, `s.contains("x")`, not Python's `in` operator. Incan hides the `String` / `&str` split: write `str` and the compiler decides ownership in the generated Rust.

Python's `bytes` is immutable; Incan's `bytes` lowers to `Vec<u8>`, which is not. Choose the type by what the data is, not by where it came from:

| Use case | Type |
| --- | --- |
| Text, user-facing content | `str` |
| File contents that are text | `str` |
| Binary files, network protocols, cryptographic inputs, raw file I/O | `bytes` |

`str.encode()` and `bytes.decode()` convert between them, UTF-8 only; where Python's `decode` raises `UnicodeDecodeError`, Incan's returns `Result[str, ValidationError]`, so a malformed input is an `Err` you match on or pass up with `?`.

## See also

- [Rust interop (how-to)](rust_interop.md)
- [Error handling (concepts)](../explanation/error_handling.md)
- [Strings and bytes (Reference)](../reference/strings.md)
