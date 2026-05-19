# Work With Dynamic JSON

Use `std.json.JsonValue` when a payload is only partly known, mixed-shape, or intentionally open. Use typed `@derive(json)` models when the schema is stable.

## Extract A Required Nested Value

For required nested fields, prefer JSON Pointer plus `?` over hand-written lookup ladders:

```incan
from std.json import JsonError, JsonValue


def first_item_name(source: str) -> Result[str, JsonError]:
    data = JsonValue.parse(source)?
    name = data.require_pointer("/items/0/name")?
    return name.expect_str()
```

`require_pointer(...)` returns a `JsonError` when the path does not resolve. `expect_str()` keeps the final type check explicit.

## Read Optional Fields

Use direct indexing, `get(...)`, or `at(...)` when missing data is expected:

```incan
from std.json import JsonValue


def enabled_or_false(data: JsonValue) -> bool:
    if let Some(value) = data["enabled"]:
        if let Some(enabled) = value.as_bool():
            return enabled
    return false
```

Optional lookup preserves the difference between a missing key and a present JSON null. A missing key returns `None`; a present null returns `Some(JsonValue.null())`.

## Update A Dynamic Object

Use mutation helpers when building or transforming dynamic payloads:

```incan
from std.json import JsonError, JsonValue


def mark_seen(source: str) -> Result[str, JsonError]:
    mut data = JsonValue.parse(source)?
    data.set("seen", JsonValue.bool(true))?

    mut events = data.require("events")?
    events.append(JsonValue.object({"kind": JsonValue.str("view")}))?
    data.set("events", events)?

    return data.to_pretty_json()
```

`set(...)` and `append(...)` return `JsonError` when the receiver is the wrong JSON kind.

## Keep Stable Fields Typed

Use `JsonValue` only for the dynamic part of a typed model:

```incan
from std.json import JsonValue
from std.serde import json


@derive(json)
model ApiResponse:
    status: int
    message: str
    data: JsonValue
```

The `data` field serializes and deserializes as ordinary JSON, not as a wrapper object.

## See Also

- [`std.json` reference](../reference/stdlib/json.md)
- [Serialization derives reference](../reference/derives/serialization.md)
- [`std.math` reference](../reference/stdlib/math.md)
