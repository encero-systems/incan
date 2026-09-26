# Derives: Comparison (Reference)

This page documents `Eq`, `Ord`, and `Hash`.

See also:

- [Derives & traits](../derives_and_traits.md)
- [Customize derived behavior (How-to)](../../how-to/customize_derived_behavior.md)

---

## Eq (equality)

- **Enables**: `==`, `!=`
- **Default behavior**: structural, field-based equality
- **Custom behavior**: define `__eq__(self, other: Self) -> bool`
- **Conflict rule**: if you define `__eq__`, do not also `@derive(Eq)`

---

## Ord (ordering)

- **Enables**: `<`, `<=`, `>`, `>=`, `sorted(...)`
- **Default behavior**: structural ordering by field order
- **Custom behavior**: define `__lt__(self, other: Self) -> bool`
- **Conflict rule**: if you define `__lt__`, do not also `@derive(Ord)`

---

## Hash

- **Enables**: use as `set` elements and `dict` keys
- **Behavior**: structural, field-based hashing
- **Provided by**: `@derive(Hash)`, or `Hash` in `@rust.derive(...)`
- **Requirement**: a `set` element type and a `dict` key type implement `Eq` and `Hash`; otherwise the program is refused with `INCAN-T0114`, and the diagnostic names the missing derives.

Which types implement `Eq` and `Hash`:

| Type | `Eq` | `Hash` |
| --- | --- | --- |
| `int`, `bool`, `str`, `bytes`, integer numerics, `decimal`, `FrozenStr`, `FrozenBytes` | yes | yes |
| `float`, `f32`, `f64` | no | no |
| tuple, `list`, `Option`, `Result` | when every element type does | when every element type does |
| `set`, `dict`, `FrozenList`, `FrozenSet`, `FrozenDict` | when every element type does | no |
| `model`, `class`, `enum`, `newtype` | with `@derive(Eq)`, `@derive(Ord)`, or `Eq` in `@rust.derive(...)` | with `@derive(Hash)`, or `Hash` in `@rust.derive(...)` |

Defining `__eq__` provides neither `Eq` nor `Hash`. A method named `__hash__` is an ordinary method: it provides neither, and a set or dict does not call it. A type declared in another module is refused only when its declaration is known to lack the derive; a type parameter, a Rust-origin type and a `rusttype` are not refused.

Where the requirement applies:

| Position | Refused at |
| --- | --- |
| `set[T]`, `dict[K, V]`, `HashMap[K, V]` in any annotation | the element or key type |
| set literal `{a, b}`, dict literal `{k: v}` | the first element or key, unless an annotation already refused its type |
| dict comprehension `{k: v for ...}` | the key expression |
| `set(source)` | the argument |
| a call to a generic function or method whose body hashes its type parameter, declared in the same module, an imported source module or a compiled library | the call, naming the type parameter |

A generic body hashes a type parameter when it uses a value of that type as a set literal element, a dict literal key or a dict comprehension key, calls `set(...)` on a collection of it, or passes it to a function or method that hashes it. The requirement belongs to the function's signature: a compiled library records it as inferred `Eq` and `Hash` bounds on the type parameter, which a call refuses under the same rule, only for a type known to lack the derive. The type argument checked is what the call instantiates the parameter with, including the element type of a literal argument (`unique([Tag.A])` instantiates `T` as `Tag`). A type parameter declared with `Eq` and `Hash` bounds is checked by those bounds instead. A call of a function or method of the same module or an imported source module that passes on the caller's own type parameter is not refused: the caller then hashes that parameter, and its own calls are checked. A call of a compiled library's function or method that passes on the caller's own type parameter is refused with `INCAN-T0114` unless that parameter is declared with `Eq` and `Hash` bounds (`def mine[T with (Eq, Hash)](items: list[T]) -> set[T]`).

`FrozenSet` elements and `FrozenDict` keys carry no requirement.

Derived `Eq` and `Hash` compare and hash the same fields, so equal values hash alike.

```incan
enum Tag:
    A
    B

@derive(Eq, Hash)
enum Label:
    A
    B

def unique[T](items: list[T]) -> set[T]:
    return set(items)

def main() -> None:
    labels: set[Label] = {Label.A}  # accepted
    tags: set[Tag] = {Tag.A}        # refused: INCAN-T0114, 'Tag' cannot be a set element: it does not implement Eq and Hash
    seen = unique([Label.A])        # accepted
    kinds = unique([Tag.A])         # refused: INCAN-T0114, 'unique' uses its type parameter 'T' as a set element or dict key, so 'Tag' cannot be its type argument
```

---

## Tie-breakers (ordering)

If you implement `__lt__`, avoid “accidental ties” by adding a deterministic tie-breaker:

```incan
model Task:
    priority: int
    name: str

    def __lt__(self, other: Task) -> bool:
        if self.priority != other.priority:
            return self.priority < other.priority
        return self.name < other.name
```


