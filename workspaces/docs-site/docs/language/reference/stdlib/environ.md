# `std.environ`

`std.environ` reads the current process's environment variables, as Unicode strings or as typed values converted through `TryFrom[str]`, and its argument vector. It never mutates the environment and never exposes an observed environment value in an error.

```incan
from std.environ import EnvironError, EnvironErrorKind
from std.environ import args, get, get_as, get_optional, get_or
```

## Functions

| Signature | Contract |
| --- | --- |
| `args() -> list[str]` | Returns the process argument vector: the program name first, exactly as the host supplied it, then each argument in order. Every entry is Unicode text; an argument that is not valid Unicode ends the process with a panic rather than being replaced or dropped, because the vector is read as a whole and there is no `EnvironError` to return. |
| `get(key: str) -> Result[str, EnvironError]` | Returns the present Unicode value of `key`. A missing key, an invalid key, and a non-Unicode host value return distinct error kinds. |
| `get_optional(key: str) -> Option[str]` | Returns `Some(value)` for a present Unicode value and `None` for a missing key, an invalid key, or a non-Unicode value. Use `get` when the failure category matters. |
| `get_or(key: str, default: str) -> str` | Returns the present Unicode value, or `default` whenever `get_optional` would return `None`. |
| `get_as[T with TryFrom[str]](key: str) -> Result[Option[T], EnvironError]` | Returns `Ok(None)` when `key` is absent. A present value is converted through `TryFrom[str]`; a conversion or validation failure returns `invalid_value`. |
| `get_as[T with TryFrom[str]](key: str, default: T) -> Result[T, EnvironError]` | Returns `default` only when `key` is absent; the default may be passed positionally or as `default=`. A present value that fails conversion returns `invalid_value` even when a default is supplied. |

A key is invalid when it is empty or contains `=` or NUL.

```incan
from std.environ import args, get, get_as, get_optional, get_or

token = get("API_TOKEN")?
mode = get_optional("APP_MODE").unwrap_or("dev")
region = get_or("APP_REGION", "eu-west-1")
port = get_as[int]("PORT", default=8080)?
command = args()[1:]
```

## Conversions for `get_as`

The compiler provides `TryFrom[str]` for `str`, `bool`, `int`, `float`, and the exact signed, unsigned, and binary floating-point numeric types. Boolean values use the spellings `true` and `false`. Numeric values follow the lexical and range rules of the target type.

A model, class, enum, or newtype can be read with `get_as` by implementing `TryFrom[str]`:

```incan
from std.environ import get_as
from std.traits.convert import TryFrom

model Deployment with TryFrom[str]:
    name: str

    @classmethod
    def try_from(cls, value: str) -> Result[Self, str]:
        if len(value) == 0:
            return Err("deployment must not be empty")
        return Ok(Deployment(name=value))


deployment = get_as[Deployment]("DEPLOYMENT")?
```

A newtype over a convertible underlying type is converted through that type. When the newtype defines `from_underlying`, the parsed value passes through that checked constructor, and so does a supplied default when the key is absent:

```incan
from std.environ import get_as

type Port = newtype int:
    def from_underlying(value: int) -> Result[Self, ValidationError]:
        if value < 1 or value > 65535:
            return Err(ValidationError("port must be between 1 and 65535"))
        return Ok(Port(value))


port = get_as[Port]("PORT", default=8080)?
```

`PORT=70000` returns `invalid_value`; an absent `PORT` returns `Port(8080)`.

## `EnvironError`

`EnvironError` implements `Error`.

| Member | Contract |
| --- | --- |
| `key: str` | The key the read was asked for. |
| `detail: str` | A redacted description; it may name the key and the expected target type, never the observed value. |
| `kind(self) -> EnvironErrorKind` | The stable category. |
| `kind_name(self) -> str` | The category's lowercase spelling, as listed below. |
| `message(self) -> str` | `detail`. |

## `EnvironErrorKind`

| Variant | `kind_name()` | Returned when |
| --- | --- | --- |
| `Missing` | `missing` | The key is not present. |
| `InvalidKey` | `invalid_key` | The key is empty or contains `=` or NUL. |
| `InvalidValue` | `invalid_value` | A typed read could not convert or validate the present value. |
| `NotUnicode` | `not_unicode` | The host value cannot be represented as Unicode text. |
| `Other` | `other` | An unexpected host failure. |

## Constraints

- Every read is a runtime operation; a call to any function of this module in a `const` initializer is a type error.
- The module reads the process environment and argument vector only. It does not set variables, does not parse arguments, and does not expose bytes for values that are not Unicode.
