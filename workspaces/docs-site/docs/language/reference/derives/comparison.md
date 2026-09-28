# Derives: Comparison (reference)

This page specifies `Eq`, `PartialEq`, `Ord`, `PartialOrd` and `Hash`, and the types a `set` element or `dict` key may have. The derive catalog and the automatic derives are in [Derives and traits](../derives_and_traits.md).

A comparison operator is available on a type through a derive below, an adopted trait, or the operator's own dunder. An operator the type does not provide is refused (`INCAN-T0001`), and so is a derive on a type that does not meet its **Requires** entry.

## Eq

- **Provides**: `==` and `!=`. A type that implements `Eq` and `Hash` can be a `set` element or `dict` key.
- **Provided by**: `@derive(Eq)` or `@derive(Ord)`; `Eq` or `Ord` in `@rust.derive(...)`; adopting `std.derives.comparison.Eq` and defining `__eq__`; adopting `std.derives.comparison.Ord` and defining `__eq__` and `__lt__`.
- **Behavior**: two values are equal when every field is equal.
- **Dunder**: `__eq__(self, other: Self) -> bool` defines `==`, and `__ne__(self, other: Self) -> bool` defines `!=`. Adopting `Eq` supplies `__ne__` as `not self.__eq__(other)`.
- **Requires**: every field type implements `Eq`. A type does not both define `__eq__` and derive `Eq` (`INCAN-T0001`).

## PartialEq

- **Provides**: `==` and `!=`.
- **Provided by**: `@derive(PartialEq)`; `PartialEq` in `@rust.derive(...)`; `Eq` and `Ord`, which imply it; a `__eq__` method; every `enum` whose payload types are numbers, `bool`, `str` or `bytes`, or a `list`, `set`, `dict`, `Option`, `Result` or tuple of those.
- **Behavior**: two values are equal when every field is equal.
- **Dunder**: as for `Eq`.
- **Requires**: every field type implements `PartialEq`.

## Ord

- **Provides**: `<`, `<=`, `>` and `>=`, `sorted(values)` of a `list` of the type, and what `Eq` provides.
- **Provided by**: `@derive(Ord)`; `Ord` in `@rust.derive(...)`; adopting `std.derives.comparison.Ord` and defining `__eq__` and `__lt__`.
- **Behavior**: values compare field by field, in declaration order.
- **Dunder**: `__lt__`, `__le__`, `__gt__` and `__ge__`, each `(self, other: Self) -> bool`, define `<`, `<=`, `>` and `>=`. Adopting `Ord` supplies `__le__`, `__gt__` and `__ge__` from `__lt__` and `__eq__`.
- **Requires**: every field type implements `Ord`. A type does not both define `__lt__` and derive `Ord` (`INCAN-T0001`).
- **Refused**: `sorted(values)` over a `list` whose element type does not implement `Ord` (`INCAN-T0001`).

## PartialOrd

- **Provides**: `<`, `<=`, `>` and `>=`.
- **Provided by**: `@derive(PartialEq, PartialOrd)`; `PartialOrd` in `@rust.derive(...)`; `Ord`, which implies it.
- **Behavior**: values compare field by field, in declaration order.
- **Dunder**: as for `Ord`.
- **Requires**: `PartialEq`, through a derive or `__eq__`; every field type implements `PartialOrd`.

## Hash

- **Provides**: with `Eq`, use as a `set` element or `dict` key.
- **Provided by**: `@derive(Hash)`; `Hash` in `@rust.derive(...)`.
- **Behavior**: hashes every field. Two values equal under a derived `Eq` have equal hashes.
- **Dunder**: none. A method named `__hash__` is an ordinary method, and a set or dict does not call it.
- **Requires**: every field type implements `Hash`.
- **Refused**: `value.__hash__()` on a value of a type parameter through its `Hash` bound (`INCAN-T0001`).

## Set elements and dict keys

- The element type `T` of a `set[T]`, and the key type `K` of a `dict[K, V]` or `HashMap[K, V]`, implement `Eq` and `Hash`. A set or dict whose element or key type does not is refused (`INCAN-T0114`), whether it is written as a type or built by a literal, a comprehension or `set(source)`.
- A generic function or method that uses a value of its type parameter as a set element or dict key requires `Eq` and `Hash` of that type parameter's type argument; a type argument without them is refused (`INCAN-T0114`). See [Operations on a type parameter](../derives_and_traits.md#operations-on-a-type-parameter).
- A `FrozenSet` element and a `FrozenDict` key carry no requirement.

| Type | `Eq` | `Hash` |
| --- | --- | --- |
| `int`, `bool`, `str`, `bytes`, `decimal`, the exact-width integers, `FrozenStr`, `FrozenBytes` | Yes | Yes |
| `float`, `f32`, `f64` | No | No |
| A tuple, `list[T]`, `Option[T]` or `Result[T, E]` | When its type arguments do | When its type arguments do |
| `set[T]`, `dict[K, V]`, `FrozenList[T]`, `FrozenSet[T]`, `FrozenDict[K, V]` | When its type arguments do | No |
| A `model`, `class`, `enum` or `newtype` | When it provides `Eq` (see [Eq](#eq)) | With `@derive(Hash)`, or `Hash` in `@rust.derive(...)` |

A generic `model`, `class`, `enum` or `newtype` implements `Eq` or `Hash` for an instantiation whose type arguments implement it too.

```incan
from std.derives.comparison import Eq

enum Tag:
    A
    B

@derive(Eq, Hash)
enum Label:
    A
    B

@derive(Hash)
model UserKey with Eq:
    id: int

    def __eq__(self, other: UserKey) -> bool:
        return self.id == other.id

def unique[T](items: list[T]) -> set[T]:
    return set(items)

def main() -> None:
    labels: set[Label] = {Label.A}        # accepted
    keys: set[UserKey] = {UserKey(id=1)}  # accepted
    tags: set[Tag] = {Tag.A}              # refused: Tag does not implement Eq and Hash (INCAN-T0114)
    seen = unique([Label.A])              # accepted
    kinds = unique([Tag.A])               # refused: unique uses T as a set element (INCAN-T0114)
```

## See also

- [Customize derived behavior (how-to)](../../how-to/customize_derived_behavior.md)
- [How derives work (explanation)](../../explanation/how_derives_work.md#how-derive-requirements-are-decided)
- [Operator traits](../stdlib_traits/operators.md#comparisons)
