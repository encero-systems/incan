# Derives: Custom behavior (Reference)

This page lists the dunder hooks used to customize derived behavior.

See also:

- [Derives & traits](../derives_and_traits.md)
- [Customize derived behavior (How-to)](../../how-to/customize_derived_behavior.md)
- [Stdlib traits](../stdlib_traits/index.md)
- [Reflection](../reflection.md) (for `__fields__()` / `__class_name__()`)

---

## Dunder hooks

| Hook      | Purpose                        |
| --------- | ------------------------------ |
| `__str__` | Display formatting (`{value}`) |
| `__eq__`  | Equality (`==`, `!=`)          |
| `__lt__`  | Ordering (`<`, sorting)        |

Hashing has no hook: a method named `__hash__` is an ordinary method, and `Hash` comes only from `@derive(Hash)` or `Hash` in `@rust.derive(...)` ([Comparison → Hash](comparison.md#hash)).

Rule:

- You must not combine a hook with the corresponding `@derive(...)` (conflict).
