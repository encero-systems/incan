# Indexing and slicing (Reference)

This page documents stdlib traits for `obj[key]`, `obj[key] = value`, and slicing.

`Index` and `IndexMut` are the canonical trait names for indexed read and write capabilities. The `GetItem` and `SetItem` names in `std.traits.ops` are compatibility/operator aliases for the same hooks.

## Index (read)

- **Syntax**: `obj[key]`
- **Hook**: `__getitem__(self, key: K) -> V`
- **Trait**: `Index[K, V]`

The same type may adopt `Index[K, V]` more than once when the key or value type arguments differ. This is how a custom type can support more than one statically checked lookup shape without runtime dispatch:

```incan
from std.traits.indexing import Index

model Table with Index[str, str], Index[int, str]:
    def __getitem__(self, key: str) -> str:
        return key

    def __getitem__(self, key: int) -> str:
        return str(key)

table = Table()
column = table["name"]  # Index[str, str]
first = table[0]        # Index[int, str]
```

When explicit adoption uses multiple `Index` instantiations, the key expression type identifies the intended indexed access shape. This is not general method overloading: same-name hooks from unrelated trait families are rejected unless a future language feature adds explicit qualification or aliasing.

## IndexMut (write)

- **Syntax**: `obj[key] = value`
- **Hook**: `__setitem__(self, key: K, value: V) -> None`
- **Trait**: `IndexMut[K, V]`

## Slicing

- **Syntax**: `obj[start:end:step]`

Incan’s long-term direction is slice-aware `__getitem__` (Python-style). The current stdlib vocabulary includes `Sliceable[T]` and `__getslice__`, which will be aligned with `__getitem__` as the feature is finalized.
