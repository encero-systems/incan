# Derives: String representation (reference)

This page specifies `Debug` and `Display`. The derive catalog and the automatic derives are in [Derives and traits](../derives_and_traits.md).

## Debug

- **Provides**: `{value:?}` formatting.
- **Provided by**: every `model`, `class` and `enum`; a `newtype` whose underlying type implements `Debug`.
- **Behavior**: a model or class formats as its type name followed by its fields in declaration order, each as `name: value`, in braces. A `str` value is quoted.
- **Dunder**: none.
- **Requires**: every field type implements `Debug` (see [Automatic derives](../derives_and_traits.md#automatic-derives), `INCAN-T0113`).

## Display

- **Provides**: `{value}` formatting, `str(value)`, and `print(value)` and `println(value)`.
- **Provided by**: a `__str__(self) -> str` method; for an enum that declares values, such as `enum Level(str)`, each variant's value; for a type that adopts `Error` and has no `__str__`, its `message()` (see [Displaying an error](../stdlib_traits/error.md#displaying-an-error)). `@derive(Display)` provides nothing.
- **Behavior**: the value displays as the string `__str__` returns.
- **Dunder**: `__str__(self) -> str`.
- **Requires**: none.

## Formats

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
