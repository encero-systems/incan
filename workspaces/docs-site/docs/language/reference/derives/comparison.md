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

- **Enables**: use as `Set` members and `Dict` keys
- **Default behavior**: structural, field-based hashing
- **Custom behavior**: define `__hash__(self) -> int`
- **Conflict rule**: if you define `__hash__`, do not also `@derive(Hash)`
- **Requirement**: a `set` element type and a `dict` key type implement `Eq` and `Hash`; otherwise the program is refused with `INCAN-T0114`, and the diagnostic names the missing derives.

Which types implement `Eq` and `Hash`:

| Type | `Eq` | `Hash` |
| --- | --- | --- |
| `int`, `bool`, `str`, `bytes`, integer numerics, `decimal`, `FrozenStr`, `FrozenBytes` | yes | yes |
| `float`, `f32`, `f64` | no | no |
| tuple, `list`, `Option`, `Result` | when every element type does | when every element type does |
| `set`, `dict`, `FrozenList`, `FrozenSet`, `FrozenDict` | when every element type does | no |
| `model`, `class`, `enum`, `newtype` | with `@derive(Eq)`, `@derive(Ord)`, or `Eq` in `@rust.derive(...)` | with `@derive(Hash)`, or `Hash` in `@rust.derive(...)` |

`__eq__` and `__hash__` provide neither. A type declared in another module is refused only when its declaration is known to lack the derive; a type parameter, a Rust-origin type and a `rusttype` are not refused.

Where the requirement applies:

| Position | Refused at |
| --- | --- |
| `set[T]`, `dict[K, V]`, `HashMap[K, V]` in any annotation | the element or key type |
| set literal `{a, b}`, dict literal `{k: v}` | the first element or key, unless an annotation already refused its type |
| dict comprehension `{k: v for ...}` | the key expression |
| `set(source)` | the argument |
| a call to a generic function declared in the same module whose body hashes its type parameter as above | the call, naming the type parameter |

`FrozenSet` elements and `FrozenDict` keys carry no requirement.

Consistency rule:

- If `a == b`, then `a.__hash__() == b.__hash__()`.

```incan
enum Tag:
    A
    B

@derive(Eq, Hash)
enum Label:
    A
    B

def main() -> None:
    labels: set[Label] = {Label.A}  # accepted
    tags: set[Tag] = {Tag.A}        # refused: INCAN-T0114, 'Tag' cannot be a set element: it does not implement Eq and Hash
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


