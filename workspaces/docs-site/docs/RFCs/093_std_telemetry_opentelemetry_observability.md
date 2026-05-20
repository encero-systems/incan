# RFC 093: `std.telemetry` — OpenTelemetry-aligned observability

- **Status:** Draft
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
    - RFC 080 (AI assets, models, prompts, evals, and agent metadata)
    - RFC 084 (RHS partial callable presets)
    - RFC 089 (`std.environ`)
    - RFC 090 (typed CLI framework)
    - RFC 094 (context managers)
    - RFC 095 (`span` vocabulary blocks)
    - OpenTelemetry specification and semantic conventions (external)
- **Issue:** #559
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
8. **Metrics are part of the north star:** `std.telemetry.metrics` belongs in the observability surface. Its meter API must address cardinality, aggregation, units, temporality, and views as part of the normative design.

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
- Include metrics in the telemetry direction while keeping the meter API subject to the same normative design bar as logs and traces.

## Non-Goals

- Making telemetry export implicit merely because a module is imported.
- Replacing `std.logging` with `std.telemetry`; simple logs should remain simple.
- Exposing Rust `tracing`, OpenTelemetry SDK crates, subscribers, collectors, or exporter implementation details in ordinary Incan source.
- Standardizing a final metrics API in this RFC.
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

def main() -> None:
    telemetry.configure(
        service="checkout-api",
        version=VERSION,
        environment="production",
        export=telemetry.otlp(endpoint="http://collector:4318"),
        instrument=telemetry.instrument.stdlib(),
    )
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

The default span name is derived from the fully qualified Incan symbol, such as `checkout.submit`. Users should not have to repeat the function name as a string for ordinary tracing.

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
import std.telemetry.vocab

span "cache.lookup", attributes={"cache.key": key}:
    result = cache.get(key)
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

Applications control stdlib instrumentation policy:

```incan
def main() -> None:
    telemetry.configure(
        service="checkout-api",
        export=telemetry.otlp(),
        instrument=[
            telemetry.instrument.std_http(),
            telemetry.instrument.std_process(),
            telemetry.instrument.std_fs(level=telemetry.InstrumentationLevel.ERRORS),
        ],
    )
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

`TelemetryValue` must support at least `None`, booleans, strings, integers, floats, bytes, lists, and maps whose nested values are also telemetry values. Implementations may add model-to-telemetry conversion hooks, but they must not silently stringify structured values when the destination claims structured preservation.

`Attributes` is a string-keyed map of `TelemetryValue`. Attribute keys may use OpenTelemetry semantic-convention names such as `service.name`, `http.request.method`, `db.system.name`, or `gen_ai.request.model`.

### Resource

`Resource` represents the entity that produced telemetry. At minimum it must support service identity:

- `service.name`
- `service.version`
- deployment environment when known
- process/runtime identity when known

The `configure(...)` convenience parameters `service`, `version`, and `environment` lower into `Resource` attributes. Direct resource construction must also be available for advanced users.

### Instrumentation scope

`InstrumentationScope` represents the logical emitter. For `std.logging`, the logger name is the default scope name. For tracing decorators, the scope should default to the containing module or package identity. A scope may include name, version, schema URL, and attributes.

`std.logging.get_logger("app.checkout")` and `std.telemetry.get_tracer("app.checkout")` should produce compatible scope identity. They do not need to share object identity, but records emitted from both should be correlatable by scope name and configured resource.

### Configuration

`std.telemetry.configure(...)` installs process-level telemetry policy:

```incan
pub def configure(
    service: str,
    version: Option[str] = None,
    environment: Option[str] = None,
    resource: Option[Resource] = None,
    export: Exporter | list[Exporter] | None = None,
    propagation: PropagationPolicy = PropagationPolicy.DEFAULT,
    sampling: SamplingPolicy = SamplingPolicy.DEFAULT,
    instrument: InstrumentationPolicy | list[InstrumentationPolicy] | None = None,
) -> TelemetryProvider
```

Calling `configure(...)` more than once must be deterministic. The accepted behavior may be replacement, explicit error, or scoped provider installation, but it must not silently merge incompatible exporter or resource policies.

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

With no explicit name, the span name must default to the fully qualified Incan symbol. For a function `submit` in module `checkout`, the default name should be `checkout.submit` or the package-qualified equivalent once package identity is available.

The decorator may accept:

- explicit span name
- attribute map
- attribute factory callable
- span kind
- status mapping policy
- error recording policy

The decorator must start a span before function body execution, make that span current while the function executes, record errors according to policy, and end the span when the function returns, propagates an error, or exits through panic/assert behavior.

Async functions must preserve the same logical current span across suspension points once async context propagation is supported.

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

Constants should remain available:

```incan
import std.telemetry as telemetry
from std.telemetry import semconv

def user_attrs() -> telemetry.Attributes:
    return {semconv.http.request.method: "GET"}
```

The semantic-convention surface must record the OpenTelemetry semantic-convention version it was generated from. Packages may support multiple convention versions, but one active version per Incan release is acceptable for the standard library.

### Exporters

Exporter APIs must be explicit. `telemetry.otlp(...)` constructs exporter configuration; it must not export anything until installed in `configure(...)` or an equivalent provider.

The exporter surface should include at least:

- no-op exporter
- stdout/debug exporter
- OTLP exporter shape

OTLP transport details such as HTTP versus gRPC, batching, retry, shutdown flushing, environment variable mapping, and collector compatibility are part of the exporter contract.

### Stdlib instrumentation policy

Stdlib instrumentation must be controlled by application policy. A module should not emit network-exported telemetry solely because it was imported.

Instrumentation policy should support:

- enabling all conservative stdlib instrumentation
- enabling a specific module such as `std.http` or `std.process`
- limiting instrumentation level, such as errors-only for file operations
- disabling instrumentation for sensitive modules
- selecting whether baggage keys may be copied into record attributes

Stdlib modules should use semantic conventions where they correspond to common operations.

### Metrics

`std.telemetry.metrics` is the OpenTelemetry-compatible metrics namespace. The meter API must address instrument kind, aggregation, units, temporality, views, cardinality control, async runtime behavior, and exporter cost.

The RFC deliberately treats metrics as part of the observability contract, not an optional add-on. Any accepted meter API must carry the same design weight as logs and traces.

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

### Why metrics remain unresolved

Metrics are deceptively easy to sketch and hard to get right. A counter and histogram API is not enough; production metrics require aggregation, temporality, cardinality controls, units, views, async instruments, and exporter behavior. The namespace belongs in the telemetry design, and the meter API must answer those questions as part of the contract.

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
- Semantic-convention helpers need a generation and versioning story.
- Tracing decorators need robust interaction with async functions and error handling.
- Metrics require deeper API design than logs and traces because their correctness depends on aggregation, temporality, views, and cardinality control.

## Implementation architecture

This section is non-normative. A pragmatic implementation should preserve the same layering as the public contract: `std.telemetry.core` pure data types remain independent from provider/exporter behavior; `std.logging` integration uses the shared data model; provider installation can support no-op and local debug exporters without changing application call sites; network exporters add delivery policy rather than new record shapes. Tracing decorators are the safer common path compared with manual span balancing. Semantic-convention helpers should be generated from pinned OpenTelemetry convention metadata rather than maintained by hand. Stdlib instrumentation should use conservative policies so enabling telemetry does not produce noisy or sensitive data by default.

## Layers affected

- **Parser / AST**: no new syntax is required for the core RFC; existing decorators are used for tracing. A general `with` statement requires a separate language RFC.
- **Typechecker / Symbol resolution**: decorator validation, attribute factory signatures, telemetry value compatibility, and semantic-convention helper types need checked behavior.
- **IR Lowering**: tracing decorators lower to span lifecycle calls around function bodies while preserving return and error behavior.
- **Emission**: generated Rust needs telemetry provider access, current-context handling, and exporter/runtime hooks without exposing Rust crate names in Incan source.
- **Stdlib / Runtime (`incan_stdlib`)**: `std.telemetry.core`, provider configuration, exporter plumbing, tracing runtime state, context propagation, and stdlib instrumentation hooks are added.
- **Formatter**: existing decorator formatting should be sufficient for the decorator API; new syntax is not required.
- **LSP / Tooling**: completions, hovers, and diagnostics should explain semantic-convention helpers, telemetry decorators, baggage/context APIs, and instrumentation policy.
- **Docs / examples**: docs must distinguish simple `std.logging` from opt-in `std.telemetry`, show controlled ambient setup, and document data-export implications.

## Unresolved questions

- What is the exact normative shape of `TelemetryValue`, especially for bytes, nested models, and custom conversion hooks?
- Should `configure(...)` replace an existing provider, reject repeated calls, or support scoped provider installation?
- How much of `Resource` should be inferred automatically from project metadata versus supplied explicitly?
- What is the precise package/module qualification rule for default decorator span names?
- What error-status mapping should `@telemetry.trace` apply to `Result` returns versus raised panics or exceptions?
- What is the minimal safe context propagation story for async tasks and spawned work?
- Which baggage keys, if any, should be copied into attributes by default?
- How should semantic-convention helpers be generated, versioned, and exposed when OpenTelemetry convention stability changes?
- What meter API should `std.telemetry.metrics` expose, and how should it control cardinality and aggregation?
- Which exporter transports belong in standard telemetry support?
- Which stdlib modules should have default instrumentation, and what default instrumentation policy avoids noisy or sensitive telemetry?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
