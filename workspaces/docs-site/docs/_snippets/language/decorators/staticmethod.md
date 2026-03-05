A static method belongs to the type rather than an instance — it has no `self` receiver.

```incan
class Temperature:
    celsius: float

    @staticmethod
    def from_fahrenheit(f: float) -> Temperature:
        return Temperature(celsius=(f - 32.0) / 1.8)

def main() -> None:
    t = Temperature.from_fahrenheit(98.6)
    println(t.celsius)
```

**Applies to**: methods on `class`, `model`, and `newtype` declarations.

Rules:

- A `@staticmethod` method **must not** have a `self` or `mut self` parameter. The compiler rejects this.
- Call static methods via the type name: `TypeName.method_name(...)`.
- `@staticmethod` can be combined with `@rust.extern` for Rust-backed static methods on types.

Use cases:

- **Factory methods**: alternative constructors (`from_fahrenheit`, `from_json`, `parse`).
- **Utility functions**: logic scoped to a type that doesn't need instance state.
- **Rust interop**: `@rust.extern` on type methods requires `@staticmethod` (instance delegation is not supported).
