# Error trait

The `Error` trait is the standard interface for custom error types used with `Result[T, E]`.

Implement it when you want:

- a human-readable message (`message()`)
- optional error chaining (`source()`)

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

A value whose type adopts `Error` renders its `message()` wherever a value is displayed: an f-string `{error}` part, `str(error)`, and each `print`/`println` argument. This covers a model or class that adopts `Error` directly or through a subtrait, a value of a type parameter bounded by `Error`, and `self` inside a default method of a trait that extends `Error`. A type with a `Display` of its own (a `__str__`, declared or supplied by an adopted trait, or a `Display` bound) renders that instead; for `self` in a default method, that is decided by each adopter. `{error:?}` renders `Debug` either way.

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
        return f"reported: {self}"  # self.message(), or the adopter's own __str__ when it declares one
```

The standard library's own error types, such as `IoError` from `std.io`, follow the same rule.
