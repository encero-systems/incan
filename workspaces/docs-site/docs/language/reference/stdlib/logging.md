# `std.logging` reference

`std.logging` provides source-defined structured logging for ordinary Incan programs. For task-oriented examples, see [Logging](../../how-to/logging.md).

## Imports

```incan
from std.logging import ColorPolicy, Level, LogFormat, LogRecord, LogStyle, Logger, basic_config, get_logger
```

## Logger acquisition

| API | Returns | Description |
| --- | --- | --- |
| `get_logger(name: Option[str] = None)` | `Logger` | Return a logger with no bound fields. When `name` is omitted, caller module metadata is used where available and metadata-free snippets fall back to `"root"`. |
| `log` | `Logger` | Ambient logger binding available when no local `log` binding shadows it. |

Logger names use the validated `LoggerName` newtype. Names are dot-separated and must not be empty, start or end with `.`, or contain empty segments such as `app..loader`.

## `Logger`

| API | Returns | Description |
| --- | --- | --- |
| `logger.child(suffix: str)` | `Logger` | Return a descendant logger named `<logger.name>.<suffix>`. |
| `logger.bind(fields)` | `Logger` | Return a logger with additional bound structured context. New fields override existing bound fields with the same key. |
| `logger.is_enabled(level: Level)` | `bool` | Return whether an event at `level` would pass the current source-level threshold. |
| `logger.trace(message: str, fields = {})` | `None` | Emit a trace event if enabled. |
| `logger.debug(message: str, fields = {})` | `None` | Emit a debug event if enabled. |
| `logger.info(message: str, fields = {})` | `None` | Emit an info event if enabled. |
| `logger.warning(message: str, fields = {})` | `None` | Emit a warning event if enabled. |
| `logger.error(message: str, fields = {})` | `None` | Emit an error event if enabled. |
| `logger.critical(message: str, fields = {})` | `None` | Emit a critical event if enabled. |

Event fields accept strings, booleans, integers, floats, `None`, and `std.telemetry.core.TelemetryValue` for nested arrays, maps, encoded bytes, or values that have already crossed a telemetry boundary. Event-specific fields override bound fields with the same key for that event.

## Levels

`Level` is a value enum ordered from least to most severe:

| Level | Alias |
| --- | --- |
| `Level.TRACE` | |
| `Level.DEBUG` | |
| `Level.INFO` | |
| `Level.WARN` | `Level.WARNING` |
| `Level.ERROR` | |
| `Level.FATAL` | `Level.CRITICAL` |

`Level.rank()` returns the ordering rank used by source-level threshold filtering. `Level.severity_number()` returns the OpenTelemetry severity number. `Level.display_name()` returns the human-facing level name.

## Configuration

Application entrypoints configure source-level logging policy with `basic_config(...)`:

| Parameter | Default | Meaning |
| --- | --- | --- |
| `level: Level` | `Level.WARN` | Minimum event level to emit. |
| `format: LogFormat` | `LogFormat.HUMAN` | Renderer format. |
| `style: LogStyle` | `LogStyle.SHORT` | Human renderer style. |
| `color: ColorPolicy` | `ColorPolicy.AUTO` | Accepted as part of the committed configuration shape; the current source renderer is colorless. |
| `target: str` | `"stderr"` | Output target name. Valid values are `"stdout"` and `"stderr"`. |

Libraries should acquire loggers and emit events, but should not call `basic_config(...)`.

## Renderer policy enums

| Enum | Values |
| --- | --- |
| `LogFormat` | `HUMAN`, `JSON` |
| `LogStyle` | `MINIMAL`, `SHORT`, `COMPLETE`, `VERBOSE` |
| `ColorPolicy` | `AUTO`, `ALWAYS`, `NEVER` |

## `LogRecord`

`LogRecord` is the structured event record produced by logger event methods. It uses Incan field names and OpenTelemetry aliases for JSON output.

| Field | Meaning |
| --- | --- |
| `timestamp` | Time when the event occurred. |
| `observed_timestamp` | Time when telemetry observed the event, when present. |
| `trace_id`, `span_id`, `trace_flags` | Optional span correlation fields. |
| `severity_text`, `severity_number` | Event severity. |
| `body` | Human or structured event body. |
| `resource` | Entity that produced the telemetry. |
| `instrumentation_scope` | Logical logger scope that emitted the record. |
| `attributes` | Structured event attributes. |
| `event_name` | Optional event class or type name. |

`LogRecord.emit(config)` renders and writes the record. `LogRecord.render(config)` returns the rendered text.

## Boundaries

`std.logging` uses `std.datetime` for timestamps and ordinary `rust::std::io` imports for stdout/stderr delivery. It does not use a Rust backing logging module. Project defaults in `incan.toml`, `INCAN_LOG_*` environment overrides, `incan run --log-*` flags, exporters, and colorized terminal output are not implemented yet.

## See also

- [Logging](../../how-to/logging.md)
- [`std.datetime` reference](datetime.md)
