A static method belongs to the type rather than an instance — it has no `self` receiver.

```incan
class Temperature:
    celsius: float

    @staticmethod
    pub def from_fahrenheit(f: float) -> Temperature:
        return Temperature(celsius=(f - 32.0) / 1.8)

def main() -> None:
    t = Temperature.from_fahrenheit(98.6)
    println(t.celsius)
```

Type-owned methods and instance-owned fields are separate receiver surfaces. A static factory may therefore share a field's spelling without ambiguity: `TimeDelta.days(7)` selects the factory, while `delta.days` reads the stored field. Static methods are not callable through an instance; use the declaring type at the call site.

**Applies to**: methods on `class`, `model`, `enum`, and `newtype` declarations.

Rules:

- A `@staticmethod` method **must not** have a `self` or `mut self` parameter. The compiler rejects this.
- Call static methods via the type name: `TypeName.method_name(...)`.
- Generic static factory methods can return `Self`; call them with explicit type arguments when inference needs the owner type, such as `Box[int].make(...)`.
- A Rust-backed static method uses an ordinary Incan body that calls a directly imported Rust function.

Use cases:

- **Factory methods**: alternative constructors (`from_fahrenheit`, `from_json`, `parse`).
- **Utility functions**: logic scoped to a type that doesn't need instance state.
- **Rust interop**: a static wrapper can keep an Incan-facing signature while calling a private `rust::` import.

Generic static factories can use the declaring type constructor directly:

```incan
class Box[T with Clone]:
    value: T

    @staticmethod
    pub def make(value: T) -> Self:
        return Box(value=value)

def main() -> None:
    boxed = Box[int].make(1)
    println(str(boxed.value))
```
