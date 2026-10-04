# How derives work (Explanation)

This page explains what Incan “derives” mean at compile time, without requiring you to read compiler source.

For the derive catalog and exact user-facing rules, see:

- [Derives & traits (Reference)](../reference/derives_and_traits.md)

---

## The intent

Incan’s derive system exists to give you **out-of-the-box behavior** (Debug, Display, Eq, Ord, Hash, Clone, Default, serde derives, etc.) without boilerplate.

At a high level:

- you write `@derive(...)` on a `model`/`class`/`enum`/`newtype`
- the compiler generates the corresponding implementations in generated Rust (or compiler-injected impls)

---

## Where the “derive vocabulary” lives

Incan’s stdlib contains trait definitions that act as **documentation vocabulary** for derive names and dunder hooks.

You may see `@rust.extern` in stdlib sources:

- it marks functions whose body is provided by Rust (via `rust.module()`)
- it marks “compiler-provided implementation” stubs in the stdlib

The compiler is responsible for providing the implementation; the stdlib is the stable vocabulary and signature registry.

A module derive follows the same pattern. `@derive(json)` adopts each trait the `std.serde.json` module lists in its `__derives__`, `json.Serialize` and `json.Deserialize`, and those traits carry the Rust derives (`serde::Serialize`, `serde::Deserialize`) the compiled type needs. The adopted traits then work like any `with` adoption: `to_json()` is found on the type, and the type satisfies a `T with json.Serialize` bound.

---

## Derives vs dunders

Incan separates two cases:

- **Derives**: default, structural behavior (field-based)
- **Dunder hooks**: custom behavior (`__str__`, `__eq__`, `__lt__`)

Hashing has only the derive: a set or dict hashes through `@derive(Hash)` or `Hash` in `@rust.derive(...)`, and a method named `__hash__` is an ordinary method that does not provide it.

If you try to do *both* for the same capability, that’s a **conflict** and should be treated as an error: the compiler must not have to guess which implementation “wins”.

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

## How derive requirements are decided

A `model`, `class` or `enum` always derives `Clone` and `Debug`, so its field types must implement both ([Automatic derives](../reference/derives_and_traits.md#automatic-derives)). A set compares and hashes its elements, and a dict its keys, so their types must implement `Eq` and `Hash` ([Comparison → Hash](../reference/derives/comparison.md#set-elements-and-dict-keys)). For both requirements the checker refuses a type only when it knows the type lacks the derive. It knows that for builtin types and for types declared in the module being checked. For a type declared in another module it knows only the derives that module's declaration lists, and a `@rust.derive(...)` there may add more, so such a type is never refused for a missing derive. A type parameter, a type imported from Rust and a `rusttype` are not refused; the Rust build is the final check for them.

A generic function or method hashes its type parameter `T` when its body uses a value of type `T` as a set literal element, a dict literal key or a dict comprehension key, calls `set(...)` on a collection of `T`, or passes `T` on to a function or method that hashes it. That requirement is part of the callable's signature, so it is known before any body is checked and whatever order the declarations come in. A compiled library records it in its manifest as `Eq` and `Hash` bounds on the type parameter, marked as inferred, and a consumer applies them under the same rule: a library type whose derives come from `@rust.derive(...)` is not refused.

The type argument checked is the one the call instantiates the parameter with, including the element type of a literal argument: `unique([Tag.A])` instantiates `T` as `Tag`. A type parameter declared `with (Eq, Hash)` is checked by those bounds instead.

When a generic function passes its own type parameter on to a callee that hashes it, what happens depends on where the callee lives:

- A callee from the same module or an imported source module moves the requirement onto the calling function: the caller now hashes that parameter too, and its own calls are checked.
- A compiled library's callee cannot do that, because the caller's generated signature carries only the bounds its declaration spells. So the forwarded type parameter is refused with `INCAN-T0114` unless it is declared `with (Eq, Hash)`.

```incan
from pub::hashing import unique  # a compiled library

def mine[T](items: list[T]) -> set[T]:
    return unique(items)  # refused: INCAN-T0114, T is not declared with (Eq, Hash)

def bounded[T with (Eq, Hash)](items: list[T]) -> set[T]:
    return unique(items)  # accepted
```

---

## Non-goals (deliberate omissions)

Some Python/Rust features are intentionally not part of Incan’s trait/derive surface area:

- **Context managers** (`__enter__` / `__exit__`): prefer scope-based cleanup RAII (Resource Acquisition Is  Initialization) style
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
