# RFC 072: `std.logging` — logger acquisition, configuration, and structured events

- **Status:** In Progress
- **Created:** 2026-04-23
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 022 (namespaced stdlib modules and compiler handoff)
    - RFC 023 (compilable stdlib and Rust module binding)
    - RFC 033 (`ctx` typed configuration context)
    - RFC 038 (variadic positional args and keyword capture)
    - RFC 041 (first-class Rust interop authoring)
    - RFC 063 (`std.process` process spawning and command execution)
    - RFC 066 (`std.http` HTTP client surface)
    - RFC 093 (`std.telemetry` OpenTelemetry-aligned observability)
    - OpenTelemetry semantic conventions (external)
    - OpenTelemetry GenAI semantic conventions (external)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/392
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC defines `std.logging` as Incan's standard-library logging module for ordinary application and library logging. The user-facing model is explicit and Pythonic: applications configure source-level logging policy through `basic_config(...)`, code may acquire named loggers through `get_logger(...)`, and ordinary events are emitted with the ambient `log` surface through calls such as `log.info("message")` and `log.warning("message", fields={"path": path})`. The record model is OpenTelemetry LogRecord-aligned from the start, using Incan-native field names with official OpenTelemetry field spellings as aliases and descriptions, while keeping backend crates such as Rust `tracing` outside the public Incan API.

The design is deliberately event-focused. It introduces ambient `log` as a shadowable soft surface for the current module's default logger, but it does not introduce span/export APIs, handler/filter graphs, or a Rust-backed logging runtime. The runtime model is still structured enough that observability RFCs can add spans, metrics, resource identity, context propagation, OpenTelemetry-compatible export, and semantic-convention mapping without changing ordinary event call sites.

## Core model

Read this RFC as one foundation plus five mechanisms:

1. **Foundation:** Logging is a namespaced stdlib/runtime surface with an ambient `log` convenience, not a Rust interop recipe.
2. **Mechanism A:** Applications configure source-level logging policy through `std.logging.basic_config(...)`.
3. **Mechanism B:** Code emits ordinary events through ambient `log`, which behaves like the current module's `get_logger()` default when not shadowed.
4. **Mechanism C:** Logger names are hierarchical, dot-separated targets such as `app`, `app.loader`, or an inferred module name.
5. **Mechanism D:** Structured event fields and logger-bound context are first-class and must remain structured through the runtime boundary.
6. **Mechanism E:** `std.logging.LogRecord` uses the shared `std.telemetry.core` data model shape for logs/events without requiring the full `std.telemetry` provider or exporters.
7. **Mechanism F:** Host primitives are limited to ordinary `rust::std` imports where the source module needs platform data such as the system clock.

## Motivation

Logging is a stdlib surface that determines whether a language feels credible for real programs. CLIs, services, automation scripts, libraries, test helpers, and query-language surfaces all need one coherent diagnostics story. Today, the practical escape hatch is Rust interop. That proves capability, but not language design. It leaks Rust crate names, backend assumptions, and macro-shaped habits into code that should stay ordinary Incan.

The standard library should solve the common case directly. A user should not need to decide between Rust `log`, `env_logger`, `tracing`, or `tracing-subscriber` just to emit an info event from an Incan program. At the same time, a library should not seize process-wide configuration just because it wants to report useful progress. The module needs a clean split between emission and runtime policy.

The long-run destination is broader than plain text logging. Purpose-built libraries, pipeline frameworks, query systems, and application runtimes need correlation identifiers, operation boundaries, and machine-usable diagnostics. This RFC therefore standardizes structured event records now and leaves span/export semantics to follow-up RFCs that can build on the same event payload.

## Goals

- Provide a first-class `std.logging` module for ordinary logging.
- Keep ordinary call sites simple: `log.info(...)`, `log.warning(...)`, and similar methods on a `Logger`.
- Provide `get_logger(...)` for logger acquisition and `basic_config(...)` for application-level configuration.
- Support hierarchical dot-separated logger names.
- Preserve structured fields and logger-bound context as structured data, not only rendered text.
- Separate library emission from application/runtime configuration.
- Provide built-in human rendering and JSON rendering policy.
- Keep the public source-level contract backend-agnostic and implemented in Incan source.
- Align `LogRecord` with the OpenTelemetry Logs Data Model now, including official field aliases and human-readable field descriptions.
- Establish `std.telemetry.core` as the pure data-model substrate under `std.logging`, while leaving the full opt-in telemetry provider/exporter surface to `std.telemetry`.
- Leave room for span/tracing/export APIs without changing the event call-site model.

## Non-Goals

- Making `log` unshadowable or giving it span/export responsibilities.
- Introducing a logging keyword or special logging statement syntax.
- Copying Python logging wholesale, including the full handler, formatter, filter, propagation, and adapter graph.
- Exposing Rust crate names such as `tracing`, `log`, or OpenTelemetry packages in ordinary Incan source code.
- Defining span APIs, distributed tracing, or exporter configuration in this RFC.
- Claiming whole-language OpenTelemetry compliance from `std.logging` alone.
- Requiring `std.telemetry.configure(...)`, network exporters, or collector configuration for ordinary `std.logging` use.
- Vendoring or freezing OpenTelemetry semantic-convention registries inside `std.logging`.
- Making JSON the default human-facing terminal output.
- Making logging depend on `ctx`; typed configuration integration is additive.
- Treating logging as a text-only `println` replacement with no structured payload underneath.

## Guide-level explanation

### Basic application logging

Applications configure logging once near their entrypoint:

```incan
from std.logging import Level, basic_config

def main() -> None:
    basic_config(level=Level.INFO)
    log.info("Starting session")
```

Ambient `log` is a soft surface: if user code has no local `log` binding, `log.info(...)` behaves like calling
`get_logger()` for the current module and then invoking the method. A local variable, parameter, import, or explicit
`log = get_logger("...")` binding shadows the ambient surface.

`get_logger()` without a name returns a logger whose name is inferred from the current module when the compiler can
provide that metadata. When module-name inference is not available, the fallback name is `"root"`. Package and
executable identity are telemetry/provider integration concerns rather than ordinary logging defaults.

### Named loggers

Libraries and larger applications should use hierarchical names:

```incan
from std.logging import get_logger

def load_dataset(path: str) -> Dataset:
    log = get_logger("etl.loader")
    log.info("Loading dataset", fields={"path": path})
    return Dataset.open(path)
```

The hierarchy is dot-separated. A logger named `etl.loader` is a child of `etl`, and filtering policy may match either the exact logger name or an ancestor prefix.

### Structured fields

Fields are part of the event payload:

```incan
log.info("Loaded dataset rows", fields={"dataset": dataset_name, "rows": row_count})
```

Human renderers may show only the message and selected metadata. JSON output and runtime sinks must preserve the field map as structured data.

### Bound context

Repeated context should be bound once instead of repeated manually:

```incan
def handle_request(request_id: str, user_id: str, elapsed_ms: int) -> None:
    request_log = get_logger("api.request").bind({"request_id": request_id, "user_id": user_id})
    request_log.info("Accepted request")
    request_log.warning("Slow upstream response", fields={"elapsed_ms": elapsed_ms})
```

Fields passed on an individual event override bound fields with the same key for that event only.

### Library behavior

Libraries should acquire loggers and emit events, but they should not configure process-wide logging policy:

```incan
from std.logging import get_logger

pub def read_csv(path: str) -> Result[Table, CsvError]:
    log = get_logger("csv.reader")
    log.debug("Reading CSV", fields={"path": path})
    ...
```

Application entrypoints, tests, command runners, or embedding hosts own `basic_config(...)` and runtime overrides.

### Configuration boundary

The implementable baseline is source-owned configuration:

```incan
from std.logging import Level, LogFormat, basic_config

def main() -> None:
    basic_config(level=Level.DEBUG, format=LogFormat.JSON, target="stdout")
```

Project defaults, environment overrides, and CLI flags are out of scope until Incan has a source-owned host
configuration boundary that can feed stdlib state without introducing a Rust logging helper module.

## Reference-level explanation

### Module scope

`std.logging` must provide:

- `Level`
- `LoggerName`
- `Logger`
- `LogFormat`
- `LogStyle`
- `ColorPolicy`
- `OutputTarget`
- `LogRecord`
- `get_logger(...)`
- `basic_config(...)`

Implementations may expose additional advanced configuration types compatibly, but the names above are the committed surface for this RFC.

### Telemetry core layering

`std.logging` sits on the pure data-model subset of `std.telemetry`, not on the full opt-in provider/exporter stack. The shared substrate is `std.telemetry.core`: timestamp values, telemetry value types, attributes, resource identity, instrumentation scope, and trace context value types.

This layering has three rules:

- importing or using `std.logging` must not require `std.telemetry.configure(...)`;
- the default logging path must remain local and simple when no telemetry provider is configured;
- when a `std.telemetry` provider is configured, `std.logging` records must be enrichable with resource, instrumentation scope, trace context, baggage-derived attributes where policy permits, and exporter routing without changing ordinary logging call sites.

In other words, `std.logging` is the simple log/event facade, while `std.telemetry` is the explicit opt-in full observability layer.

### Levels

The standard level set is:

- `TRACE`
- `DEBUG`
- `INFO`
- `WARN`
- `ERROR`
- `FATAL`

`WARN` and `FATAL` are canonical because they match OpenTelemetry's normalized severity range names. `WARNING` aliases
`WARN`, and `CRITICAL` aliases `FATAL` for source readability and Python-style familiarity.

The ordering is `TRACE < DEBUG < INFO < WARN < ERROR < FATAL`. A runtime threshold includes events at that level or above.

### Logger acquisition

`get_logger(name: Option[str] = None) -> Logger` returns a logger value.

The name contract is:

- logger names are represented by the validated `LoggerName` newtype;
- explicit names must be non-empty dot-separated identifiers;
- each segment must be non-empty;
- the implementation should reject names with empty segments such as `"app..db"`;
- names are case-sensitive;
- `get_logger()` with no name uses compiler/runtime module identity when available, then `"root"`.

Calling `get_logger(...)` repeatedly with the same name must produce loggers with the same effective identity. The API does not require object identity or global mutable logger objects to be observable.

`log` is an ambient soft surface for the current module's default logger:

```incan
log.info("ready")
```

It is equivalent to using a default logger for the current module. It must be shadowable by ordinary source bindings:

```incan
def load(path: str) -> None:
    log = get_logger("etl.loader")
    log.info("ready")
```

An implementation must not freeze ambient `log` to the implementation module `std.logging`, because that would make the
shortcut less correct than the explicit `get_logger()` form.

### Logger API

`Logger` must expose:

- `name: LoggerName`
- `child(suffix: str) -> Logger`
- `bind(fields: Dict[str, LogValue]) -> Logger`
- `is_enabled(level: Level) -> bool`
- `trace(message: str, fields: Dict[str, LogValue] = {}) -> None`
- `debug(message: str, fields: Dict[str, LogValue] = {}) -> None`
- `info(message: str, fields: Dict[str, LogValue] = {}) -> None`
- `warning(message: str, fields: Dict[str, LogValue] = {}) -> None`
- `error(message: str, fields: Dict[str, LogValue] = {}) -> None`
- `critical(message: str, fields: Dict[str, LogValue] = {}) -> None`

`child("loader")` on logger `etl` returns a logger named `etl.loader`. Calling `child(...)` with a suffix containing `.` is allowed and appends the suffix as a dotted descendant path after validating that it has no empty segments.

`bind(...)` returns a logger with additional bound context. It must not mutate the original logger in place.

### Structured field values

`LogValue` is a conceptual structured value accepted by the runtime boundary. The implementation must support at least:

- `None`
- `bool`
- signed and unsigned integer values representable by the runtime payload
- floating-point values
- `str`
- `bytes` rendered or encoded according to output format policy
- lists and maps containing supported `LogValue` values

Model values may be accepted if the implementation has a stable structured representation for them. If a value cannot be preserved structurally, the runtime must either reject it with a typed logging configuration or emission error before rendering, or preserve a clearly marked debug string fallback. It must not silently pretend lossy stringification is the same as structured preservation in JSON output.

Field keys must be strings. Field keys should be valid identifier-like names for maximum backend portability, but this RFC only requires that keys are non-empty and do not contain control characters.

Reserved runtime keys include `timestamp`, `level`, `message`, `logger`, `target`, `module`, `file`, `line`, and `thread`. User-provided fields with reserved names are allowed only inside the user field map and must not overwrite the top-level runtime metadata fields.

Field keys may use dotted semantic-convention names such as `http.request.method`, `db.system.name`,
`gen_ai.request.model`, `mcp.method.name`, or provider-specific keys. `std.logging` must preserve those keys as
ordinary structured fields. It must not normalize, split, or reserve external telemetry namespaces in the base logging
API.

### Event records

Each emitted event produces a `LogRecord` internally. `LogRecord` is the Incan source-level projection of the OpenTelemetry Logs Data Model. It must use idiomatic Incan field names while preserving the official OpenTelemetry field spellings as field aliases:

```incan
pub model LogRecord:
    timestamp [alias="Timestamp", description="Time when the event occurred."]: Timestamp
    observed_timestamp [alias="ObservedTimestamp", description="Time when telemetry observed the event."]: Option[Timestamp] = None
    trace_id [alias="TraceId", description="Request trace identifier when the event is span-correlated."]: Option[TraceId] = None
    span_id [alias="SpanId", description="Span identifier when the event is span-correlated."]: Option[SpanId] = None
    trace_flags [alias="TraceFlags", description="W3C trace flags for the correlated span."]: Option[TraceFlags] = None
    severity_text [alias="SeverityText", description="OpenTelemetry severity text, such as INFO or WARN."]: str
    severity_number [alias="SeverityNumber", description="OpenTelemetry normalized severity number."]: int
    body [alias="Body", description="Human or structured event body."]: TelemetryValue
    resource [alias="Resource", description="Entity that produced this telemetry."]: Resource
    instrumentation_scope [alias="InstrumentationScope", description="Logical scope that emitted this record."]: InstrumentationScope
    attributes [alias="Attributes", description="Additional structured attributes for this event."]: Attributes
    event_name [alias="EventName", description="Optional event class or type name."]: Option[str] = None
```

`timestamp`, `severity_text`, `severity_number`, `body`, `resource`, `instrumentation_scope`, and `attributes` are the required Incan-side fields for records emitted by `std.logging`. Trace context fields remain optional unless a tracing provider supplies an active span context. `observed_timestamp` remains optional unless a collector/exporter boundary records observation time. `event_name` remains optional for ordinary log messages and should be set when the record represents a named event class.

`Level` maps to OpenTelemetry severity as follows:

- `TRACE` -> `severity_text="TRACE"`, `severity_number=1`
- `DEBUG` -> `severity_text="DEBUG"`, `severity_number=5`
- `INFO` -> `severity_text="INFO"`, `severity_number=9`
- `WARN` -> `severity_text="WARN"`, `severity_number=13`
- `ERROR` -> `severity_text="ERROR"`, `severity_number=17`
- `FATAL` -> `severity_text="FATAL"`, `severity_number=21`

`WARNING` and `CRITICAL` are aliases for the canonical `WARN` and `FATAL` variants, so JSON output can use
`level.value()` directly for `SeverityText`. Human renderers may still display `WARNING` and `CRITICAL` if that is the
configured Incan presentation policy.

Human output is a projection of this record. JSON output must preserve the record as structured data. OTel-oriented JSON output should honor the field aliases above so downstream tools see the official OpenTelemetry field spellings, while human-oriented projections may use Incan names or compact labels.

### Configuration

`basic_config(...)` configures the source-level logging policy for the current generated program. It is an application entrypoint API, not a library API.

The committed configuration knobs are:

- `level: Level`
- `format: LogFormat`
- `style: LogStyle`
- `color: ColorPolicy`
- `target: OutputTarget`

`LogFormat` must include:

- `HUMAN`
- `JSON`

`LogStyle` applies only when `format` is `HUMAN` and must include:

- `MINIMAL`
- `SHORT`
- `COMPLETE`
- `VERBOSE`

`ColorPolicy` must include:

- `AUTO`
- `ALWAYS`
- `NEVER`

`OutputTarget` is a validated newtype over `str`. It accepts at least `"stderr"` and `"stdout"` and owns routing emitted
lines to the selected standard stream. `basic_config(...)` may continue to accept a string argument at the call boundary,
but the stored configuration should use the nominal target type instead of carrying an unchecked string.

Calling `basic_config(...)` more than once is deterministic. A second call replaces the previous source-level policy.

### Project and runtime configuration

`incan.toml`, CLI flags, and environment overrides are not part of the implementable baseline in this RFC. They should
be specified once Incan has a source-owned configuration/import boundary for host-provided settings.

A project configuration surface may look like:

```toml
[logging]
level = "warning"
format = "human"
style = "short"
color = "auto"
target = "stderr"
```

CLI and environment values should use the same semantic vocabulary as `basic_config(...)`. String parsing should be case-insensitive for level, format, style, and color names.

The CLI/environment surface may include:

- `--log-level`
- `--log-format`
- `--log-style`
- `--log-color`
- `INCAN_LOG_LEVEL`
- `INCAN_LOG_FORMAT`
- `INCAN_LOG_STYLE`
- `INCAN_LOG_COLOR`

The implementation may add `--quiet` or `--verbose` convenience flags, but those flags must be specified as translations into the same logging policy model rather than as separate hidden behavior.

### Human rendering

Human rendering must be readable in terminals. `short` is the default style.

Normative style expectations:

- `minimal` omits timestamps and renders level plus message.
- `short` renders a compact time-of-day timestamp.
- `complete` renders a full datetime timestamp.
- `verbose` may render additional metadata and fields, but it must keep the first line message-first and readable.
- When a record carries active span context, human renderers should be able to expose that hierarchy with lightweight
  span guides rather than long repeated logger prefixes.

Illustrative shapes:

```text
[INFO] starting query
```

```text
21:18:04 [INFO] starting query
```

```text
2026-04-23T21:18:04.221Z [INFO] starting query
```

```text
2026-04-23T21:18:04.221Z [INFO] starting query
  logger=query.engine module=query.run fields={dataset="sales", rows=42}
```

Illustrative span-correlated `short` output:

```text
21:18:04 [INFO] starting query
21:18:04 └─ [INFO] lowering plan
21:18:04 └─ [INFO] emitting sql
21:18:04 └─ [INFO] query complete
```

Illustrative span-correlated `verbose` output:

```text
2026-04-23T21:18:04.221Z [INFO] starting query
  logger=query.engine trace=7f9c span=01 fields={dataset="sales"}
2026-04-23T21:18:04.228Z └─ [INFO] lowering plan
  logger=query.engine.lowering trace=7f9c span=02 parent=01
```

The exact spacing and glyph topology are renderer policy. The level, message, selected metadata, and span relationship
must be visible without requiring a JSON consumer. Span guides are a human projection of trace/span context supplied by
the telemetry layer; this RFC does not make `std.logging` responsible for starting or ending spans.

### JSON rendering

JSON output is selected with `format=LogFormat.JSON`.

Each event should render as one JSON object per line. The object must separate named OpenTelemetry top-level fields from user/event attributes. A representative OTel-oriented shape is:

```json
{"Timestamp":"2026-04-23T21:18:04.221Z","SeverityText":"INFO","SeverityNumber":9,"Body":{"Type":"string","StringValue":"starting query","BoolValue":null,"IntValue":null,"FloatValue":null,"BytesValue":null,"ArrayValue":[],"MapValue":{}},"Resource":{"Attributes":{}},"InstrumentationScope":{"Name":"query.engine","Version":null,"SchemaUrl":null},"Attributes":{"dataset":{"Type":"string","StringValue":"sales","BoolValue":null,"IntValue":null,"FloatValue":null,"BytesValue":null,"ArrayValue":[],"MapValue":{}},"rows":{"Type":"int","StringValue":null,"BoolValue":null,"IntValue":42,"FloatValue":null,"BytesValue":null,"ArrayValue":[],"MapValue":{}}}}
```

JSON output must be colorless regardless of `ColorPolicy`.

### Filtering

The minimum filter is a global threshold level. Implementations may support per-logger filters through project, CLI, or environment policy, but this RFC does not require a complete filter expression language.

If per-logger filters are implemented, matching must respect hierarchical names. A policy for `etl` applies to `etl.loader` unless a more specific `etl.loader` policy overrides it.

### Library and application boundary

Libraries may call `get_logger(...)`, `Logger.bind(...)`, and event methods. Libraries must not call `basic_config(...)` as part of import-time or normal helper execution.

Application entrypoints, command runners, tests, and embedding hosts may call `basic_config(...)`. Documentation should teach this boundary explicitly.

### Source implementation boundary

The reference implementation is authored in `crates/incan_stdlib/stdlib/logging.incn`. It may import from `rust::std`
for platform primitives such as `SystemTime`, but it must not use a Rust backing module or `@rust.extern` logging
helpers.

The source implementation must preserve:

- logger name
- event level
- message
- structured field values
- bound context

Logger-name source metadata uses module identity where the compiler can provide it. File and line metadata are outside
this RFC and belong with the broader telemetry source-location policy.

### OpenTelemetry compatibility

This RFC does not standardize the full OpenTelemetry model. `std.telemetry` owns provider configuration, tracing decorators, explicit span handles, metric instruments, baggage/context propagation, OpenTelemetry export, semantic-convention helpers, and stdlib instrumentation policy. The event payload defined here must remain usable as the log/event component inside that observability model.

The absence of span, metric, resource, and propagation APIs is intentional. It keeps RFC 072 implementable as a
standard-library/runtime surface without adding parser support or a new ambient language binding.

Incan should become OpenTelemetry-compatible as a language and stdlib ecosystem, not merely GenAI-compatible. That means observability RFCs should account for OpenTelemetry's traces, metrics, logs/events, resources, profiles, and context propagation concepts, and should use semantic conventions where Incan stdlib surfaces correspond to common operations. `std.logging` contributes by adopting the OpenTelemetry log record shape now and preserving structured attributes and a stable named-fields/attributes boundary.

The preferred tracing surface is decorator-first because decorators can own span lifetime for whole functions:

```incan
from std.telemetry import trace

@trace
def submit(req: Request) -> Result[Response, Error]:
    log.info("accepted", fields={"order.id": req.order_id})
    return charge(req)
```

The default span name should be derived from the fully qualified Incan symbol, such as `checkout.submit`, so common tracing does not require repeating the function name as a string. The decorator may accept an explicit override and an attribute factory when needed:

```incan
@trace("checkout.payment.authorize", attributes=payment_attrs)
def authorize(req: Request) -> Result[Auth, Error]:
    return gateway.authorize(req)
```

Manual span handles should remain available for non-function scopes, but they are an escape hatch until Incan has a block-level scope construct:

```incan
span = telemetry.start_span("cache.lookup", attributes={"cache.key": key})
result = cache.get(key)
span.end()
```

Scoped span blocks are owned by the telemetry/context-manager RFCs rather than by `std.logging`.

OpenTelemetry's GenAI semantic conventions define GenAI, MCP, and provider-specific telemetry vocabulary outside
Incan's standard library. They are one convention family under the broader OpenTelemetry compatibility goal, not the
whole target. RFC 072 should align with that direction by preserving arbitrary structured field names, but it should not
copy any convention registry into `std.logging`. `std.telemetry` should define opt-in mapping profiles for OpenTelemetry
logs/events/spans/metrics.

## Design details

### Why ambient `log` lowers into `std.logging`

Ambient `log` is worth the language commitment because logging is part of the everyday program surface. Requiring every
module to spell `log = get_logger()` adds ceremony without adding useful information in the common case.

The important constraint is that ambient `log` is soft and shadowable. It should behave like an unresolved identifier
fallback, not like an unshadowable keyword. This keeps explicit logger values available for libraries, tests, and code
that wants a deliberate hierarchy:

```incan
def load(path: str) -> None:
    log = get_logger("etl.loader")
    log.info("ready")
```

The ambient form lowers into `std.logging.get_logger(<current module>)` and then uses the same source-defined `Logger`
implementation as explicit logger values.

### Why not copy Python logging wholesale

Python's logging module has useful concepts, especially named loggers and application-owned configuration, but its handler, formatter, filter, propagation, and adapter graph is too much surface for this RFC. Incan should preserve the parts that matter for ordinary code while avoiding a compatibility-shaped clone that would be hard to implement and harder to teach.

### Why `fields={...}` is the structured field syntax

`fields={...}` keeps the event signature implementable and explicit. It avoids making arbitrary keyword capture part of every logging method's public contract. A keyword-style field layer can be added compatibly, but `fields={...}` remains the stable baseline.

### Why bound context returns a new logger

Returning a new logger from `bind(...)` keeps context local and composable. It avoids hidden mutation on shared logger values and makes it easier to pass request- or job-scoped loggers through code without changing global policy.

### Why libraries must not configure logging

Configuration is a process-level policy decision. If libraries call `basic_config(...)`, importing a helper can unexpectedly change application output, filtering, or JSON ingestion. Libraries should emit events and let applications or hosts decide how those events are rendered or exported.

### Why JSON is a format, not a style

JSON is for machine consumers, collectors, and ingestion pipelines. Human styles are terminal presentation policy. Keeping `format` and `style` separate avoids treating JSON as another terminal skin and keeps color/template settings from leaking into machine output.

### Why OpenTelemetry alignment belongs in this RFC

OpenTelemetry's Logs Data Model already separates named top-level fields from attribute collections. That is exactly the distinction `std.logging` needs: timestamp, severity, body, resource, scope, and trace context are not just arbitrary user fields, while application-specific data remains in attributes. Aligning `LogRecord` prevents Incan from shipping a local record shape that needs a breaking migration.

Incan should still use Incan names in source. Field aliases provide the bridge: source code reads as `severity_text` and `instrumentation_scope`, while wire-oriented serialization can use `SeverityText` and `InstrumentationScope`.

### Why full telemetry is not part of this RFC

Span semantics need more than a helper method. They raise questions about scoped lifetime syntax, async/task propagation, context inheritance, duration recording, error association, and exporter mapping. Metrics add aggregation, units, temporality, views, and cardinality concerns. Exporters add endpoint configuration, batching, retries, shutdown flushing, and host/network I/O. Those are real requirements, but adding them here would make this logging RFC too broad. The correct move is to adopt the OpenTelemetry log record model and design the full `std.telemetry` provider/exporter surface in its own RFC.

## Alternatives considered

1. **Ambient built-in `log`**
   - Rejected for this RFC because it requires new language/tooling behavior and bypasses the existing namespaced stdlib model.

2. **Expose Rust `tracing` or `log` directly**
   - Rejected because ordinary Incan code should not need Rust crate names, macros, subscribers, or backend-specific error behavior.

3. **Only provide `println`-style text logging**
   - Rejected because fields, logger names, and machine output are core requirements, not optional polish.

4. **Copy Python logging's full handler/filter graph**
   - Rejected because it is too large for this Incan stdlib contract and would overfit Python runtime assumptions.

5. **Start with spans as the primary API**
   - Rejected because spans deserve their own RFC and should build on a stable event record model.

6. **Bake OpenTelemetry attribute types directly into `std.logging`**
   - Rejected because OpenTelemetry semantic conventions evolve independently and cover more than ordinary log events.
     The logging API should preserve namespaced structured fields now and let dedicated observability/export RFCs own
     the mapping contract.

## Drawbacks

- Ambient `log` is a small language/tooling commitment and needs shadowing, formatting, and LSP support.
- The runtime must preserve structured field values, which is more work than plain text printing.
- Keeping spans out of this RFC means some tracing use cases still need a follow-up design.
- `basic_config(...)` plus project/CLI/env policy requires clear precedence tests to avoid confusing behavior.
- The `LogValue` boundary needs careful implementation to avoid lossy structured output.

## Layers affected

- **Stdlib registry:** add `std.logging` as a registered source stdlib namespace.
- **Language surface:** add ambient, shadowable `log` resolution for ordinary event calls.
- **Stdlib source:** add the `.incn` implementation for imports, typechecking, logger values, bound context, OTel-aligned event records, filtering, and rendering.
- **Telemetry core stdlib source:** add the pure data-model subset needed by `std.logging`, including telemetry values, attributes, resource identity, instrumentation scope, and trace context value types.
- **Runtime (`incan_stdlib`):** no logging-specific Rust module; existing generated-code storage is reused and stream output uses ordinary `rust::std` imports from source.
- **Emission:** support source stdlib default arguments, static-field reads, and static-field mutation well enough for `std.logging` to dogfood them.
- **LSP / tooling:** provide import completions, hover docs, and diagnostics for the new stdlib module.
- **Docs / examples:** document ordinary use, library/application boundaries, configuration, structured fields, and JSON output.

## Delivery Plan

### Phase 1: Stdlib surface and registry

- Add `std.logging` to the stdlib namespace registry so imports, stub lookup, and LSP completion recognize the module.
- Add the source-defined `std.telemetry.core` data-model subset that `std.logging` depends on.
- Add `std.logging` source declarations and implementations for `Level`, `LoggerName`, `Logger`, `LogFormat`, `LogStyle`, `ColorPolicy`, `OutputTarget`, OTel-aligned `LogRecord`, `get_logger(...)`, and `basic_config(...)`.
- Add ambient `log` lowering as a shadowable default logger surface.
- Dogfood Incan for the public stdlib contract; no logging-specific `@rust.extern` implementation is allowed.

### Phase 2: Source behavior and generated-code support

- Implement logger values, bound context, OTel-aligned structured event records, threshold filtering, human rendering, and JSON rendering in Incan source.
- Map Incan `Level` values to OpenTelemetry `SeverityText` and `SeverityNumber`.
- Render OTel-oriented JSON with the official field aliases while keeping human rendering concise.
- Use `std.datetime` for timestamps so logging follows the stdlib time surface rather than importing Rust time directly.
- Fix generated-code support for source stdlib default arguments, static-field reads, and static-field mutation as needed.

### Phase 3: Deferred host configuration policy

- Keep `basic_config(...)` as the implemented source-level configuration mechanism.
- Defer `incan.toml`, CLI, and environment handling until a source-owned host configuration boundary exists.
- Route `"stdout"` and `"stderr"` through ordinary source-level `rust::std::io` imports.

### Phase 4: Tests and docs

- Add focused tests for logger acquisition, level filtering, structured fields, bound context, source configuration, human rendering, JSON rendering, and library/application behavior.
- Add user-facing stdlib documentation and examples outside the RFC.
- Add release notes for the active development release.

## Progress Checklist

### Spec / design

- [x] Move RFC 072 to an implementation-ready `std.logging` contract.
- [x] Record the dogfooding constraint: public stdlib surface and logging behavior are Incan-defined, with no logging-specific `rust.extern` implementation.
- [x] Record OpenTelemetry LogRecord alignment as a requirement for the committed `LogRecord` model.
- [x] Record ambient `log` as the ordinary default event surface.
- [x] Keep the RFC checklist synchronized with implementation progress.

### Stdlib surface and registry

- [x] Register `std.logging` in the stdlib namespace registry.
- [x] Add source-defined `std.logging` declarations.
- [x] Add source-defined `std.telemetry.core` declarations needed by the logging record model.
- [x] Confirm imports such as `from std.logging import Level, basic_config, get_logger` typecheck.
- [x] Confirm ambient `log.info(...)` typechecks and lowers to the current module's default logger.
- [x] Confirm the public `Logger` surface is source-defined rather than a Rust-only type shell where possible.

### Runtime and generated-code handoff

- [x] Implement logger values and logger-name validation.
- [x] Implement `Logger.child(...)`, `Logger.bind(...)`, and level-specific event methods.
- [x] Implement the OTel-aligned `LogRecord` model with official field aliases and descriptions.
- [x] Map `Level` to OpenTelemetry `SeverityText` and `SeverityNumber`.
- [x] Preserve structured event fields and bound context in runtime records.
- [x] Implement human rendering for `minimal`, `short`, `complete`, and `verbose`.
- [x] Implement OTel-oriented JSON-lines rendering with official field aliases and without color control sequences.
- [x] Preserve source metadata where available.
- [x] Keep Rust backend crate choices out of the Incan source-level API.
- [x] Use `std.datetime` for timestamps and avoid logging-specific Rust host primitives.

### Configuration policy

- [x] Implement `basic_config(...)` threshold, format, style, color, and target validation in source.
- [ ] Apply color policy in human rendering.
- [ ] Load project defaults from `[logging]` in `incan.toml`.
- [ ] Add runtime overrides through CLI flags.
- [ ] Add runtime overrides through `INCAN_LOG_*` environment variables.
- [ ] Test and document the host/source precedence order.

### Tests

- [x] Test logger acquisition and hierarchical child names.
- [x] Test level filtering and `is_enabled(...)`.
- [x] Test structured fields and field override behavior.
- [x] Test bound logger context without mutating the original logger.
- [x] Test library emission without root configuration ownership.
- [x] Test human renderer styles.
- [x] Test OTel-aligned JSON renderer output.
- [x] Test source-level `basic_config(...)`.
- [ ] Test configuration precedence.

### Docs / release notes

- [x] Add user-facing `std.logging` reference documentation.
- [x] Add an example that shows application-owned configuration and library-owned emission.
- [x] Add release notes for RFC 072.
- [x] Run `mkdocs build --strict`.

## Design Decisions

- `std.logging` is a namespaced stdlib module with ambient, shadowable `log` as the ordinary event surface.
- `std.logging` sits on the pure `std.telemetry.core` data model, while full `std.telemetry` provider/exporter behavior remains explicit and opt-in.
- `get_logger(...)` and `basic_config(...)` are in scope for this RFC.
- Logger names are hierarchical, dot-separated, and case-sensitive.
- `WARN` is the canonical warning level spelling, with `WARNING` as an alias.
- Structured event fields use `fields={...}` in the committed API.
- `Logger.bind(...)` returns a new logger and event fields override bound fields for that event.
- Libraries emit logs but must not own root logging configuration.
- `short` is the default human style.
- JSON is a `LogFormat`, not a `LogStyle`, and OTel-oriented JSON uses the official `LogRecord` field aliases.
- Color policy is human-renderer-only and never affects JSON output.
- Colorized terminal behavior remains tied to a source-owned CLI/terminal capability surface instead of a Rust
  logging helper.
- The filter contract requires a global threshold; per-logger filtering may be added compatibly.
- The implemented reference surface is source Incan and may only use ordinary `rust::std` imports for host primitives.
- Rust `tracing` integration is deferred until it can be introduced without replacing the source-owned logging surface.
- Span, context propagation, and external export APIs are deferred to follow-up RFCs.
- OpenTelemetry compatibility is an Incan-wide observability goal. RFC 072 adopts the OpenTelemetry log record model,
  while full provider/exporter behavior, semantic convention helpers, tracing decorators, baggage/context propagation,
  and metrics are deferred to dedicated observability RFCs.
- RFC 093 owns the full `std.telemetry` provider/exporter direction; RFC 094 and RFC 095 record the context-manager and
  `span` vocabulary foundations needed for ambient but controlled spans.
