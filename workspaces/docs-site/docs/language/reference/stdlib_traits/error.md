# Error trait

`Error` is the trait for error types used as the `E` of `Result[T, E]`. An adopter defines `message()`. `source()` returns `None` unless the adopter defines it.

## Definition

```incan
trait Error:
    def message(self) -> str:
        """Return a human-readable error message"""
        ...

    def source(self) -> Option[str]:
        """Optional: Return the underlying cause of this error"""
        return None
```

## Example: simple structured error

```incan
model AgeValidationError with Error:
    field: str
    msg: str

    def message(self) -> str:
        return f"Validation failed for '{self.field}': {self.msg}"

def validate_age(age: int) -> Result[int, AgeValidationError]:
    if age < 0:
        return Err(AgeValidationError(field="age", msg="cannot be negative"))
    return Ok(age)
```

## Example: chaining with `source()`

```incan
model DatabaseError with Error:
    query: str
    cause: Option[str]

    def message(self) -> str:
        return f"Database query failed: {self.query}"

    def source(self) -> Option[str]:
        return self.cause
```

## Displaying an error

A value whose type adopts `Error` renders its `message()` in an f-string `{value}` part, in `str(value)`, and as a `print` or `println` argument. This holds for a value of a type parameter bounded by `Error` and for `self` in a default method of a trait that extends `Error`. A type with a `Display` of its own (a `__str__`, or a `Display` it adopts) renders that instead. `{value:?}` renders `Debug`.

```incan
from std.traits.error import Error

model ParseFailure with Error:
    field: str
    reason: str

    def message(self) -> str:
        return f"cannot parse {self.field}: {self.reason}"

def main() -> None:
    failure = ParseFailure(field="age", reason="not a number")
    println(f"error {failure}")  # error cannot parse age: not a number
    println(str(failure))        # cannot parse age: not a number
    println(failure)             # cannot parse age: not a number
```

```incan
from std.traits.error import Error

def describe[E with Error](error: E) -> str:
    return f"failed: {error}"  # "failed: " followed by error.message()

trait Reported with Error:
    def report(self) -> str:
        return f"reported: {self}"  # the adopter's __str__ if it has one, else self.message()
```
