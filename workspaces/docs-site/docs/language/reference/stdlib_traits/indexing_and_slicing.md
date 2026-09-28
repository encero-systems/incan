# Indexing and slicing (Reference)

This page specifies the traits of `std.traits.indexing` for `obj[key]`, `obj[key] = value` and slicing, and their aliases in `std.traits.ops`.

| Trait | Module | Hook | Syntax |
| --- | --- | --- | --- |
| `Index[K, V]` | `std.traits.indexing` | `__getitem__(self, key: K) -> V` | `obj[key]` |
| `IndexMut[K, V]` | `std.traits.indexing` | `__setitem__(self, key: K, value: V) -> None` | `obj[key] = value` |
| `Sliceable[T]` | `std.traits.indexing` | `__getslice__(self, start: Option[int], end: Option[int], step: Option[int]) -> list[T]` | `obj[start:end:step]` |
| `GetItem[Key, Output]` | `std.traits.ops` | `__getitem__(self, key: Key) -> Output` | `obj[key]` |
| `SetItem[Key, Value]` | `std.traits.ops` | `__setitem__(self, key: Key, value: Value) -> None` | `obj[key] = value` |

`GetItem` and `SetItem` name the same hooks as `Index` and `IndexMut`.

## Index (read)

- **Syntax**: `obj[key]`
- **Hook**: `__getitem__(self, key: K) -> V`
- **Trait**: `Index[K, V]`

A type may adopt `Index[K, V]` more than once with different type arguments, defining one `__getitem__` per adoption. `obj[key]` calls the `__getitem__` whose key type is the type of `key`.

Refused (`INCAN-T0001`): a type that adopts two unrelated traits that each require a method of the same name, such as `__getitem__` from `Index` and from another trait family.

```incan
from std.traits.indexing import Index

model Table with Index[str, str], Index[int, str]:
    label: str

    def __getitem__(self, key: str) -> str:
        return key

    def __getitem__(self, key: int) -> str:
        return str(key)

def main() -> None:
    table = Table(label="t")
    column = table["name"]  # accepted: Index[str, str]
    first = table[0]        # accepted: Index[int, str]
```

## IndexMut (write)

- **Syntax**: `obj[key] = value`
- **Hook**: `__setitem__(self, key: K, value: V) -> None`
- **Trait**: `IndexMut[K, V]`

## Slicing

- **Syntax**: `obj[start:end:step]`; each part is optional, and a slice that omits its end may write its two colons together (`obj[::step]`, `obj[start::step]`).
- **Hook**: `__getslice__(self, start: Option[int], end: Option[int], step: Option[int]) -> list[T]`
- **Trait**: `Sliceable[T]`; a type that adopts it defines the hook.

On a `str` or a `list[T]`, the forms and their results are specified in [Strings: Indexing and slicing](../strings.md#indexing-and-slicing). On a type that defines `__getslice__`, `obj[start:end:step]` calls it with `Some(part)` for each part written and `None` for each part omitted, and has the hook's return type.

Refused (`INCAN-T0001`): slice syntax on a model, class, enum or newtype that defines no `__getslice__`, and a part that is not an `int`.

```incan
from std.traits.indexing import Sliceable

model Window with Sliceable[int]:
    items: list[int]

    def __getslice__(self, start: Option[int], end: Option[int], step: Option[int]) -> list[int]:
        return self.items[start.unwrap_or(0):]

def main() -> None:
    w = Window(items=[1, 2, 3])
    tail = w[1:]        # accepted: w.__getslice__(Some(1), None, None)
    every = w[::2]      # accepted: w.__getslice__(None, None, Some(2))
```
