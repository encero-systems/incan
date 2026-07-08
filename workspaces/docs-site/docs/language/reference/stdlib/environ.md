# `std.environ`

`std.environ` reads current-process environment variables at runtime. The initial surface is read-only and string-only:

```incan
from std.environ import get, get_optional, get_or

token = get("API_TOKEN")?
mode = get_optional("APP_MODE").unwrap_or("dev")
region = get_or("APP_REGION", "eu-west-1")
```

## Functions

`get(key: str) -> Result[str, EnvironError]` returns a required Unicode value or an `EnvironError`.

`get_optional(key: str) -> Option[str]` returns `Some(value)` for a present Unicode value and `None` otherwise. Use
`get()` when code needs to distinguish missing variables from invalid keys or non-Unicode host values.

`get_or(key: str, default: str) -> str` returns the present Unicode value or `default`.

## Errors

`EnvironError` has a stable `kind()` / `kind_name()` category, the requested `key`, and a redacted `detail` message. The current categories are:

- `missing`: the key is not present.
- `invalid_key`: the key is empty.
- `not_unicode`: the host value cannot be represented as Unicode text.
- `other`: reserved for unexpected host failures.

Error details include the key name where useful, but never include the observed environment value.

## Scope

The current `std.environ` surface is limited to string reads. Typed `get_as[T]` parsing, validated-newtype integration, bytes-oriented access, and current-process environment mutation are not part of this initial surface.
