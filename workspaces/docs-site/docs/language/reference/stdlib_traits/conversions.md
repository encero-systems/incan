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

A model, class, or enum may adopt multiple `Into[T]` instantiations when the target types differ. A call to `into()` must then have an expected result type so the compiler can select the same-family trait instantiation:

```incan
model Reading with Into[int], Into[float]:
    value: int

    def into(self) -> int:
        return self.value

    def into(self) -> float:
        return 1.0

reading = Reading(value=1)
as_float: float = reading.into()
as_int: int = reading.into()
```

An untyped call is ambiguous because both `Into[int]` and `Into[float]` have the same method name and no value argument that distinguishes them:

```incan
value = reading.into()  # error: add an expected result type
```

Expected type context does not have to be a local binding. Passing the call into a function that expects a concrete target type works too:

```incan
def takes_float(value: float) -> None:
    pass

takes_float(reading.into())  # selects Into[float]
```

Use multiple `Into[T]` adoptions when the target type is the real static distinction. If the conversions need different input parameters or different names in user code, prefer explicit methods such as `to_json()` and `to_yaml()` instead of forcing everything through `into()`.

## TryFrom / TryInto

- **`TryFrom[T]`**
    - Hook: `@classmethod def try_from(cls, value: T) -> Result[Self, str]`
- **`TryInto[T]`**
    - Hook: `def try_into(self) -> Result[T, str]`

Use `TryFrom[T]` when the conversion needs validation or parsing and may return an error instead of a value.
