# `std.logging` reference

`std.logging` provides source-defined structured logging for ordinary Incan programs. For task-oriented examples, see [Logging](../../how-to/logging.md).

## Imports

```incan
from std.logging import ColorPolicy, Level, LogFormat, LogRecord, LogStyle, Logger, basic_config, get_logger, log
```

## Logger acquisition

| API | Returns | Description |
| --- | --- | --- |
| `get_logger(name: Option[str] = None)` | `Logger` | Return a logger with no bound fields. When `name` is omitted, caller module metadata is used where available and metadata-free snippets fall back to `"root"`. |
| `log` | `Logger` | Root logger exported by the standard prelude. A parameter, local declaration or explicit import named `log` shadows it. |

Logger names use the validated `LoggerName` newtype. Names are dot-separated and must not be empty, start or end with `.`, or contain empty segments such as `app..loader`.

## `Logger`

| API | Returns | Description |
| --- | --- | --- |
| `logger.child(suffix: str)` | `Logger` | Return a descendant logger named `<logger.name>.<suffix>`. |
| `logger.bind(fields: Dict[str, Union[TelemetryValue, str, bool, int, float, None]])` | `Logger` | Return a logger with additional bound structured context. New fields override existing bound fields with the same key. |
| `logger.is_enabled(level: Level)` | `bool` | Return whether an event at `level` would pass the current source-level threshold. |
| `logger.trace(message: str, fields: Dict[str, Union[TelemetryValue, str, bool, int, float, None]] = {})` | `None` | Emit a trace event if enabled. |
| `logger.debug(message: str, fields: Dict[str, Union[TelemetryValue, str, bool, int, float, None]] = {})` | `None` | Emit a debug event if enabled. |
| `logger.info(message: str, fields: Dict[str, Union[TelemetryValue, str, bool, int, float, None]] = {})` | `None` | Emit an info event if enabled. |
| `logger.warning(message: str, fields: Dict[str, Union[TelemetryValue, str, bool, int, float, None]] = {})` | `None` | Emit a warning event if enabled. |
| `logger.error(message: str, fields: Dict[str, Union[TelemetryValue, str, bool, int, float, None]] = {})` | `None` | Emit an error event if enabled. |
| `logger.critical(message: str, fields: Dict[str, Union[TelemetryValue, str, bool, int, float, None]] = {})` | `None` | Emit a critical event if enabled. |

Event fields accept strings, booleans, integers, floats, `None`, and `std.telemetry.core.TelemetryValue` for nested arrays, maps, encoded bytes, or values that have already crossed a telemetry boundary. Omitting the dictionary uses the empty default. Event-specific fields override bound fields with the same key for that event.

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

`Logger`, its telemetry values and the root `log` static are declared by `std.primitives`; `std.logging` reexports those same declarations and `std.prelude` reexports the same `log` identity. The primitive implementation imports no higher standard package. A program that does not use `log` or another logging declaration does not reach the primitive logger through the prelude alone. Logging uses the host clock and ordinary native stdout/stderr delivery; it does not use a Rust backing logging module.

The logging API does not define project defaults in `loaf.toml`, `INCAN_LOG_*` environment overrides, `incan run --log-*` flags, exporters, or colorized terminal output.

## See also

- [Logging](../../how-to/logging.md)
- [`std.datetime` reference](datetime.md)
