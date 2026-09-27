# Derives: Custom behavior (reference)

This page specifies the dunder methods that define behavior a derive would otherwise provide. The derive catalog is in [Derives and traits](../derives_and_traits.md); the dunders of the arithmetic, bitwise and indexing operators are in [Operator traits](../stdlib_traits/operators.md).

## Dunders

| Dunder | Signature | Defines | Matching derive |
| --- | --- | --- | --- |
| `__str__` | `(self) -> str` | `{value}`, `str(value)`, `print(value)` | `Display` |
| `__eq__` | `(self, other: Self) -> bool` | `==` | `Eq` |
| `__ne__` | `(self, other: Self) -> bool` | `!=` | `Eq` |
| `__lt__` | `(self, other: Self) -> bool` | `<` | `Ord` |
| `__le__` | `(self, other: Self) -> bool` | `<=` | `Ord` |
| `__gt__` | `(self, other: Self) -> bool` | `>` | `Ord` |
| `__ge__` | `(self, other: Self) -> bool` | `>=` | `Ord` |

- Each dunder defines only its own operator. Adopting `std.derives.comparison.Eq` supplies `__ne__` from `__eq__`, and adopting `std.derives.comparison.Ord` supplies `__le__`, `__gt__` and `__ge__` from `__lt__` and `__eq__` (see [Comparison](comparison.md)).
- A type does not both define `__str__` and derive `Display`, define `__eq__` and derive `Eq`, or define `__lt__` and derive `Ord`; such a type is refused (`INCAN-T0001`).
- `Debug`, `Clone`, `Copy`, `Default` and `Hash` have no dunder. A method named `__hash__` is an ordinary method: it does not provide `Hash`, and a set or dict does not call it.

## See also

- [Customize derived behavior (how-to)](../../how-to/customize_derived_behavior.md)
- [Reflection](../reflection.md), for `__fields__()` and `__class_name__()`
