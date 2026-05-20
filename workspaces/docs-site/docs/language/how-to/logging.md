# Logging

Use `std.logging` to emit structured application events without adding a separate logging runtime. Application entrypoints own configuration; libraries acquire loggers and emit events without changing process policy.

## Configure an application entrypoint

Call `basic_config(...)` near the start of the entrypoint:

```incan
from std.logging import Level, basic_config

def main() -> None:
    basic_config(level=Level.INFO, target="stdout")
    log.info("started", fields={"component": "worker"})
```

Use `target="stdout"` when another process consumes JSON lines or event text. Use the default `target="stderr"` when logs should stay separate from application output.

## Acquire named loggers

Use ambient `log` for the current module's default logger. Use `get_logger(...)` when a stable explicit name is part of the logging contract:

```incan
from std.logging import get_logger

def load(path: str) -> None:
    log = get_logger("etl.loader")
    log.info("loading", fields={"path": path})
```

Use `child(...)` for hierarchical names:

```incan
from std.logging import get_logger

def load(path: str) -> None:
    root = get_logger("etl")
    loader = root.child("loader")
    loader.info("loading", fields={"path": path})
```

Logger names are dot-separated and must not be empty, start or end with `.`, or contain empty segments such as `app..loader`.

## Add structured fields

Pass event-specific fields in the `fields` dictionary:

```incan
log.info("loaded rows", fields={"dataset": "users", "rows": 42, "cached": true})
```

Fields accept primitive structured values and `std.telemetry.core.TelemetryValue`. Dotted semantic keys such as `http.request.method`, `db.system.name`, `gen_ai.request.model`, or `mcp.method.name` are preserved as ordinary field keys.

## Bind repeated context

Use `bind(...)` when several events share context:

```incan
from std.logging import get_logger

def handle(request_id: str, elapsed_ms: int) -> None:
    request_log = get_logger("api.request").bind({"request_id": request_id})
    request_log.info("accepted")
    request_log.warning("slow upstream", fields={"elapsed_ms": elapsed_ms})
```

Event fields override bound fields with the same key for that event.

## Choose output format and style

Use `LogFormat.JSON` for one JSON object per emitted event and `LogFormat.HUMAN` for terminal-oriented text:

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

`LogStyle.MINIMAL`, `LogStyle.SHORT`, `LogStyle.COMPLETE`, and `LogStyle.VERBOSE` affect human output only. JSON output keeps a stable record-shaped representation.

## Keep library code passive

Libraries should acquire named loggers and emit events, but they should not call `basic_config(...)`. That keeps application policy, output target, format, and level threshold under the entrypoint's control.

## See also

- [`std.logging` reference](../reference/stdlib/logging.md)
- [`std.datetime` reference](../reference/stdlib/datetime.md)
