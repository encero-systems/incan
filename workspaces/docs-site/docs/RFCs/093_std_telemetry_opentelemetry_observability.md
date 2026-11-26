# RFC 093: `std.telemetry` — OpenTelemetry-aligned observability

- **Status:** Planned
- **Created:** 2026-05-11
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 021 (model field metadata and aliases)
    - RFC 033 (`ctx` typed configuration context)
    - RFC 036 (user-defined decorators)
    - RFC 052 (module static storage)
    - RFC 055 (`std.fs`)
    - RFC 063 (`std.process` process spawning and command execution)
    - RFC 066 (`std.http` HTTP client surface)
    - RFC 072 (`std.logging`)
    - RFC 080 (declared AI assets)
    - RFC 084 (RHS partial callable presets)
    - RFC 089 (`std.environ`)
    - RFC 090 (typed CLI framework)
    - RFC 094 (context managers)
    - RFC 095 (`span` vocabulary blocks)
    - OpenTelemetry specification and semantic conventions (external)
- **Issue:** [#559](https://github.com/encero-systems/incan/issues/559)
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC defines `std.telemetry` as Incan's opt-in, OpenTelemetry-aligned observability surface. `std.logging` remains the simple logging facade, but it sits on `std.telemetry.core` data-model types so logs are OpenTelemetry-shaped from the start. Applications that want full observability explicitly call `std.telemetry.configure(...)` to enable resource identity, exporters, tracing decorators, baggage/context propagation, semantic-convention helpers, stdlib instrumentation policy, and metrics. The goal is controlled ambient telemetry: nothing exports implicitly, but once an application opts in, ordinary Incan stdlib and application code can emit correlated logs, spans, and metrics without Rust crate leakage or stringly ad hoc conventions.

## Core model

Read this RFC as eight foundations:

1. **Telemetry is opt-in:** importing `std.telemetry` or `std.logging` must not export data or start background network work; an application must explicitly configure telemetry.
2. **Core data types are always safe:** `std.telemetry.core` provides pure value types such as timestamps, attributes, resources, scopes, trace identifiers, span identifiers, baggage, and telemetry values. These types may be used by `std.logging` without enabling exporters.
3. **Logging is the simple facade:** `std.logging` remains the right API for ordinary logs, but its records are OpenTelemetry LogRecord-aligned and can be enriched by a configured telemetry provider.
4. **Provider configuration is explicit:** `std.telemetry.configure(...)` installs process-level telemetry policy, including resource identity, exporters, propagation, sampling, and stdlib instrumentation.
5. **Tracing is decorator-first:** function spans should use `@telemetry.trace` with an ambient default span name derived from the fully qualified Incan symbol. Explicit names and attribute factories are available when needed.
6. **Baggage and context are values:** context propagation is explicit at API boundaries, and baggage is a distinct key-value carrier rather than a disguised log attribute map.
7. **Semantic conventions are generated helpers:** OpenTelemetry semantic conventions should be exposed through `std.telemetry.semconv` helpers and constants, not copied as unstructured strings into every stdlib module.
8. **Metrics are part of the north star:** `std.telemetry.metrics` belongs in the observability surface. Instruments declare their measurement type, unit and permitted labels. Deployment policy selects aggregation and limits the number of distinct label combinations.

## Motivation

Incan can do better than treating observability as an afterthought bolted onto individual libraries. OpenTelemetry gives the ecosystem a shared data model for logs, traces, metrics, baggage, resources, instrumentation scope, and semantic conventions. If Incan aligns early, standard-library modules can produce useful telemetry with consistent attributes, users can add exporters without rewriting application code, and generated or checked API metadata can surface observability behavior cleanly.

The current danger is fragmentation. `std.logging` could invent one record shape, `std.http` instrumentation could invent another attribute scheme, AI or MCP surfaces could copy provider-specific names by hand, and applications would still need external Rust interop to get serious telemetry into collectors. That would waste Incan's advantage: a typed language and stdlib can make observability predictable, inspectable, and explicit without requiring users to memorize Rust `tracing`, OpenTelemetry SDK setup, or exporter crate details.

The desired end state is controlled ambient observability. A small script can use `std.logging` and print local logs. A production service can call `std.telemetry.configure(...)` once and have logs, spans, baggage, resource attributes, semantic conventions, and stdlib instrumentation flow through the same provider. The key is that ambient behavior is activated only by explicit application configuration.

## Goals

- Define `std.telemetry` as the opt-in observability module for Incan.
- Define `std.telemetry.core` as the pure data-model layer shared by `std.logging` and telemetry APIs.
- Align Incan telemetry records with OpenTelemetry concepts without exposing Rust crate names in ordinary Incan source.
- Support explicit application configuration through `std.telemetry.configure(...)`.
- Keep `std.logging` simple while allowing configured telemetry to enrich and export its records.
- Define a decorator-first tracing API with ambient default span names derived from fully qualified Incan symbols.
- Define context and baggage APIs that avoid the reserved `ctx` spelling and keep baggage distinct from attributes.
- Define `std.telemetry.semconv` as the home for semantic-convention constants and typed helper functions.
- Define a stdlib instrumentation policy model so modules such as `std.http`, `std.process`, `std.fs`, `std.testing`, and AI/MCP surfaces can emit consistent telemetry when enabled.
- Support typed counters and histograms with explicit units, checked labels and bounded aggregation.

## Non-Goals

- Making telemetry export implicit merely because a module is imported.
- Replacing `std.logging` with `std.telemetry`; simple logs should remain simple.
- Exposing Rust `tracing`, OpenTelemetry SDK crates, subscribers, collectors, or exporter implementation details in ordinary Incan source.
- Selecting an exporter-specific metrics backend or prescribing one bucket layout for all deployments.
- Adding a `with` statement in this RFC.
- Treating baggage as ordinary event attributes.
- Vendoring a frozen copy of all OpenTelemetry semantic conventions into handwritten stdlib source.
- Guaranteeing that every OpenTelemetry exporter, processor, sampler, or semantic convention is supported by the standard library.
- Making application libraries call `std.telemetry.configure(...)`; configuration is an application or host concern.

## Guide-level explanation

### Simple logging remains simple

Users who only need ordinary logs should continue to use `std.logging`:

```incan
from std.logging import Level, basic_config, get_logger

def main() -> None:
    basic_config(level=Level.INFO)
    log = get_logger("app")
    log.info("started", fields={"component": "worker"})
```

This produces local logging behavior and does not require exporters, collector endpoints, or telemetry configuration.

### Full telemetry is explicit

Applications that want OpenTelemetry-compatible observability configure it once near the entrypoint:

```incan
import std.telemetry as telemetry

def main() -> Result[None, telemetry.ConfigurationError]:
    telemetry.configure(
        service="checkout-api",
        version="1.0.0",
        environment="production",
        export=telemetry.otlp(endpoint="http://collector:4318"),
        instrument=telemetry.instrument.stdlib(),
    )?
    return Ok(None)
```

After configuration, logs emitted through `std.logging` can be enriched with the configured resource, instrumentation scope, and active trace context. Stdlib modules that are enabled by instrumentation policy may emit spans, events, or attributes through the same provider.

### Decorator-first tracing

Tracing should be ergonomic for the common function-span case:

```incan
import std.telemetry as telemetry

@telemetry.trace
def submit(req: Request) -> Result[Response, Error]:
    log.info("accepted", fields={"order.id": req.order_id})
    return charge(req)
```

If this function is declared in the `orders` module of the `checkout` Loaf, its default span name is `checkout.orders.submit`. Users should not have to repeat the function name as a string for ordinary tracing.

Users can override the span name and provide an attribute factory when a domain-specific operation name is clearer:

```incan
def submit_attrs(req: Request) -> telemetry.Attributes:
    return telemetry.semconv.http.server_request(method="POST", route="/checkout") | {"order.id": req.order_id}

@telemetry.trace("checkout.payment.authorize", attributes=submit_attrs)
def authorize(req: Request) -> Result[Auth, Error]:
    return gateway.authorize(req)
```

Because telemetry decorators and semantic-convention helpers are ordinary callables, RFC 084 partials can package reusable configuration without hiding runtime-dependent attributes:

```incan
checkout_request = partial telemetry.semconv.http.server_request(method="POST", route="/checkout")

def submit_attrs(req: Request) -> telemetry.Attributes:
    return checkout_request() | {"order.id": req.order_id}

trace_checkout_authorize = partial telemetry.trace(name="checkout.payment.authorize")

@trace_checkout_authorize(attributes=submit_attrs)
def authorize(req: Request) -> Result[Auth, Error]:
    return gateway.authorize(req)
```

Telemetry APIs should therefore stay partial-friendly: prefer named parameters, expose semantic-convention helpers as plain functions that return `Attributes`, and allow configured decorator callables to be named and reused. Partials are not a separate telemetry abstraction; they are the language-level way to preset ordinary callable surfaces. Top-level partials still follow RFC 084's declaration-safe preset rules, so dynamic values and function references belong in local partials, wrapper functions, or decorator application arguments rather than top-level partial presets.

### Block-level spans use vocabulary syntax

RFC 095 defines `span` as a standard telemetry vocabulary block layered over RFC 094 context managers. This gives non-function operation scopes a native spelling without making general resource management telemetry-shaped:

```incan
import std.telemetry as telemetry

@telemetry.attributes("cache")
model CacheAttributes:
    key: str

def lookup(cache: dict[str, str], key: str) -> Option[str]:
    span "lookup", attributes=CacheAttributes(key=key):
        return cache.get(key)
```

Manual span handles remain available as a lower-level escape hatch, but ordinary source should prefer `@telemetry.trace` for whole functions and `span "name":` for meaningful sub-operation blocks.

### Baggage and context

Context and baggage are explicit values:

```incan
import std.telemetry as telemetry

context = telemetry.current_context().with_baggage("tenant.id", tenant_id)
telemetry.run(context, handle_request, request)
```

Baggage may propagate across process boundaries. It is not automatically copied into every log record or span attribute. Policies may choose which baggage keys become attributes.

Decorator-based baggage is also useful for request handlers:

```incan
def request_baggage(req: Request) -> telemetry.Baggage:
    return {"tenant.id": req.tenant_id}

@telemetry.baggage(request_baggage)
@telemetry.trace
def handle(req: Request) -> Response:
    return route(req)
```

### Semantic conventions

Semantic conventions should be discoverable and typed enough to avoid raw string sprawl:

```incan
import std.telemetry as telemetry
from std.telemetry import semconv

def checkout_attrs() -> telemetry.Attributes:
    return semconv.http.server_request(
        method="POST",
        route="/checkout",
        status_code=200,
    )
```

Lower-level constants remain available when a helper is too opinionated:

```incan
def checkout_attrs() -> telemetry.Attributes:
    return {semconv.http.request.method: "POST", semconv.http.route: "/checkout"}
```

### Stdlib instrumentation

The following independent application chooses its instrumentation policy at startup.

```incan
import std.telemetry as telemetry

def main() -> Result[None, telemetry.ConfigurationError]:
    telemetry.configure(
        service="checkout-api",
        export=telemetry.otlp(),
        instrument=[
            telemetry.instrument.std_http(),
            telemetry.instrument.std_process(),
            telemetry.instrument.std_fs(level=telemetry.InstrumentationLevel.ERRORS),
        ],
    )?
    return Ok(None)
```

This keeps telemetry ambient only after explicit configuration. Libraries emit useful telemetry when a provider exists, but they do not own exporter setup.

## Reference-level explanation

### Module layout

`std.telemetry` must expose these conceptual submodules or equivalent namespaced surfaces:

- `std.telemetry.core`
- `std.telemetry.trace`
- `std.telemetry.context`
- `std.telemetry.baggage`
- `std.telemetry.semconv`
- `std.telemetry.instrument`
- `std.telemetry.export`
- `std.telemetry.metrics`

The top-level `std.telemetry` module may re-export the most common functions and types, including `configure(...)`, `trace`, `current_context(...)`, `run(...)`, `otlp(...)`, and `semconv`.

### Core data model

`std.telemetry.core` must be pure data and must not start exporters or background tasks. It should define:

- `Timestamp`
- `TelemetryValue`
- `Attributes`
- `Resource`
- `InstrumentationScope`
- `TraceId`
- `SpanId`
- `TraceFlags`
- `SpanContext`
- `Baggage`
- `TelemetryContext`
- `SchemaUrl` or equivalent schema-version metadata

`TelemetryValue` must support at least `None`, booleans, strings, integers, floats, bytes, lists, and maps whose nested values are also telemetry values. Attribute construction requires an explicit, checked conversion for raw bytes and custom models. Apply RFC 103 redaction and bounded nesting, collection size and attribute count to the converted value. For example, a Boolean `cached` is an ordinary attribute. A `Customer` model requires an explicit selection of safe data before recording.

`Attributes` is a string-keyed map of `TelemetryValue`. Attribute keys may use OpenTelemetry semantic-convention names such as `service.name`, `http.request.method`, `db.system.name`, or `gen_ai.request.model`.

### Resource

A `Resource` describes the application that produced the telemetry. Resolve its attributes per key when installing the provider. Explicit application settings (including a supplied `Resource`) take precedence over deployment values, which take precedence over detected defaults. Follow the [OpenTelemetry Resource SDK](https://opentelemetry.io/docs/specs/otel/resource/sdk/) and the [SDK environment variable rules](https://opentelemetry.io/docs/specs/otel/configuration/sdk-environment-variables/).

| Attribute | Incan resolution, from highest priority to fallback |
| --- | --- |
| `service.name` | Explicit service name, then `OTEL_SERVICE_NAME`, then `service.name` in `OTEL_RESOURCE_ATTRIBUTES`, then the application Loaf’s name. If none is available, use `unknown_service:<executable>` or `unknown_service` when the executable name is unavailable. |
| `service.version` | Explicit version, then `service.version` in `OTEL_RESOURCE_ATTRIBUTES`, then the application Loaf’s version if known. Otherwise omit it. |
| `deployment.environment.name` | Explicit environment, then `deployment.environment.name` in `OTEL_RESOURCE_ATTRIBUTES`. If neither supplies it, omit it. |

The Resource also supports process and runtime identity when known. The convenience arguments `service`, `version`, and `environment` set these three attributes. A library emitting a span uses the application’s Resource. The service identity stays the same when the deployment tier changes. See the OpenTelemetry [service](https://opentelemetry.io/docs/specs/semconv/resource/service/) and [deployment](https://opentelemetry.io/docs/specs/semconv/resource/deployment-environment/) conventions.

For example, deploy version `1.4.0` of the `checkout` application Loaf with these settings:

```sh
export OTEL_SERVICE_NAME=orders-api
export OTEL_RESOURCE_ATTRIBUTES=deployment.environment.name=staging
```

```incan
import std.telemetry as telemetry

def main() -> Result[None, telemetry.ConfigurationError]:
    telemetry.configure()?
    return Ok(None)
```

The frozen Resource has service name `orders-api`, version `1.4.0`, and environment `staging`. Removing `OTEL_SERVICE_NAME` from this launch makes the service name fall back to `checkout`. Calling `configure(service="checkout-worker")` instead uses that explicit name and still picks up the deployed environment.

### Instrumentation scope

`InstrumentationScope` represents the logical emitter. For `std.logging`, the logger name is the default scope name. For tracing decorators, the scope should default to the containing module or package identity. A scope may include name, version, schema URL, and attributes.

`std.logging.get_logger("app.checkout")` and `std.telemetry.get_tracer("app.checkout")` should produce compatible scope identity. They do not need to share object identity, but records emitted from both should be correlatable by scope name and configured resource.

### Configuration

`std.telemetry.configure(...)` installs process-level telemetry policy:

```incan
pub def configure(
    service: Option[str] = None,
    version: Option[str] = None,
    environment: Option[str] = None,
    resource: Option[Resource] = None,
    export: Exporter | list[Exporter] | None = None,
    propagation: PropagationPolicy = PropagationPolicy.DEFAULT,
    sampling: SamplingPolicy = SamplingPolicy.DEFAULT,
    instrument: InstrumentationPolicy | list[InstrumentationPolicy] | None = None,
) -> Result[TelemetryProvider, ConfigurationError]
```

The first successful call atomically installs a process-wide provider and freezes its configuration for the lifetime of the process. The provider must own or retain an immutable snapshot of its resource and policies, including the selection and configuration of exporters. Mutating caller-owned configuration inputs after installation cannot alter the installed settings. The provider can continue emitting, buffering and flushing telemetry.

Every subsequent call returns `Err(ConfigurationError.AlreadyConfigured)`, including a call with identical arguments. A rejected call leaves the active provider in place and must not start another exporter. A failed initial setup releases any partially acquired resources and leaves the process unconfigured, so a corrected call can succeed. Concurrent attempts must produce at most one successful installation.

For example, once the application configures the service as `checkout`, a later call specifying `billing` returns `AlreadyConfigured`. Subsequent spans still identify the service as `checkout`. Tests that need independent process-wide configurations can run in separate processes.

Libraries must not call `configure(...)` during import or normal helper execution. Application entrypoints, command runners, test harnesses, and embedding hosts may call it.

### Logging integration

`std.logging` records must use `std.telemetry.core` value types where those types are available. When no telemetry provider is configured, `std.logging` still produces local human or JSON output according to `basic_config(...)`.

When a telemetry provider is configured, `std.logging` records must be enrichable with:

- configured `Resource`
- `InstrumentationScope`
- active `TraceId`, `SpanId`, and `TraceFlags`
- attributes selected from baggage by policy
- exporter routing if log export is enabled

This enrichment must not require callers to change ordinary `log.info(...)` or `log.warning(...)` call sites.

### Tracing decorators

`@telemetry.trace` may be used with no arguments:

```incan
@telemetry.trace
def submit(req: Request) -> Result[Response, Error]:
    ...
```

By default, the span name is the fully qualified Incan name of the traced declaration. It includes the declaring Loaf, module path and any enclosing type. For example, `submit` in the `orders` module of the `checkout` Loaf produces `checkout.orders.submit`. The same declaration spelling in the `billing` Loaf produces `billing.orders.submit`. Calling the first function through an import alias such as `send_order` still produces `checkout.orders.submit`. An explicit `@telemetry.trace(name=...)` supplies the span name instead.

The decorator may accept:

- explicit span name
- attribute map
- attribute factory callable
- span kind
- status mapping policy
- error recording policy

The decorator must start a span before function body execution, make that span current while the function executes, record errors according to policy, and end the span when the function returns, propagates an error, or exits through panic/assert behavior.

For a traced function returning a `Result`, the span reports that call's own return value. An `Err` marks the call as failed and identifies the returned error by its RFC 131 type and case. An `Ok` records a successful call, including when the function recovered from a child call's failure. If the caller propagates the error with `?`, its own span also reports failure because it returned `Err`. Error identity suffices for this rule; it does not require exporting payloads or arguments.

For example, a customer lookup can recover from an unavailable cache by reading from the database. With all three functions traced, the trace reads:

```text
load_customer           succeeded
  read_from_cache       failed: CacheError.Unavailable
  read_from_database    succeeded
```

On export, an ordinary successful call leaves the OTel span status `Unset` and omits `error.type`. For a returned `Err`, export status `Error` with `error.type` identifying the RFC 131 error case. In the cache example only the cache span carries the error type. See the [OTel error conventions](https://opentelemetry.io/docs/specs/semconv/general/recording-errors/).

When a panic unwinds through traced calls, record one failure at its origin. For an assertion failure, include the failed condition and its authored source location, resolvable against the build that produced the trace. Each enclosing span that exits through the same panic reports failure and refers to that origin. Apply RFC 103 redaction to exported evidence. The default trace omits arguments and locals. RFC 131 controls stack trace capture.

```text
import_orders    failed: panic propagated from write_batch
  write_batch    failed: assertion `len(rows) > 0` at writer.incn:42
```

An async function retains its current span across suspension and resumption. At spawn, each child inherits a snapshot of the spawning task’s context and the link to its active parent span. If two siblings inherit the same parent, adding baggage in one child affects only that child’s context. Ending either task leaves the other task’s context valid.

### Explicit spans

Manual spans are allowed:

```incan
span = telemetry.start_span("cache.lookup", attributes={"cache.key": key})
result = cache.get(key)
span.end()
```

Implementations should provide diagnostics or linting for spans that may not end on all paths once the language has enough control-flow analysis. Manual spans are the low-level escape hatch; RFC 095 defines the preferred scoped block form for ordinary sub-operation spans.

### Context and baggage

The API must avoid the reserved `ctx` spelling. Use `context`, `TelemetryContext`, and `current_context(...)`.

`TelemetryContext` must carry current span context and baggage. `Baggage` is a separate key-value carrier and must not be treated as identical to attributes. Baggage may be propagated; attributes describe a specific telemetry record.

With HTTP instrumentation enabled, extract valid W3C Trace Context from inbound requests and inject the active context into outbound requests using `traceparent` and, when present, `tracestate`. A Python caller, an Incan handler and a Rust downstream service can then produce spans under the same trace ID. The default sampler follows the parent’s sampling decision and samples new root traces. See [OTel propagators](https://opentelemetry.io/docs/specs/otel/context/api-propagators/) and [parent-based sampling](https://opentelemetry.io/docs/specs/otel/trace/sdk/#sampling).

Baggage travels with a request under the propagation policy. By default none of its keys are copied to span or log attributes. A public, non-secret key is eligible for copying when explicitly selected by application policy. For example, `checkout` can send `region="eu"` to `billing` for routing. If the application selects `region` for span attributes, the collector also sees `region="eu"` on those spans. Secret values are excluded from both propagation and attribute export, even when a key is selected. See [OTel baggage](https://opentelemetry.io/docs/concepts/signals/baggage/).

`TelemetryContext.with_baggage(key, value)` returns a new context with an additional baggage item. It must not mutate unrelated active context invisibly.

`telemetry.run(context, fn, *args, **kwargs)` runs a callable inside a telemetry context. The exact variadic spelling should follow existing Incan callable conventions.

### Semantic conventions

`std.telemetry.semconv` should expose OpenTelemetry semantic conventions through generated, versioned helpers and constants. Helpers should prefer Incan-friendly function names while returning standard attribute keys:

```incan
import std.telemetry as telemetry
from std.telemetry import semconv

def user_attrs() -> telemetry.Attributes:
    return semconv.http.server_request(method="GET", route="/users/{id}", status_code=200)
```

RFC 095 span blocks accept typed attribute models. Generate those models, the convenience functions that return attribute maps, and the key constants from the same pinned convention metadata. For example, a block can use `semconv.HttpServer(request=semconv.HttpRequest(method="GET"), route="/users/{id}")`. Map-returning helpers serve call sites that accept `Attributes`.

Constants should remain available:

```incan
import std.telemetry as telemetry
from std.telemetry import semconv

def user_attrs() -> telemetry.Attributes:
    return {semconv.http.request.method: "GET"}
```

HTTP instrumentation names the server span using the method and matched route template, for example `GET /customers/{id}` for a request to `/customers/42`. If the route template is unavailable, use the method alone. A traced Incan function inside that request still produces its declaration name, such as `checkout.customers.load`. See the [OTel HTTP span conventions](https://opentelemetry.io/docs/specs/semconv/http/http-spans/).

Each Incan release pins one OpenTelemetry semantic-convention version for its generated standard helpers. Generated metadata must identify the version and mark experimental helpers. An ordinary dependency refresh retains the same attribute keys. When a release intentionally updates the pin, it must identify changed, renamed and removed helpers and their affected keys so applications can update instrumentation and dashboards together.

### Exporters

Exporter APIs must be explicit. `telemetry.otlp(...)` constructs exporter configuration; it must not export anything until installed in `configure(...)` or an equivalent provider.

The exporter surface should include at least:

- no-op exporter
- stdout/debug exporter
- OTLP exporter shape

The OTLP exporter supports `http/protobuf` (default) and `grpc`. Use standard OTLP configuration to select the protocol and endpoint. For example, an application that installs `telemetry.otlp()` can send to an HTTP/Protobuf collector with `OTEL_EXPORTER_OTLP_ENDPOINT=http://collector:4318`. To use a gRPC collector instead, set `OTEL_EXPORTER_OTLP_PROTOCOL=grpc` and point `OTEL_EXPORTER_OTLP_ENDPOINT` at its gRPC endpoint. The same instrumented code serves both deployments. Follow the [OTLP exporter specification](https://opentelemetry.io/docs/specs/otel/protocol/exporter/).

Protobuf encoding belongs to the OTLP exporter. Applications using only local logging or core telemetry types must build independently of the OTLP codec and transport dependencies. Applications using either OTLP transport must build with the supported Incan toolchain alone. If bindings are generated, use pinned schemas and supply the resulting bindings with the exporter. Regeneration belongs to exporter maintenance. An ordinary consumer build must neither require a system `protoc` or CMake for Protobuf nor silently download or build a vendored `protoc`.

Implementation acceptance includes clean builds of a local-logging program and an OTLP program on a host without `protoc` or CMake. Inspect their resolved build dependencies and build steps to catch transitive generator dependencies. Exercise both OTLP transports against a collector and verify it receives a known span.

Batching, retry, shutdown flushing and collector compatibility remain part of the exporter contract.

Explicit flush reports whether pending export succeeded, failed, or timed out. Shutdown attempts to drain buffered records within a bound before releasing exporter resources. For example, `oven test` can finish its last test span and shut down telemetry before exiting. A collector outage can be reported separately while the command retains the actual test result. See the [OTel tracing SDK lifecycle](https://opentelemetry.io/docs/specs/otel/trace/sdk/#forceflush).

### Stdlib instrumentation policy

Stdlib instrumentation must be controlled by application policy. The application explicitly enables instrumentation at configuration time. Importing a stdlib module or choosing an exporter alone does not enable that module’s automatic instrumentation. Local logging continues to follow the application’s logging settings.

Instrumentation policy should support:

- enabling all conservative stdlib instrumentation
- enabling a specific module such as `std.http` or `std.process`
- limiting instrumentation level, such as errors-only for file operations
- disabling instrumentation for sensitive modules
- selecting whether baggage keys may be copied into record attributes

Stdlib modules should use semantic conventions where they correspond to common operations.

### Metrics

`std.telemetry.metrics` provides typed counters and histograms. A declaration specifies its measurement type, unit, and permitted label keys and value types. Recording a value or label that does not match the declaration must produce a diagnostic. The application declares what it measures. Deployment policy chooses compatible aggregation and bounds the number of distinct label combinations an instrument tracks.

For example, declare an HTTP duration histogram in seconds with labels for the matched route and response class. Two successful requests to `/orders/41` and `/orders/42` contribute two measurements to the same series:

```text
instrument: http.server.request.duration   kind: histogram   unit: s
route=/orders/{id}   response_class=2xx   observations: 0.012, 0.034
```

A request ID belongs on the trace. Recording it as a label on this histogram is a diagnostic because the declaration permits only route and response class. Even a declared string label can have many distinct values; the deployment limit must bound series growth during collection.

## Design details

### Why `std.telemetry` rather than only `std.logging`

OpenTelemetry is broader than logging. Logs need resources, instrumentation scopes, trace context, baggage policy, and exporters to become fully useful in production. Traces and metrics need the same provider configuration and semantic-convention vocabulary. Keeping all of that inside `std.logging` would overload a simple logging API and confuse users who only want local log output.

### Why `std.logging` sits on `std.telemetry.core`

The log record model should be OpenTelemetry-aligned because record shape is part of the long-lived public contract. The full provider/exporter stack should remain opt-in. Splitting `std.telemetry.core` from provider behavior gives both: simple logs are OTel-shaped, while exporting remains explicit.

### Why decorators are the primary tracing API

Function spans are the most common tracing unit. A decorator can start a span, make it current, record errors, and end it around the full function body without requiring users to manually balance `start_span(...)` and `end(...)` calls. The default span name can be derived from the function's fully qualified symbol, avoiding string repetition. RFC 095 complements decorators with scoped `span` blocks for sub-operations that are not whole functions.

### Why manual spans still exist

Not all spans align with whole functions. Cache lookups, parsing regions, external retries, or partial workflow steps may need narrower scopes. RFC 095 makes `span "name":` the ordinary source form for these scopes. Manual spans still exist for advanced APIs and unusual dynamic cases, but documentation should present them as lower-level than decorators and vocabulary blocks.

### Why baggage is not attributes

Baggage propagates across boundaries; attributes describe a particular telemetry record. Automatically copying all baggage into every span or log can leak sensitive data and create high-cardinality telemetry. The API should make propagation and record attributes distinct, with explicit policy for copying selected baggage keys.

### Why semantic conventions need helpers

OpenTelemetry semantic conventions are valuable because they standardize attribute names and meanings. They are also large and evolving. Handwritten string constants scattered through stdlib code would drift. Generated helpers and constants give users discoverability, type hints, and version metadata while still producing standard OTel keys.

### Why aggregation belongs to deployment policy

The application knows that it measured an HTTP duration in seconds. Operators can change histogram aggregation or the series limit for that deployment without changing the measurement at every request handler.

## Alternatives considered

1. **Only align `std.logging` with OpenTelemetry and stop there**
   - Rejected because logs without resource identity, trace context, propagation, and exporter policy are only partially useful. `std.logging` should be simple, but Incan needs a broader observability story.

2. **Expose Rust `tracing` and OpenTelemetry SDK crates directly**
   - Rejected because ordinary Incan source should not depend on Rust macro syntax, subscribers, crate-specific setup, or exporter implementation details.

3. **Make telemetry ambient by default**
   - Rejected because exporting telemetry can send data across process or network boundaries. Ambient behavior is acceptable only after explicit application configuration.

4. **Prioritize metrics over tracing and logging**
   - Rejected because tracing, logging, and context define the correlation model that metrics also need to participate in. The metrics API still belongs in the north star, but it should be shaped by the same resource, scope, exporter, and semantic-convention model rather than designed in isolation.

5. **Require explicit span names everywhere**
   - Rejected because the compiler already knows function and module identity. Requiring users to repeat `checkout.submit` on `submit` creates drift and weakens the language's ability to provide good defaults.

6. **Use a general `with` statement as the only span API**
   - Rejected for this RFC because function decorators cover the most common tracing pattern, and RFC 095 gives telemetry spans a direct vocabulary-block spelling. A general `with` statement remains useful for context-manager APIs, but span ergonomics should not depend on it exclusively.

## Drawbacks

- The API surface is larger than a logging-only design.
- OpenTelemetry alignment introduces terminology that simple users may not care about.
- Provider/exporter behavior needs careful policy and shutdown semantics.
- Upgrading pinned semantic conventions can require coordinated changes to application code and dashboards.
- Tracing decorators need robust interaction with async functions and error handling.
- Wide label sets can exhaust a series budget and reduce the detail visible to operators.

## Implementation architecture

This section is non-normative. A pragmatic implementation should preserve the same layering as the public contract: `std.telemetry.core` pure data types remain independent from provider/exporter behavior; `std.logging` integration uses the shared data model; provider installation can support no-op and local debug exporters without changing application call sites; network exporters add delivery policy rather than new record shapes. Tracing decorators are the safer common path compared with manual span balancing. Semantic-convention helpers should be generated from pinned OpenTelemetry convention metadata rather than maintained by hand. Stdlib instrumentation should use conservative policies so enabling telemetry does not produce noisy or sensitive data by default.

## Layers affected

- **Parser / AST**: no new syntax is required for the core RFC; existing decorators are used for tracing. A general `with` statement requires a separate language RFC.
- **Typechecker / Symbol resolution**: decorator validation, attribute factory signatures, telemetry value compatibility, and semantic-convention helper types need checked behavior.
- **IR Lowering**: tracing decorators lower to span lifecycle calls around function bodies while preserving return and error behavior.
- **Emission**: generated Rust needs telemetry provider access, current-context handling, and exporter/runtime hooks without exposing Rust crate names in Incan source.
- **Stdlib / Runtime (`loaves/stdlib/`)**: `std.telemetry.core`, provider configuration, exporter plumbing, tracing runtime state, context propagation, and stdlib instrumentation hooks are added.
- **Formatter**: existing decorator formatting should be sufficient for the decorator API; new syntax is not required.
- **LSP / Tooling**: completions, hovers, and diagnostics should explain semantic-convention helpers, telemetry decorators, baggage/context APIs, and instrumentation policy.
- **Docs / examples**: docs must distinguish simple `std.logging` from opt-in `std.telemetry`, show controlled ambient setup, and document data-export implications.

## Design Decisions

- **Process configuration:** The first successful installation freezes the settings. Every later installation attempt returns a typed error.
- **Export:** Support OTLP over HTTP/Protobuf by default and gRPC through configuration.
- **Resource identity:** Resolve explicit attributes first, then standard OpenTelemetry environment settings, then application Loaf defaults for service name and version.
- **Attribute values:** Bound structured values, redact secrets and require checked conversion for bytes and custom models.
- **Async context:** Preserve the active span across suspension and snapshot context independently for each spawned task.
- **Stdlib instrumentation:** Automatic recording by stdlib modules requires explicit application opt-in.
- **Baggage:** Copy only public, non-secret keys explicitly selected by application policy into record attributes.
- **Default span names:** Use the fully qualified name of the declaration, independent of how the caller imports it.
- **Traced results:** Each span reports its own call’s outcome. A caller that recovers can succeed while the failed child remains visible.
- **Panics and assertions:** Record the origin once. Spans unwound by the same failure refer to that origin.
- **Convention versions:** Pin one version per Incan release, mark experimental helpers and identify changes during an intentional upgrade.
- **Metrics:** Declare typed instruments, explicit units and permitted labels. Deployment policy selects compatible aggregation and bounds label combinations.
