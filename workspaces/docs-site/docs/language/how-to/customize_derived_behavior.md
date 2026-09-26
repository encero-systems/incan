# Customize derived behavior (How-to)

This page is task-focused: how to get the behavior you want when the default derive behavior doesn’t fit.

For the authoritative catalog of derives/dunders, see:

- [Derives & traits (Reference)](../reference/derives_and_traits.md)

---

## Custom string output for your type

### Goal (custom string output)

Make `println(f"{x}")` show a friendly string.

### Steps (custom string output)

1. Define `__str__(self) -> str` on your `model`/`class`.
2. Do not also `@derive(Display)` (that’s a conflict).

```incan
model User:
    name: str
    email: str

    def __str__(self) -> str:
        return f"{self.name} <{self.email}>"
```

---

## Custom equality (`==`)

### Goal (custom equality)

Compare values by a subset of fields (for example, compare by `id` only).

### Steps (custom equality)

1. Define `__eq__(self, other: Self) -> bool`.
2. Do not also `@derive(Eq)`.

```incan
model User:
    id: int
    name: str

    def __eq__(self, other: User) -> bool:
        return self.id == other.id
```

---

## Custom ordering (`sorted(...)`)

### Goal (custom ordering)

Sort values using a domain-specific rule.

### Steps (custom ordering)

1. Define `__lt__(self, other: Self) -> bool`.
2. Do not also `@derive(Ord)`.

```incan
model Task:
    priority: int
    title: str

    def __lt__(self, other: Task) -> bool:
        return self.priority < other.priority
```

!!! note "Why `__lt__`?"
    `sorted(...)` needs an ordering rule. Incan uses the `<` hook (`__lt__`) as the basis for ordering, so implementing `__lt__` gives the runtime a way to compare two values and sort a list.

    See [Derives: Comparison → Ord](../reference/derives/comparison.md#ord-ordering) for the canonical ordering rules.

---

## Key a set or dict by a custom identity

### Goal (custom identity keys)

Use values as set elements or dict keys by one part of them (for example, by `id` only).

### Steps (custom identity keys)

1. Key the collection by the field that carries the identity: a `dict[int, User]` keyed by `user.id`, or a `set[int]` of ids.
2. When every field is part of the identity, add `@derive(Eq, Hash)` to the type and use it as the key directly.
3. Do not rely on a `__hash__` method: a set or dict does not call it, and a type that defines `__eq__` is refused as a set element or dict key (`INCAN-T0114`). A custom hash is tracked in [#1822](https://github.com/encero-systems/incan/issues/1822).

```incan
model User:
    id: int
    name: str

    def __eq__(self, other: User) -> bool:
        return self.id == other.id

def index_by_id(users: list[User]) -> dict[int, User]:
    by_id: dict[int, User] = {}
    for user in users:
        by_id[user.id] = user
    return by_id
```

See [Derives: Comparison → Hash](../reference/derives/comparison.md#hash) for which types can be set elements and dict keys.

---

## Field defaults: make construction ergonomic

### Goal (field defaults)

Make `Type()` work when fields have defaults.

### Steps (field defaults)

1. Put defaults on the fields (`field: T = expr`).
2. Any field with no default must still be provided.

```incan
model Settings:
    theme: str = "dark"
    font_size: int = 14
```
