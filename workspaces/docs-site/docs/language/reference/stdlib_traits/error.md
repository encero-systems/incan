# Error trait

`Error` is the standard-library trait for error types, declared in `std.traits.error` and re-exported by `std.traits`. The `E` of `Result[T, E]` is any type; it is not required to adopt `Error`.

## Methods

| Method | Returns | Required |
| --- | --- | --- |
| `message(self) -> str` | The error's message. | Yes |
| `source(self) -> Option[str]` | The underlying cause. | No; the default returns `None`. |

```incan
trait Error:
    def message(self) -> str: ...

    def source(self) -> Option[str]:
        return None
```

## Displaying an error

These values render their `message()` wherever a value is displayed, in an f-string `{value}` part, `str(value)`, and a `print` or `println` argument, unless the value's type has a `Display` of its own:

- a value of a `model`, `class`, `enum` or `newtype` that adopts `Error`, directly or through a trait that extends `Error`;
- a value of a type parameter bounded by `Error`, or by a trait that extends it;
- `self` in a default method of a trait that extends `Error`, where the type is the adopter's.

A type has a `Display` of its own when it:

- defines `__str__`, declared on the type, inherited from a class it extends, or supplied by an adopted trait or one of its supertraits;
- is an enum that declares values, such as `enum Code(str)`: a variant displays its value;
- adopts `Display`, directly or through a supertrait;
- carries `@derive(Display)`, which displays it as its `{value:?}` structure;
- takes a Rust derive macro named `Display`, forwarded with `@rust.derive(...)` or imported from a Rust crate and named in `@derive(...)`.

A type parameter has a `Display` of its own when one of its bounds is `Display` or supplies `__str__`. `{value:?}` renders `Debug`.

```incan
from std.traits.error import Error

model ParseFailure with Error:
    field: str
    reason: str

    def message(self) -> str:
        return f"cannot parse {self.field}: {self.reason}"


enum Code(str) with Error:
    Missing = "missing"

    def message(self) -> str:
        return "a required value is missing"


def describe[E with Error](error: E) -> str:
    return f"failed: {error}"


def main() -> None:
    failure = ParseFailure(field="age", reason="not a number")
    println(f"error {failure}")
    println(str(failure))
    println(describe(failure))
    println(str(Code.Missing))
```

| Expression | Output |
| --- | --- |
| `f"error {failure}"` | `error cannot parse age: not a number` |
| `str(failure)` | `cannot parse age: not a number` |
| `describe(failure)` | `failed: cannot parse age: not a number` |
| `str(Code.Missing)` | `missing` |

## See also

- [Error handling](../../explanation/error_handling.md): how errors are propagated and handled.
- [Define an error type with `Error`](../../how-to/error_handling_recipes.md#pattern-define-an-error-type-with-error).
