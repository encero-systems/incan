# Error trait

`Error` is the trait for the error type `E` of `Result[T, E]`.

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

A value whose type adopts `Error` and has no `Display` of its own (no `__str__`, and no adopted `Display`) renders its `message()` wherever a value is displayed: an f-string `{value}` part, `str(value)`, and a `print` or `println` argument. `{value:?}` renders `Debug`.

```incan
from std.traits.error import Error

model ParseFailure with Error:
    field: str
    reason: str

    def message(self) -> str:
        return f"cannot parse {self.field}: {self.reason}"

def main() -> None:
    failure = ParseFailure(field="age", reason="not a number")
    println(f"error {failure}")   # error cannot parse age: not a number
    println(str(failure))         # cannot parse age: not a number
```

See [Error handling](../../explanation/error_handling.md) for how errors are propagated and handled.
