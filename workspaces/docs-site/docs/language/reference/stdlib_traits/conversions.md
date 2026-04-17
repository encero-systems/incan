# Conversion traits (Reference)

This page documents stdlib traits for explicit conversions.

Use these traits when a type should define an explicit conversion from or into another type.

The two main patterns are:

- `From[T]` / `Into[T]` for conversions that should always succeed
- `TryFrom[T]` / `TryInto[T]` for conversions that may fail

## From / Into

- **`From[T]`**
    - Hook: `@classmethod def from(cls, value: T) -> Self`
- **`Into[T]`**
    - Hook: `def into(self) -> T`

Example:

```incan
from std.traits.convert import From

model UserId with From[str]:
    value: int

    @classmethod
    def from(cls, value: str) -> Self:
        # accepts a str and converts it to int
        return UserId(value=int(value))


user_id = UserId.from("42")
```

## TryFrom / TryInto

- **`TryFrom[T]`**
    - Hook: `@classmethod def try_from(cls, value: T) -> Result[Self, str]`
- **`TryInto[T]`**
    - Hook: `def try_into(self) -> Result[T, str]`

Use `TryFrom[T]` when the conversion needs validation or parsing and may return an error instead of a value.
