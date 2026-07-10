# `std.environ`

`std.environ` reads current-process environment variables at runtime. The current surface is read-only and centered on Unicode string reads:

```incan
from std.environ import get, get_optional, get_or, get_as
from std.traits.convert import TryFrom

token = get("API_TOKEN")?
mode = get_optional("APP_MODE").unwrap_or("dev")
region = get_or("APP_REGION", "eu-west-1")
```

## Functions

`get(key: str) -> Result[str, EnvironError]` returns a required Unicode value or an `EnvironError`.

`get_optional(key: str) -> Option[str]` returns `Some(value)` for a present Unicode value and `None` otherwise. Use `get()` when code needs to distinguish missing variables from invalid keys or non-Unicode host values.

`get_or(key: str, default: str) -> str` returns the present Unicode value or `default`.

`get_as[T with TryFrom[str]](key: str) -> Result[Option[T], EnvironError]` reads a Unicode value and converts it through the target type's `TryFrom[str]` implementation. It returns `Ok(None)` when the key is absent and `Err(EnvironError)` when the key is invalid, the host value is not Unicode, or the target conversion rejects the present value.

```incan
from std.environ import get_as
from std.traits.convert import TryFrom

model Port with TryFrom[str]:
    value: int

    @classmethod
    def try_from(cls, value: str) -> Result[Self, str]:
        port = int(value)
        if port < 1 or port > 65535:
            return Err("port out of range")
        return Ok(Port(value=port))


port: Option[Port] = get_as[Port]("PORT")?
```

## Errors

`EnvironError` has a stable `kind()` / `kind_name()` category, the requested `key`, and a redacted `detail` message. The current categories are:

- `missing`: the key is not present.
- `invalid_key`: the key is empty.
- `invalid_value`: a typed read could not parse or validate the present value.
- `not_unicode`: the host value cannot be represented as Unicode text.
- `other`: reserved for unexpected host failures.

Error details include the key name where useful, but never include the observed environment value.

## Scope

The current `get_as` surface is trait-based. It supports targets that explicitly implement `TryFrom[str]`; it does not yet provide the full RFC 089 primitive parsing and defaulted overload surface such as `get_as[int]("PORT", default=8080)`. Bytes-oriented access and current-process environment mutation are also outside the current surface.
