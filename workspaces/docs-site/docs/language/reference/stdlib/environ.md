# `std.environ`

`std.environ` reads the current process's environment variables, as Unicode strings or as typed values converted through `TryFrom[str]`, and its argument vector. It never mutates the environment and never exposes an observed environment value in an error.

```incan
from std.environ import EnvironError, EnvironErrorKind
from std.environ import args, get, get_as, get_optional, get_or
```

## Functions

| Signature | Contract |
| --- | --- |
| `args() -> Result[list[str], EnvironError]` | Returns the process argument vector: the program name first, exactly as the host supplied it, then each argument in order. Every entry is Unicode text; the first argument that is not valid Unicode returns `not_unicode` with `key` set to its position, `argv[2]`, and the observed bytes never appear in the error. |
| `get(key: str) -> Result[str, EnvironError]` | Returns the present Unicode value of `key`. A missing key returns `missing`, an invalid key `invalid_key`, and a non-Unicode host value `not_unicode`. |
| `get_optional(key: str) -> Option[str]` | Returns `Some(value)` for a present Unicode value and `None` for a missing key, an invalid key, or a non-Unicode value. |
| `get_or(key: str, default: str) -> str` | Returns the present Unicode value, or `default` whenever `get_optional` would return `None`. |
| `get_as[T with TryFrom[str]](key: str) -> Result[Option[T], EnvironError]` | Returns `Ok(None)` when `key` is absent. A present value is converted through `TryFrom[str]`; a conversion or validation failure returns `invalid_value`. An invalid key and a non-Unicode value return the errors `get` returns. |
| `get_as[T with TryFrom[str]](key: str, default: T) -> Result[T, EnvironError]` | Returns `default` only when `key` is absent; the default may be passed positionally or as `default=`. A present value that fails conversion returns `invalid_value` even when a default is supplied. |

A key is invalid when it is empty or contains `=` or NUL.

```incan
from std.environ import EnvironError, args, get, get_as, get_optional, get_or

def main() -> Result[None, EnvironError]:
    token = get("API_TOKEN")?
    mode = get_optional("APP_MODE").unwrap_or("dev")
    region = get_or("APP_REGION", "eu-west-1")
    port = get_as[int]("PORT", default=8080)?
    command = args()?[1:]
    println(f"{len(token)} {mode} {region} {port} {len(command)}")
    return Ok(None)
```

## Conversions for `get_as`

`str`, `bool`, `int`, `float`, and the exact signed, unsigned, and binary floating-point numeric types provide `TryFrom[str]`. Boolean values use the spellings `true` and `false`. Numeric values follow the lexical and range rules of the target type.

A model, class, enum, or newtype that adopts `TryFrom[str]` can be read with `get_as`:

```incan
from std.environ import EnvironError, get_as
from std.traits.convert import TryFrom

model Deployment with TryFrom[str]:
    name: str

    @classmethod
    def try_from(cls, value: str) -> Result[Self, str]:
        if len(value) == 0:
            return Err("deployment must not be empty")
        return Ok(Deployment(name=value))


def main() -> Result[None, EnvironError]:
    match get_as[Deployment]("DEPLOYMENT")?:
        Some(deployment) => println(deployment.name)
        None => println("no deployment")
    return Ok(None)
```

A newtype over a convertible underlying type is converted through that type. When the newtype defines `from_underlying`, the parsed value passes through that checked constructor, and so does a supplied default when the key is absent:

```incan
from std.environ import EnvironError, get_as

type Port = newtype int:
    def from_underlying(value: int) -> Result[Self, ValidationError]:
        if value < 1 or value > 65535:
            return Err(ValidationError("port must be between 1 and 65535"))
        return Ok(Port(value))


def main() -> Result[None, EnvironError]:
    port = get_as[Port]("PORT", default=8080)?
    println(port.0)
    return Ok(None)
```

`PORT=70000` returns `invalid_value`; an absent `PORT` returns `Port(8080)`.

A type argument that does not provide `TryFrom[str]`, such as `list[int]`, is refused (`INCAN-T0001`).

## `EnvironError`

`EnvironError` is a class that implements `Error` and `Clone`. It displays its `detail`.

| Member | Contract |
| --- | --- |
| `key: str` | The key the read was asked for; for `args`, the position of the refused argument as `argv[N]`. |
| `detail: str` | A redacted description; it may name the key and the expected target type, never the observed value. |
| `kind(self) -> EnvironErrorKind` | The stable category. |
| `kind_name(self) -> str` | The category's lowercase spelling, `kind().as_str()`. |
| `message(self) -> str` | `detail`. |
| `source(self) -> Option[str]` | `None`. |

| Constructor | Result |
| --- | --- |
| `EnvironError.from_kind(kind: EnvironErrorKind, key: str, detail: str) -> EnvironError` | An error of `kind` with `key` and `detail` as given. |
| `EnvironError.missing(key: str) -> EnvironError` | Kind `Missing`; `detail` names `key`. |
| `EnvironError.invalid_key(key: str) -> EnvironError` | Kind `InvalidKey`; `detail` states the key rule and does not name `key`. |
| `EnvironError.not_unicode(key: str) -> EnvironError` | Kind `NotUnicode`; `detail` names `key`. |
| `EnvironError.invalid_value(key: str, target: str) -> EnvironError` | Kind `InvalidValue`; `detail` names `key` and `target`. |

## `EnvironErrorKind`

`EnvironErrorKind` is a `str` value enum (`enum EnvironErrorKind(str)`). Each variant's value is its lowercase spelling; `value()` and `as_str(self) -> str` return it, and a variant displays it.

| Variant | Value | Returned when |
| --- | --- | --- |
| `Missing` | `missing` | The key is not present. |
| `InvalidKey` | `invalid_key` | The key is empty or contains `=` or NUL. |
| `InvalidValue` | `invalid_value` | A typed read could not convert or validate the present value. |
| `NotUnicode` | `not_unicode` | The host value, or a process argument, cannot be represented as Unicode text. |
| `Other` | `other` | An unexpected host failure. |

## Constraints

- Every read is a runtime operation: a call of a function of this module in a `const` initializer is refused (`INCAN-T0001`).
- The module reads the process environment and argument vector only. It does not set variables, does not parse arguments, and does not expose bytes for values that are not Unicode.

## See also

- [Read an environment variable and branch on the failure](../../how-to/error_handling_recipes.md#pattern-read-an-environment-variable-and-branch-on-the-failure)
