# Derives: String representation (reference)

This page specifies `Debug` and `Display`. The derive catalog and the automatic derives are in [Derives and traits](../derives_and_traits.md).

## Debug

- **Provides**: `{value:?}` formatting.
- **Provided by**: every `model`, `class` and `enum`; a `newtype` whose underlying type implements `Debug`.
- **Behavior**: a model or class formats as its type name followed by its fields in declaration order, each as `name: value`, in braces. An enum value formats as its variant name, followed by its payloads in parentheses when it has any: `Red`, `Leaf(1)`, `Error(404, "missing")`. A newtype formats as its type name followed by its underlying value in parentheses: `UserId(42)`. A `str` value is quoted.
- **Dunder**: none.
- **Requires**: every field type implements `Debug` (see [Automatic derives](../derives_and_traits.md#automatic-derives), `INCAN-T0113`).

## Display

- **Provides**: `{value}` formatting, `str(value)`, and `print(value)` and `println(value)`; satisfies a `Display` bound.
- **Provided by**: a `__str__(self) -> str` method, declared on the type, inherited from a class it extends, or supplied by an adopted trait; for an enum that declares values, such as `enum Level(str)`, each variant's value; `@derive(Display)` on a `model`, `class`, `enum` or `newtype`; for a type that adopts `Error` and has no `Display` by one of these, its `message()` (see [Displaying an error](../stdlib_traits/error.md#displaying-an-error)).
- **Behavior**: the value displays as the string `__str__` returns, a value enum's variant as its value, a type that derives `Display` as its `{value:?}` structure (see [Debug](#debug)), and an `Error` adopter without another `Display` as the string `message()` returns. On an enum that declares values, `@derive(Display)` displays the variant's value.
- **Dunder**: `__str__(self) -> str`.
- **Requires**: `@derive(Display)` implies `Debug`, so on a newtype its underlying type implements `Debug`.
- **Refused**: a `model`, `class`, `enum` or `newtype` value whose type provides no `Display`, in each position above (`INCAN-T0103`); see [Display](../strings.md#display). `@derive(Display)` on a type that has a `__str__`, declared, inherited or supplied by an adopted trait (`INCAN-T0001`).

## Formats

Frozen values use the same representation as their ordinary counterparts. `FrozenStr` is quoted in structured output; `FrozenList`, `FrozenSet` and `FrozenDict` format their elements structurally, including the decimal point of an integral `float`. `bytes` and `FrozenBytes` have no `Display` form and are refused with `INCAN-T0103`.

A union value inside structured output formats as its active member value alone.

```incan
model Point:
    x: int
    y: int

model User:
    name: str
    email: str

    def __str__(self) -> str:
        return f"{self.name} <{self.email}>"

enum Level(str):
    WARN = "warn"

enum Node:
    Leaf(int)
    Error(int, str)

type UserId = newtype int

@derive(Display)
model Tag:
    name: str

def main() -> None:
    p = Point(x=10, y=20)
    u = User(name="Alice", email="alice@example.com")
    leaf = Node.Leaf(1)
    error = Node.Error(404, "missing")
    user_id = UserId(42)
    t = Tag(name="new")
    println(f"{p:?} {u:?} {u} {Level.WARN} {t}")
    println(f"{leaf:?} {error:?} {user_id:?}")
```

| Expression | Output |
| --- | --- |
| `f"{p:?}"` | `Point { x: 10, y: 20 }` |
| `f"{u:?}"` | `User { name: "Alice", email: "alice@example.com" }` |
| `f"{u}"` | `Alice <alice@example.com>` |
| `f"{Level.WARN}"` | `warn` |
| `f"{leaf:?}"` | `Leaf(1)` |
| `f"{error:?}"` | `Error(404, "missing")` |
| `f"{user_id:?}"` | `UserId(42)` |
| `f"{t}"` | `Tag { name: "new" }` |
