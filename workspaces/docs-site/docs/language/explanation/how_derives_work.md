# How derives work (Explanation)

This page explains what Incan “derives” mean at compile time, without requiring you to read compiler source.

For the derive catalog and exact user-facing rules, see:

- [Derives & traits (Reference)](../reference/derives_and_traits.md)

---

## The intent

Incan’s derive system exists to give you **out-of-the-box behavior** (Debug, Display, Eq, Ord, Hash, Clone, Default,
serde derives, etc.) without boilerplate.

At a high level:

- you write `@derive(...)` on a `model`/`class`/`enum`/`newtype`
- the compiler generates the corresponding implementations in generated Rust (or compiler-injected impls)

---

## Where the “derive vocabulary” lives

Incan’s stdlib contains trait definitions that act as **documentation vocabulary** for derive names and dunder hooks.

You may see `@rust.extern` in stdlib sources:

- it marks functions whose body is provided by Rust (via `rust.module()`)
- it marks “compiler-provided implementation” stubs in the stdlib

The compiler is responsible for providing the implementation; the stdlib is the stable vocabulary and signature
registry.

---

## Derives vs dunders

Incan separates two cases:

- **Derives**: default, structural behavior (field-based)
- **Dunder hooks**: custom behavior (`__str__`, `__eq__`, `__lt__`, `__hash__`)

If you try to do *both* for the same capability, that’s a **conflict** and should be treated as an error: the compiler
must not have to guess which implementation “wins”.

Example conflicts:

```incan
@derive(Eq)
model User:
    id: int

    def __eq__(self, other: User) -> bool:
        return self.id == other.id
```

The authoritative rule set (including the full conflict list) lives in:

- [Derives & traits (Reference)](../reference/derives_and_traits.md)

---

## Non-goals (deliberate omissions)

Some Python/Rust features are intentionally not part of Incan’s trait/derive surface area:

- **Context managers** (`__enter__` / `__exit__`): prefer scope-based cleanup RAII (Resource Acquisition Is  Initialization)
  style
- **Destructors** (`__del__` / `Drop` as a user feature): cleanup is automatic; exposing destruction hooks adds complexity
- **`__format__`**: `Display` (`__str__`) + f-strings cover most needs

### Resource management without context managers

Incan’s default approach is: allocate the resource, use it, and let scope end handle cleanup.

```incan
def process_file() -> Result[str, str]:
    file = File.open("data.txt")?
    content = file.read_all()?
    return Ok(content)
```
