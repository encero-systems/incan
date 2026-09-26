# Frozen collections (reference)

This page is the reference for reading the frozen collections a module `const` carries: `FrozenList[T]`, `FrozenSet[T]` and `FrozenDict[K, V]`.

For the mental model, see [Const bindings](../explanation/consts.md).

## Types

A `const` initialized with a list, set or dict literal has the frozen collection type of that literal:

| Initializer | Const type |
| --- | --- |
| `[a, b]` | `FrozenList[T]` |
| `{a, b}` | `FrozenSet[T]` |
| `{k: v}` | `FrozenDict[K, V]` |

A frozen collection offers reads only.

## Reads

| Expression | Receiver | Result |
| --- | --- | --- |
| `len(c)`, `c.len()` | `FrozenList[T]`, `FrozenSet[T]`, `FrozenDict[K, V]` | `int` |
| `c.is_empty()` | `FrozenList[T]`, `FrozenSet[T]`, `FrozenDict[K, V]` | `bool` |
| `d[key]` | `FrozenDict[K, V]` | `V` |
| `d.contains_key(key)` | `FrozenDict[K, V]` | `bool` |
| `[expr for item in c]` | `FrozenList[T]`, `FrozenSet[T]` | `list[U]`; `item` has type `T` |

`d[key]` and `d.contains_key(key)` compare `key` with the stored keys. When `K` is `str` or `FrozenStr`, any `str` value is a valid `key`.

An item of a `FrozenList[str]` or `FrozenSet[str]` is a `str`.

## Errors

| Condition | Result |
| --- | --- |
| `d[key]` where `key` is not a `K` | refused at check time: `INCAN-T0001`, "Index type mismatch" |
| `d[key]` where no stored key equals `key` | `KeyError` at run time |

## Examples

```incan
const TABLE: FrozenDict[str, FrozenList[str]] = {"names": ["alpha", "beta"], "empty": []}
const NAMES: FrozenList[str] = ["alpha", "beta"]
const SQUARES: FrozenDict[int, int] = {2: 4, 3: 9}


def main() -> None:
    println(len(TABLE["names"]))                    # 2
    println(TABLE.contains_key("names"))            # true
    println(TABLE.contains_key("missing"))          # false
    println(" ".join([name for name in NAMES]))     # alpha beta
    println(SQUARES[3])                             # 9
    println(SQUARES[5])                             # KeyError at run time
```

```incan
const SQUARES: FrozenDict[int, int] = {2: 4, 3: 9}


def main() -> None:
    println(SQUARES["two"])  # refused: INCAN-T0001, Index type mismatch: expected 'int', found 'str'
```

## Related pages

- [Const bindings](../explanation/consts.md)
- [Static storage (reference)](static_storage.md)
