# Derives: String representation (reference)

This page specifies `Debug` and `Display`. The derive catalog and the automatic derives are in [Derives and traits](../derives_and_traits.md).

## Debug

- **Provides**: `{value:?}` formatting.
- **Provided by**: every `model`, `class` and `enum`; a `newtype` whose underlying type implements `Debug`.
- **Behavior**: a model or class formats as its type name followed by its fields in declaration order, each as `name: value`, in braces. A `str` value is quoted.
- **Dunder**: none.
- **Requires**: every field type implements `Debug` (see [Automatic derives](../derives_and_traits.md#automatic-derives), `INCAN-T0113`).

## Display

- **Provides**: `{value}` formatting, `str(value)`, and `print(value)` and `println(value)`; satisfies a `Display` bound.
- **Provided by**: a `__str__(self) -> str` method, declared on the type, inherited from a class it extends, or supplied by an adopted trait; for an enum that declares values, such as `enum Level(str)`, each variant's value; for a type that adopts `Error` and has no `__str__`, its `message()` (see [Displaying an error](../stdlib_traits/error.md#displaying-an-error)). `@derive(Display)` provides nothing.
- **Behavior**: the value displays as the string `__str__` returns, a value enum's variant as its value, and an `Error` adopter without `__str__` as the string `message()` returns.
- **Dunder**: `__str__(self) -> str`.
- **Requires**: none.
- **Refused**: a `model`, `class`, `enum` or `newtype` value whose type provides no `Display`, in each position above (`INCAN-T0103`); see [Display](../strings.md#display).

## Formats

Frozen values use the same representation as their ordinary counterparts. `FrozenStr` is quoted in structured output; `FrozenList`, `FrozenSet` and `FrozenDict` format their elements structurally, including the decimal point of an integral `float`. `bytes` and `FrozenBytes` have no `Display` form and are refused with `INCAN-T0103`.

A union value inside structured output formats as its active member value. Generated union variant names are not part of the representation.

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

def main() -> None:
    p = Point(x=10, y=20)
    u = User(name="Alice", email="alice@example.com")
    println(f"{p:?} {u:?} {u} {Level.WARN}")
```

| Expression | Output |
| --- | --- |
| `f"{p:?}"` | `Point { x: 10, y: 20 }` |
| `f"{u:?}"` | `User { name: "Alice", email: "alice@example.com" }` |
| `f"{u}"` | `Alice <alice@example.com>` |
| `f"{Level.WARN}"` | `warn` |
