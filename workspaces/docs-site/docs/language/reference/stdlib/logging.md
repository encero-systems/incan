# `std.logging`

`std.logging` provides source-defined structured logging for ordinary Incan programs. The logger types, configuration state, level filtering, bound context, human rendering, and JSON rendering are implemented in Incan stdlib source. `std.logging` uses the pure `std.telemetry.core` data model for log records so simple logs are OpenTelemetry-shaped without requiring users to configure exporters or the full `std.telemetry` provider.

The implementation uses `std.datetime` for timestamps. It does not use a Rust backing module or `@rust.extern` logging helpers.

```incan
from std.logging import Level, basic_config

def main() -> None:
    basic_config(level=Level.INFO, target="stdout")
    log.info("started", fields={"component": "worker"})
```

## Logger Acquisition

Use `get_logger(name: Option[str] = None) -> Logger` to acquire a logger.

For the common default logger, use ambient `log`. It behaves like binding `log = get_logger()` in the current module when no local `log` binding exists.

```incan
log.info("ready")
```

```incan
from std.logging import get_logger

def load(path: str) -> None:
    log = get_logger("etl.loader")
    log.info("loading", fields={"path": path})
```

Logger names are represented by the validated `LoggerName` newtype. Names are dot-separated and must not be empty, start or end with `.`, or contain empty segments such as `app..loader`. Calling `get_logger()` without a name uses the current source module name when the compiler has source metadata; command snippets and other metadata-free entry points fall back to `"root"`.

Use `child(...)` for hierarchical names:

```incan
def load(path: str) -> None:
    root = get_logger("etl")
    loader = root.child("loader")
    loader.info("loading", fields={"path": path})
```

## Structured Fields

Event fields are passed as a structured dictionary:

```incan
log.info("loaded rows", fields={"dataset": "users", "rows": 42, "cached": true})
```

Event methods accept primitive structured values such as strings, booleans, integers, floats, and `None`. They also accept `std.telemetry.core.TelemetryValue` for nested arrays, maps, encoded bytes, or values that have already crossed a telemetry boundary. JSON output stores fields as structured `std.telemetry.core.Attributes` values instead of concatenating them into the message text.

Dotted semantic keys such as `http.request.method`, `db.system.name`, `gen_ai.request.model`, or `mcp.method.name` are preserved as ordinary field keys. External telemetry conventions are not interpreted by `std.logging` itself.

Internally, logging events are represented as OpenTelemetry-aligned `LogRecord` values. Incan source uses snake_case field names such as `severity_text` and `instrumentation_scope`; wire-oriented serialization can use official OpenTelemetry aliases such as `SeverityText` and `InstrumentationScope`.

Use `bind(...)` for repeated context:

```incan
def handle(request_id: str, elapsed_ms: int) -> None:
    request_log = get_logger("api.request").bind({"request_id": request_id})
    request_log.info("accepted")
    request_log.warning("slow upstream", fields={"elapsed_ms": elapsed_ms})
```

Event fields override bound fields with the same key for that event.

## Levels

`Level` is a value enum:

- `Level.TRACE`
- `Level.DEBUG`
- `Level.INFO`
- `Level.WARN`
- `Level.ERROR`
- `Level.FATAL`

`Level.WARNING` aliases `Level.WARN`, and `Level.CRITICAL` aliases `Level.FATAL` for readability at call sites that prefer the longer names.

A configured threshold emits events at that level or above. `Logger.is_enabled(level)` reports whether a level is enabled for the current source-level policy.

## Configuration

Application entrypoints can configure logging with `basic_config(...)`:

```incan
from std.logging import Level, LogFormat, LogStyle, basic_config

def main() -> None:
    basic_config(
        level=Level.INFO,
        format=LogFormat.JSON,
        style=LogStyle.SHORT,
        target="stdout",
    )
```

Available renderer policy enums:

- `LogFormat.HUMAN`
- `LogFormat.JSON`
- `LogStyle.MINIMAL`
- `LogStyle.SHORT`
- `LogStyle.COMPLETE`
- `LogStyle.VERBOSE`
- `ColorPolicy.AUTO`
- `ColorPolicy.ALWAYS`
- `ColorPolicy.NEVER`

`basic_config(...)` controls the generated program's source-level logging state. Project defaults in `incan.toml`, `INCAN_LOG_*` environment overrides, and `incan run --log-*` flags are not implemented yet.

`ColorPolicy` is accepted as part of the committed configuration shape, but the current source renderer is colorless.

The `target` argument accepts `"stdout"` and `"stderr"`. Internally, the setting is stored as the validated `OutputTarget` newtype, which owns writing rendered log lines to the selected standard stream.

Libraries should acquire loggers and emit events, but should not call `basic_config(...)`.
