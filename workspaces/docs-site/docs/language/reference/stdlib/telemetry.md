# `std.telemetry` reference

`std.telemetry` provides pure data-model types for observability-facing stdlib modules. Importing it does not configure exporters, install providers, start background tasks, or capture runtime context.

## Imports

```incan
from std.telemetry import Attributes, InstrumentationScope, Resource, SpanContext, TelemetryValue
from std.telemetry.core import TraceFlags, TraceId, SpanId, Timestamp
```

Use the top-level `std.telemetry` prelude for ordinary data-model imports. Use `std.telemetry.core` when code should make the internal layering explicit or avoid prelude-style imports.

## `TelemetryValue`

`TelemetryValue` carries structured values across logging and future telemetry boundaries without forcing callers to stringify nested data.

| API | Returns | Description |
| --- | --- | --- |
| `TelemetryValue.none()` | `TelemetryValue` | Null telemetry value. |
| `TelemetryValue.string(value: str)` | `TelemetryValue` | String value. |
| `TelemetryValue.bool(value: bool)` | `TelemetryValue` | Boolean value. |
| `TelemetryValue.int(value: int)` | `TelemetryValue` | Integer value. |
| `TelemetryValue.float(value: float)` | `TelemetryValue` | Floating-point value. |
| `TelemetryValue.bytes(value: str)` | `TelemetryValue` | Encoded byte value; the caller owns the encoding convention. |
| `TelemetryValue.array(values: list[TelemetryValue])` | `TelemetryValue` | Nested telemetry array. |
| `TelemetryValue.map(values: Dict[str, TelemetryValue])` | `TelemetryValue` | Nested telemetry map. |
| `value.display_text()` | `str` | Human-oriented text; strings render directly and structured values render as JSON. |

The `TelemetryValueKind` enum uses stable string values: `NONE`, `STRING`, `BOOL`, `INT`, `FLOAT`, `BYTES`, `ARRAY`, and `MAP`.

## Attributes

| API | Returns | Description |
| --- | --- | --- |
| `Attributes(fields)` | `Attributes` | Newtype wrapper around `Dict[str, TelemetryValue]`. |
| `Attributes.from_string_fields(fields: Dict[str, str])` | `Attributes` | Convert ordinary string fields into structured telemetry attributes. |

Attribute keys may use OpenTelemetry semantic-convention names such as `service.name`, `http.request.method`, or `gen_ai.request.model`. Values remain structured through the data-model boundary so logging and future telemetry exporters can decide how to render them.

## Resource, Scope, And Context

| Type | Description |
| --- | --- |
| `Timestamp` | RFC 3339-style timestamp string newtype used by records that already have a time value. |
| `Resource` | Entity that produced telemetry, currently represented as structured attributes. |
| `InstrumentationScope` | Logical scope name, optional version, and optional schema URL for the code that emitted telemetry. |
| `TraceId` | W3C/OpenTelemetry trace-id string newtype. |
| `SpanId` | W3C/OpenTelemetry span-id string newtype. |
| `TraceFlags` | W3C trace-flags string newtype. |
| `SpanContext` | Serializable grouping of trace id, span id, and optional flags. |

These types are inert data holders in 0.3. They preserve identifiers and attributes when another stdlib module, such as `std.logging`, already has structured observability data to carry.

## Boundaries

`std.telemetry` is not a provider API in 0.3. It does not sample spans, manage active context, export metrics, configure OpenTelemetry SDKs, or read process resource attributes. Those behaviors belong to a future telemetry provider surface; this module only provides the shared typed payload shape.

## See also

- [`std.logging`](logging.md)
