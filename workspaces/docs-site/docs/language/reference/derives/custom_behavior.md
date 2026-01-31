# Derives: Custom behavior (Reference)

This page lists the dunder hooks used to customize derived behavior.

See also:

- [Derives & traits](../derives_and_traits.md)
- [Customize derived behavior (How-to)](../../how-to/customize_derived_behavior.md)
- [Stdlib traits](../stdlib_traits/index.md)
- [Reflection](../reflection.md) (for `__fields__()` / `__class_name__()`)

---

## Dunder hooks

| Hook       | Purpose                        |
| ---------- | ------------------------------ |
| `__str__`  | Display formatting (`{value}`) |
| `__eq__`   | Equality (`==`, `!=`)          |
| `__lt__`   | Ordering (`<`, sorting)        |
| `__hash__` | Hashing (`Set` / `Dict` keys)  |

Rule:

- You must not combine a hook with the corresponding `@derive(...)` (conflict).
