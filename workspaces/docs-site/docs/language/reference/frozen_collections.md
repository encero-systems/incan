# Frozen collections (reference)

This page specifies the frozen collection types a `const` holds, `FrozenList[T]`, `FrozenSet[T]` and `FrozenDict[K, V]`, and the reads they answer. Refusals are reported with `INCAN-T0001`.

## Types

| Const initializer | Type |
| --- | --- |
| `[a, b]` | `FrozenList[T]` |
| `{a, b}` | `FrozenSet[T]` |
| `{k: v}` | `FrozenDict[K, V]` |

- The annotation of such a `const`, when written, names the frozen type. An annotation `list[T]`, `set[T]` or `dict[K, V]` is refused.
- A frozen type annotates a parameter, a return type or a binding. A value of the frozen type is accepted there; a `list`, `set` or `dict` value is refused, and a frozen value is refused where a `list`, `set` or `dict` is expected.
- `FrozenList(...)`, `FrozenSet(...)` and `FrozenDict(...)` are not constructors: a call is refused.
- A frozen collection is read-only. A mutating method, such as `append` or `add`, and an index assignment are refused.

## Reads

| Expression | Receiver | Result |
| --- | --- | --- |
| `len(c)`, `c.len()` | `FrozenList[T]`, `FrozenSet[T]`, `FrozenDict[K, V]` | `int` |
| `c.is_empty()` | `FrozenList[T]`, `FrozenSet[T]`, `FrozenDict[K, V]` | `bool` |
| `items[i]` | `FrozenList[T]` | `T`; `i` is an `int`, counted from the end when negative; an index out of range raises `IndexError` |
| `d[key]` | `FrozenDict[K, V]` | `V`; a key no entry holds raises `KeyError` |
| `d.contains_key(key)` | `FrozenDict[K, V]` | `bool` |
| `s.contains(x)` | `FrozenSet[T]` | `bool` |
| `x in c`, `x not in c` | `FrozenList[T]`, `FrozenSet[T]` | `bool`: whether an element equals `x` |
| `key in d`, `key not in d` | `FrozenDict[K, V]` | `bool`: whether an entry holds `key` |
| `for item in c:` | `FrozenList[T]`, `FrozenSet[T]` | `item` is each element, a `T` |
| `for key in d:` | `FrozenDict[K, V]` | `key` is each key, a `K` |
| `list(c)`, `set(c)` | `FrozenList[T]`, `FrozenSet[T]` | `list[T]`, `set[T]` |
| `[expr for item in c]` | `FrozenList[T]`, `FrozenSet[T]`, `FrozenDict[K, V]` | `list[U]`, where `item` is a `T` (a key `K` of a `FrozenDict`) and `expr` is a `U` |
| `sorted(c)` | `FrozenList[T]`, with `T` a `float` or an ordered type (see [Ord](derives/comparison.md#ord)) | `list[T]`, the elements in ascending order |
| `min(c)`, `max(c)` | `FrozenList[T]`, with `T` one of `int`, `float`, `bool`, `str`, `FrozenStr` | `T`: the least or greatest element |
| `enumerate(c)` | `FrozenList[T]` | `list[tuple[int, T]]`: each element with its position, from `0` |
| `print(c)`, `str(c)`, `f"{c}"` | `FrozenList[T]`, `FrozenSet[T]`, `FrozenDict[K, V]` | The elements' structure, as a `list`, `set` or `dict` displays (see [Display](strings.md#display)) |

- `key` is a `K`, and `x` is a `T`. When `K` or `T` is `str` or `FrozenStr`, `key` or `x` may be any `str` or `FrozenStr` value.
- A `str` or `bytes` item, key or value that a read produces is a `str` or `bytes`.

Refused: `items[i]` with an `i` that is not an `int`; `d[key]`, `d.contains_key(key)` and `key in d` with a `key` that is not a `K`; `s.contains(x)` and `x in c` with an `x` that is not a `T`; `sorted`, `min` and `max` of a `FrozenSet` or a `FrozenDict`.

## Examples

```incan
const TABLE: FrozenDict[str, FrozenList[str]] = {"names": ["alpha", "beta"], "empty": []}
const NAMES: FrozenList[str] = ["alpha", "beta"]
const SQUARES: FrozenDict[int, int] = {2: 4, 3: 9}


def count(names: FrozenList[str]) -> int:
    return len(names)                                  # accepted


def main() -> None:
    names: FrozenList[str] = TABLE["names"]            # accepted
    found: bool = TABLE.contains_key("names")          # accepted
    joined: str = " ".join([name for name in NAMES])   # accepted
    first: str = NAMES[0]                              # accepted
    listed: bool = "alpha" in NAMES                    # accepted
    for key in SQUARES:                                # accepted
        println(key)
    nine: int = SQUARES[3]                             # accepted
    has_five: bool = 5 in SQUARES                      # accepted
    ordered: list[str] = sorted(NAMES)                 # accepted
    size: int = count(NAMES)                           # accepted
    println(NAMES)                                     # accepted
    two: int = SQUARES["two"]                          # refused: the key type is int
    NAMES.append("gamma")                              # refused: a frozen collection is read-only
    copy = FrozenList(["alpha"])                       # refused: a frozen collection has no constructor
```

## Related pages

- [Const bindings](../explanation/consts.md)
- [Static storage (reference)](static_storage.md)
