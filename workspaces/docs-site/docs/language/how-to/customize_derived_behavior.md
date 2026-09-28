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

## Custom equality (`==` and `!=`)

### Goal (custom equality)

Compare values by a subset of fields (for example, compare by `id` only).

### Steps (custom equality)

1. Adopt `Eq` from `std.derives.comparison` and define `__eq__(self, other: Self) -> bool`. Adopting `Eq` gives you `!=` as the negation of `__eq__`; without it, `!=` needs its own `__ne__`.
2. Do not also `@derive(Eq)`.

```incan
from std.derives.comparison import Eq

model User with Eq:
    id: int
    name: str

    def __eq__(self, other: User) -> bool:
        return self.id == other.id
```

If such a type also derives `Hash` to be a set element or dict key, keep `__eq__` consistent with the hash: two values that `__eq__` calls equal must hash alike. A derived hash covers every field, so pair it only with an `__eq__` that compares every field. For equality by a subset of fields, key the collection by that subset instead (see [Key a set or dict by a custom identity](#key-a-set-or-dict-by-a-custom-identity)).

---

## Custom ordering (`<`, `<=`, `>`, `>=`)

### Goal (custom ordering)

Compare values using a domain-specific rule.

### Steps (custom ordering)

1. Adopt `Ord` from `std.derives.comparison` and define `__eq__` and `__lt__(self, other: Self) -> bool`. Adopting `Ord` gives you `<=`, `>` and `>=` from those two; without it, each operator needs its own dunder.
2. Do not also `@derive(Ord)`.

```incan
from std.derives.comparison import Ord

model Task with Ord:
    priority: int
    name: str

    def __eq__(self, other: Task) -> bool:
        return self.priority == other.priority and self.name == other.name

    def __lt__(self, other: Task) -> bool:
        if self.priority != other.priority:
            return self.priority < other.priority
        return self.name < other.name
```

Comparing a second field when the first ties keeps `<` consistent with `__eq__`: two tasks that are not equal are always ordered one way or the other.

See [Derives: Comparison → Ord](../reference/derives/comparison.md#ord) for the ordering rules.

---

## Key a set or dict by a custom identity

### Goal (custom identity keys)

Use values as set elements or dict keys by one part of them (for example, by `id` only).

### Steps (custom identity keys)

1. Key the collection by the field that carries the identity: a `dict[int, User]` keyed by `user.id`, or a `set[int]` of ids.
2. When every field is part of the identity, add `@derive(Eq, Hash)` to the type and use it as the key directly.
3. Do not rely on a `__hash__` method: a set or dict does not call it. A set element or dict key implements `Eq` and `Hash`, so a type that defines `__eq__` is a key only when it also adopts `Eq` (`model User with Eq`) and derives `Hash`, and is refused otherwise (`INCAN-T0114`). A derived `Hash` covers every field, so when `__eq__` compares only part of the value, key the collection by that part as in step 1.

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

See [Derives: Comparison → Hash](../reference/derives/comparison.md#set-elements-and-dict-keys) for which types can be set elements and dict keys.

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
