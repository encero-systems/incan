# Frozen collections (reference)

This page specifies the frozen collection types a `const` holds, `FrozenList[T]`, `FrozenSet[T]` and `FrozenDict[K, V]`, and the reads they answer. Refusals are reported with `INCAN-T0001`.

For the mental model, see [Const bindings](../explanation/consts.md).

## Types

| Const initializer | Type |
| --- | --- |
| `[a, b]` | `FrozenList[T]` |
| `{a, b}` | `FrozenSet[T]` |
| `{k: v}` | `FrozenDict[K, V]` |

- The annotation of such a `const`, when written, names the frozen type. An annotation `list[T]`, `set[T]` or `dict[K, V]` is refused.
- A frozen collection is read-only. A mutating method, such as `append` or `add`, and an index assignment are refused.

## Reads

| Expression | Receiver | Result |
| --- | --- | --- |
| `len(c)`, `c.len()` | `FrozenList[T]`, `FrozenSet[T]`, `FrozenDict[K, V]` | `int` |
| `c.is_empty()` | `FrozenList[T]`, `FrozenSet[T]`, `FrozenDict[K, V]` | `bool` |
| `d[key]` | `FrozenDict[K, V]` | `V`; a key no entry holds raises `KeyError` |
| `d.contains_key(key)` | `FrozenDict[K, V]` | `bool` |
| `s.contains(x)` | `FrozenSet[T]` | `bool` |
| `list(c)` | `FrozenList[T]`, `FrozenSet[T]` | `list[T]` |
| `[expr for item in c]` | `FrozenList[T]`, `FrozenSet[T]` | `list[U]`, where `item` is a `T` and `expr` is a `U` |

- `key` is a `K`, and `x` is a `T`. When `K` is `str` or `FrozenStr`, `key` may be any `str` or `FrozenStr` value.
- A `str` item or value that a read produces is a `str`.

Refused: `d[key]` and `d.contains_key(key)` with a `key` that is not a `K`.

## Examples

```incan
const TABLE: FrozenDict[str, FrozenList[str]] = {"names": ["alpha", "beta"], "empty": []}
const NAMES: FrozenList[str] = ["alpha", "beta"]
const SQUARES: FrozenDict[int, int] = {2: 4, 3: 9}


def main() -> None:
    names: FrozenList[str] = TABLE["names"]            # accepted
    found: bool = TABLE.contains_key("names")          # accepted
    joined: str = " ".join([name for name in NAMES])   # accepted
    nine: int = SQUARES[3]                             # accepted
    missing: int = SQUARES[5]                          # accepted
    two: int = SQUARES["two"]                          # refused: the key type is int
    NAMES.append("gamma")                              # refused: a frozen collection is read-only
```

## Related pages

- [Const bindings](../explanation/consts.md)
- [Static storage (reference)](static_storage.md)
