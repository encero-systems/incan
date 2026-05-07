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
