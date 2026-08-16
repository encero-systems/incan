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

| Python habit              | Rust / interop pattern             | Notes              |
| ------------------------- | ---------------------------------- | ------------------ |
| `dict.get(key)`           | `map.get(&key)`                    | Returns `Option`   |
| `dict[key]`               | `map[&key]`                        | Panics if missing  |
| `dict.get(key, default)`  | `map.get(&key).copied().unwrap_or(default)` | Copy a scalar value or use the default |
| `str(x)`                  | `x.to_string()`                    | Convert to string  |
| `len(x)`                  | `x.len()`                          | Length             |

## Option and Result: unwrap-like patterns

Rust APIs often return `Option`/`Result` instead of raising exceptions.

`unwrap()` is a “this must exist” assertion (it panics if missing), so prefer `unwrap_or(...)` or `match` when the value can be absent.

```incan
value = my_dict.get("key").copied().unwrap_or(0)

match my_dict.get("key"):
    case Some(v): println(v)
    case None: println("missing")
```

## See also

- [Rust interop (how-to)](rust_interop.md)
- [Error handling (concepts)](../explanation/error_handling.md)
