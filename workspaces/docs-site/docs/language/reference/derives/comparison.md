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
- **Requirement**: a `set` element type and a `dict` key type implement `Eq` and `Hash`. A `model`, `class`, `enum` or `newtype` implements `Eq` through `@derive(Eq)` or `@derive(Ord)` and `Hash` through `@derive(Hash)`. One that lacks either, as the element or key type or inside a tuple, `list`, `Option` or `Result` in that position, is refused with `INCAN-T0001` where it is written: in an annotation, or as the first element of a set literal or the first key of a dict literal. The diagnostic names the missing derives. `FrozenSet` elements and `FrozenDict` keys carry no such requirement.

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
    tags: set[Tag] = {Tag.A}        # refused: INCAN-T0001, 'Tag' cannot be a set element: it does not derive Eq and Hash
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


