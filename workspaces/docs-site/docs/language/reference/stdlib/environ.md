# `std.environ`

`std.environ` provides read-only runtime access to current-process environment variables as Unicode strings or typed values converted through `TryFrom[str]`. Structured errors distinguish missing, invalid, non-Unicode, and malformed values without exposing observed environment contents.

```incan
from std.environ import get, get_optional, get_or, get_as

token = get("API_TOKEN")?
mode = get_optional("APP_MODE").unwrap_or("dev")
region = get_or("APP_REGION", "eu-west-1")
port = get_as[int]("PORT", default=8080)?
```

## String reads

`get(key: str) -> Result[str, EnvironError]` returns a required Unicode value. Missing keys, invalid keys, and non-Unicode host values return distinct errors. A key is invalid when it is empty or contains `=` or NUL.

`get_optional(key: str) -> Option[str]` returns `Some(value)` for a present Unicode value and `None` for missing, invalid-key, or non-Unicode reads. Use `get()` when code needs the precise failure category.

`get_or(key: str, default: str) -> str` returns the present Unicode value or `default` when no Unicode value is available. It has the same deliberately lossy error behavior as `get_optional()`.

## Typed reads

`get_as[T with TryFrom[str]](key: str) -> Result[Option[T], EnvironError]` returns `Ok(None)` when the key is absent. A present value is converted through `TryFrom[str]`; parse or validation failure returns `invalid_value`.

`get_as[T with TryFrom[str]](key: str, default: T) -> Result[T, EnvironError]` returns the default only when the key is absent. The default can be positional or named:

```incan
from std.environ import get_as

port = get_as[int]("PORT", 8080)?
timeout = get_as[float]("TIMEOUT", default=2.5)?
```

A malformed present value never falls back to the default. For example, `PORT=not-a-number` returns `invalid_value` even when a default is supplied.

The compiler provides `TryFrom[str]` conversion for `str`, `bool`, `int`, `float`, and the exact signed, unsigned, and binary floating-point numeric types. Boolean values use the canonical `true` and `false` spellings. Numeric values follow the lexical and range rules of their target type.

User-defined models, classes, enums, and newtypes can opt in by implementing `TryFrom[str]` explicitly:

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

## Validated newtypes

Newtype instantiations compose automatically when their underlying type supports `TryFrom[str]`. If a newtype defines `from_underlying`, typed reads use that checked constructor after parsing:

```incan
from std.environ import get_as

type Port = newtype int:
    def from_underlying(value: int) -> Result[Self, ValidationError]:
        if value < 1 or value > 65535:
            return Err(ValidationError("port must be between 1 and 65535"))
        return Ok(Port(value))


port = get_as[Port]("PORT", default=8080)?
```

`PORT=70000` fails validation. When `PORT` is absent, the integer default is converted through the ordinary checked newtype coercion path, so an invalid default does not bypass `from_underlying`.

## Errors

`EnvironError` exposes `kind()` and `kind_name()`, the requested `key`, and a redacted `detail` message. The stable categories are:

- `missing`: the key is not present.
- `invalid_key`: the key is empty or contains `=` or NUL.
- `invalid_value`: a typed read could not parse or validate the present value.
- `not_unicode`: the host value cannot be represented as Unicode text.
- `other`: an unexpected host failure.

`kind() -> EnvironErrorKind` returns the typed value enum. Its variants are `Missing`, `InvalidKey`, `InvalidValue`, `NotUnicode`, and `Other`. `kind_name() -> str` returns the corresponding lowercase spelling shown above when text output or serialization is more convenient.

Error details may include the key and expected target type. They never include the observed environment value.

## Runtime scope

Environment reads are runtime operations and are rejected in `const` initializers. The module does not mutate the current process environment and does not expose a byte-oriented host environment API.

Use `std.environ` for direct ambient reads. Structured application configuration belongs to the planned `ctx` surface, while planned `std.ci.env` helpers may add CI-specific policy without becoming the only environment namespace. Child-process environment construction belongs to the planned `std.process` command API rather than current-process reads.
