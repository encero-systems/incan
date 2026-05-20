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

## Custom hashing (`Set` / `Dict` keys)

### Goal (custom hashing)

Use a type as a `Set` member / `Dict` key based on a custom identity.

### Steps (custom hashing)

1. Define `__hash__(self) -> int`.
2. Ensure it matches equality: if `a == b`, their hashes must match.
3. Do not also `@derive(Hash)`.

```incan
model User:
    id: int
    name: str

    def __eq__(self, other: User) -> bool:
        return self.id == other.id

    def __hash__(self) -> int:
        return self.id.__hash__()
```

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
