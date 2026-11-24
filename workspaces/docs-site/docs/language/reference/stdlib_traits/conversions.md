# Conversion traits (Reference)

`std.traits.convert` defines traits for explicit conversion. `From[T]` and `Into[T]` return the converted value directly. `TryFrom[T]` and `TryInto[T]` return a `Result`.

## From / Into

- **`From[T]`**
    - Hook: `@classmethod def from(cls, value: T) -> Self`
- **`Into[T]`**
    - Hook: `def into(self) -> T`

Example:

```incan
from std.traits.convert import From

model UserId with From[int]:
    value: int

    @classmethod
    def from(cls, value: int) -> Self:
        return UserId(value=value)


user_id = UserId.from(42)
```

A model, class, or enum may adopt multiple `Into[T]` instantiations for distinct target types. Each `into()` call on that type requires an expected result type that selects one instantiation.

```incan
from std.traits.convert import Into

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

This `Reading` has two `into()` implementations. A call without an expected result type is ambiguous.

```incan
value = reading.into()  # refused: no expected result type
```

A call argument takes its expected type from the corresponding parameter.

```incan
def takes_float(value: float) -> None:
    pass

takes_float(reading.into())  # accepted: selects Into[float]
```

## TryFrom / TryInto

- **`TryFrom[T]`**
    - Hook: `@classmethod def try_from(cls, value: T) -> Result[Self, str]`
- **`TryInto[T]`**
    - Hook: `def try_into(self) -> Result[T, str]`

`Ok` carries the converted value. `Err` carries the conversion error text.

## See also

- [Trait dispatch and overloading](../../explanation/derives_and_traits.md#trait-dispatch-and-overloading)
