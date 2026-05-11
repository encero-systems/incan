# RFC 095: `span` vocabulary blocks

- **Status:** Draft
- **Created:** 2026-05-11
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 027 (`incan-vocab` block registration and desugaring)
    - RFC 036 (user-defined decorators)
    - RFC 040 (scoped DSL surface forms)
    - RFC 045 (scoped DSL symbol surfaces)
    - RFC 072 (`std.logging`)
    - RFC 081 (language-shaped DSL embeddings)
    - RFC 093 (`std.telemetry`)
    - RFC 094 (context managers)
    - OpenTelemetry specification and semantic conventions (external)
- **Issue:** #561
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC defines `span` as a standard vocabulary block layered over context managers and `std.telemetry`. A module that explicitly activates the telemetry vocabulary may write `span "operation": ...` to create an OpenTelemetry-aligned scoped span for an arbitrary block of work. The spelling is a soft keyword owned by `std.telemetry.vocab`, not a globally reserved core keyword. The construct gives Incan native syntax for operation scopes while keeping telemetry export opt-in, preserving ordinary identifiers named `span`, and reusing the context-manager cleanup contract from RFC 094.

## Core model

Read this RFC as seven foundations:

1. **Span blocks are vocabulary syntax:** `span` is active only where the telemetry vocabulary is explicitly imported or otherwise activated.
2. **A span is a scoped unit of work:** the block starts a span before the suite, makes it current for the suite, and ends it after the suite on every exit path.
3. **OpenTelemetry semantics are the target:** names, attributes, kinds, links, events, status, parentage, context, and resource/scope enrichment should follow `std.telemetry` and OpenTelemetry concepts.
4. **Context managers supply the mechanism:** the block lowers through the RFC 094 scoped entry/exit contract instead of inventing telemetry-only cleanup semantics.
5. **Naming is ambient but controlled:** default and relative names may use module/function/span nesting, but authors can always provide an explicit span name.
6. **Telemetry remains opt-in:** span blocks are valid when telemetry is not configured, but they must not export data or start background work unless `std.telemetry.configure(...)` has installed a provider.
7. **Async safety is stricter than ordinary blocks:** span context must not be corrupted by holding non-await-safe guards across suspension points.

## Motivation

OpenTelemetry spans are not just library calls; they describe the causal structure of a program. A span represents a unit of work, may have a parent span, records timestamps and attributes, and can be nested into a trace. That maps naturally to an indented block. Function decorators cover whole-function spans, but they do not cover the sub-operation scopes that make traces useful: validation, cache lookup, queue publish, database query, remote call, model invocation, serialization, file write, or retry loop.

Manual span handles are too easy to misuse. Users can forget to end a span, return early before cleanup, lose the current context, or create inconsistent naming. Incan has a chance to make the common case structurally correct: if a block is a span, the compiler can guarantee that it starts before the block and ends after the block while preserving ordinary control flow.

The construct should not become a hard telemetry keyword. The project already has a vocabulary direction for descriptor-gated DSL surfaces. `span` should be a standard, high-value example of that direction: domain syntax that is explicit, scoped, typed, and backed by ordinary stdlib APIs.

## Goals

- Define `span` as a telemetry vocabulary block, not as a globally reserved hard keyword.
- Let users create block-level spans without manual `start_span()` / `end()` pairing.
- Reuse RFC 094 context-manager semantics for guaranteed scoped cleanup.
- Align the block with `std.telemetry` and OpenTelemetry span concepts.
- Support explicit span names, ambient relative naming, attributes, span kind, links, and events where appropriate.
- Make `std.logging` records emitted inside a span automatically correlatable when telemetry is configured.
- Keep telemetry export disabled unless the application explicitly configures a provider.
- Define conservative async rules so current-span context is not accidentally held across unsafe suspension.

## Non-Goals

- Replacing `@telemetry.trace` for ordinary whole-function tracing.
- Defining the full `std.telemetry` provider, exporter, metrics, or semantic-convention APIs in this RFC.
- Adding general `with` statements or context managers in this RFC.
- Making `span` a hard keyword in all Incan modules.
- Requiring applications to configure telemetry merely to compile or run code containing span blocks.
- Treating every block in Incan as implicitly traced.
- Defining a full macro system for arbitrary vocabulary statements.

## Guide-level explanation

A module opts into telemetry vocabulary syntax explicitly:

```incan
import std.telemetry.vocab
```

After activation, a span block creates a scoped operation:

```incan
span "cache.lookup", attributes={"cache.key": key}:
    result = cache.get(key)
```

The block starts the span before `cache.get(key)`, makes the span current while the suite runs, and ends the span after the suite exits. If the suite returns, breaks, continues, propagates an error with `?`, or panics through an assertion, the span still ends and records the appropriate outcome.

Function decorators remain the best spelling for whole-function spans:

```incan
import std.telemetry as telemetry
import std.telemetry.vocab

@telemetry.trace
def submit(order: Order) -> Result[Receipt, Error]:
    span "validate", attributes={"order.id": order.id}:
        validate(order)?

    span "charge", attributes={"payment.method": order.payment.method}:
        return charge(order)
```

If `submit` lives in module `checkout`, the function decorator may create `checkout.submit`. The nested span names should become `checkout.submit.validate` and `checkout.submit.charge` unless the implementation chooses a different accepted naming rule. Authors can still provide fully explicit names when a domain convention needs them:

```incan
span "payment.gateway.authorize", kind=telemetry.SpanKind.CLIENT, attributes=attrs:
    response = gateway.authorize(request)?
```

Bare `span:` is permitted only if the telemetry vocabulary defines a clear ambient name:

```incan
span:
    refresh_cache()
```

The bare form is convenient for quick instrumentation, but documentation should prefer named spans for durable traces because operation names are part of the observability contract.

Span blocks compose with logging:

```incan
span "db.query", attributes={"db.system.name": "postgresql"}:
    log.info("query started")
    rows = db.query(sql)?
```

When telemetry is configured, logs emitted inside the block can carry the active trace and span identifiers. When telemetry is not configured, the block must still behave correctly and cheaply, but it must not export telemetry.

Long span headers may use parentheses so formatter-friendly option lists stay readable:

```incan
span (
    "http.request",
    kind=telemetry.SpanKind.SERVER,
    attributes=semconv.http.server_request(method=req.method, route=route),
):
    response = handle(req)
```

## Reference-level explanation

### Activation and soft-keyword behavior

`span` is a soft statement keyword owned by `std.telemetry.vocab`. It is parsed as a span block only in statement position and only when the telemetry vocabulary is active for the module or lexical scope.

These forms are span blocks when the vocabulary is active:

```incan
span:
    suite

span "name":
    suite

span "name", attributes=attrs:
    suite
```

These remain ordinary identifier uses:

```incan
span = telemetry.current_span()
return span
fields = {"span": span.id}
```

If the vocabulary is not active, statement-position `span "name":` must produce a diagnostic that explains the missing vocabulary activation rather than treating the source as unrelated syntax.

### Syntax

The grammar should support compact and parenthesized span headers:

```incan
span:
    suite

span string_literal:
    suite

span string_literal, span_options:
    suite

span (span_header):
    suite
```

`span_header` is the same payload accepted by the compact form: an optional string-literal name followed by zero or more named options. A trailing comma is allowed in the parenthesized form.

`span_options` should support named arguments:

- `attributes=expr`
- `kind=expr`
- `links=expr`
- `events=expr`

The option expressions are evaluated before the span is entered. Their types must satisfy the corresponding `std.telemetry` API contracts.

The standard spelling should prefer string-literal span names. Relative dotted shorthand such as `span .charge:` is outside this RFC unless it is justified by the accepted naming rule.

### Naming

A span name identifies a class of operation, not a single instance. The provided name must be used as the OpenTelemetry span name after applying Incan's relative naming rules.

If the name contains no package or module separator and the compiler can determine an active lexical operation prefix, the implementation should treat it as relative to the active function or parent span name. For example, inside `checkout.submit`, `span "validate":` should produce `checkout.submit.validate`.

If the author provides a name that is already fully qualified according to the accepted naming rule, the implementation must not add the lexical prefix again.

Bare `span:` must derive a name from the nearest useful lexical symbol or active operation. This is intentionally less explicit and should not be the primary style for durable instrumentation.

### Semantics

A span block must start a span before running the suite, make it current for the suite, and end it after the suite exits. The block must preserve the suite's original control flow.

The span block must lower through the RFC 094 context-manager contract or an equivalent internal representation with the same guarantees:

- the span setup expression is evaluated once;
- if span entry succeeds, span exit runs exactly once;
- span exit runs on fallthrough, `return`, `break`, `continue`, `?` propagation, and panic/assert exits;
- span exit receives an informational `ScopeExit`;
- span exit must not suppress or transform the original control flow.

If telemetry has not been configured, span blocks must still be valid. The implementation may use a no-op span, but it must preserve block execution and must not export data.

### OpenTelemetry mapping

A span block maps to a `std.telemetry` span with at least:

- a span name;
- a parent derived from the active telemetry context, if present;
- start and end timestamps;
- span kind, defaulting to internal if not specified;
- attributes supplied by the block options;
- links supplied by the block options;
- events supplied by the block options or added through the active span handle;
- status derived from explicit user calls or from `ScopeExit`.

`ScopeExit.Success` should normally leave status unset unless the user explicitly marks success. `ScopeExit.Error` and `ScopeExit.Panic` should mark or record an error according to the accepted `std.telemetry` policy.

Logs emitted through `std.logging` inside a span block should be correlated with the active span when telemetry is configured. This means the resulting log records may include trace and span identifiers through the `std.telemetry.core` record model.

### Span handle access

The basic `span "name":` form does not bind a handle. If the user needs to add attributes or events inside the block, the API should provide `telemetry.current_span()`:

```incan
span "cache.lookup":
    current = telemetry.current_span()
    result = cache.get(key)
    current.set_attribute("cache.hit", result.is_some())
```

This RFC leaves `span "name" as current:` unresolved. The form is useful, but it also makes `span` look more like `with`; the current-span accessor is enough unless the ergonomics prove too weak.

### Async interaction

Span blocks must follow at least the async-safety contract required by RFC 094. Additionally, because current-span state affects trace correctness, a span block must not hold a non-await-safe current-span guard across an `await`.

The accepted implementation must choose one of these models before this RFC advances beyond Draft:

- reject `await` inside span blocks until async-aware span context propagation exists;
- allow `await` only when the telemetry provider exposes an await-safe context mechanism;
- define a separate async span block lowering that propagates span context through the async runtime rather than using a synchronous guard.

The default must be correctness over convenience.

## Design details

### Why vocabulary syntax instead of only decorators

Decorators are good for declaration-shaped spans:

```incan
@telemetry.trace
def submit(order: Order) -> Result[Receipt, Error]:
    return charge(order)
```

Many important spans are not declarations. They are sub-operations inside a function. A block-level vocabulary form covers those without forcing users into manual span handles.

### Why `span` instead of `with telemetry.span(...)`

`with telemetry.span(...)` should remain a valid explicit form once RFC 094 exists. The `span` vocabulary form is still justified because the compiler can attach lexical operation names, source locations, and descriptor identity more naturally than an ordinary function call. It also makes the source communicate the intent directly: this block is an operation span, not a generic resource.

### Why soft keyword

`span` is a common domain word and a likely variable name in observability code. Making it a hard keyword would be unnecessary damage. Descriptor-gated soft syntax lets modules opt into the block form while ordinary code can still bind and pass values named `span`.

### Interaction with semantic conventions

Span attributes should be ordinary `std.telemetry.Attributes`, so users can pass semantic-convention helpers:

```incan
import std.telemetry as telemetry
from std.telemetry import semconv

span "http.request", kind=telemetry.SpanKind.SERVER, attributes=semconv.http.server_request(method=req.method, route=route):
    response = handle(req)
```

For longer semantic-convention calls, the parenthesized header is the preferred source form:

```incan
span (
    "http.request",
    kind=telemetry.SpanKind.SERVER,
    attributes=semconv.http.server_request(method=req.method, route=route),
):
    response = handle(req)
```

This RFC does not define the helper catalog, but span blocks must accept the attribute values produced by it.

### Interaction with sampling

Span blocks should create the same logical span request whether or not the provider samples it. If the provider decides the span is not recording, the block still executes normally and `telemetry.current_span()` should return a valid non-recording span handle or equivalent no-op representation.

### Interaction with source metadata

The compiler may attach source module, function, package, and source-location metadata to span creation when the telemetry policy enables it. This metadata must not become the only way to name spans; explicit names remain the public contract.

## Alternatives considered

1. Use only `@telemetry.trace`. Rejected because whole-function tracing does not cover sub-operation spans inside a function.

2. Use only `with telemetry.span(...)`. Rejected as the primary surface because it hides telemetry intent behind a generic resource abstraction and gives the compiler less room to provide lexical naming and vocabulary-owned diagnostics.

3. Make `span` a hard keyword. Rejected because vocabulary activation can provide the syntax without taking an ordinary identifier from all programs.

4. Make bare `span:` the primary spelling. Rejected because durable traces need meaningful operation names. The bare form is useful, but it should be secondary.

5. Auto-create spans for every function or block. Rejected because uncontrolled ambient telemetry creates noise, cost, and privacy risk. Instrumentation should be explicit in source or explicit through configured stdlib policy.

## Drawbacks

- Adding vocabulary syntax for telemetry increases parser, formatter, and tooling complexity.
- Span names can become inconsistent if users mix relative and fully qualified naming without clear conventions.
- A native `span` block may encourage over-instrumentation if docs do not emphasize meaningful operation boundaries.
- Async context propagation is subtle and must be designed before broad use in async-heavy code.
- A no-op unconfigured span must be cheap enough that libraries can contain span blocks without surprising runtime cost.

## Implementation architecture

This section is non-normative. The telemetry vocabulary descriptor can parse `span` blocks and lower them to a context-manager-like scoped span object supplied by `std.telemetry`. The compiler can attach lexical identity and source metadata during this lowering, then rely on the RFC 094 cleanup representation to call span exit on all block exits.

## Layers affected

- **Parser / AST**: descriptor-gated statement-position `span` blocks are needed without reserving `span` globally.
- **Typechecker / Symbol resolution**: telemetry vocabulary activation, option expression types, relative naming metadata, and async-safety checks must be validated.
- **IR Lowering**: span blocks must lower through the scoped context-manager mechanism while preserving original control flow.
- **Emission**: generated code must start, enter, exit, and end spans deterministically without relying on manual user cleanup.
- **Stdlib / Runtime (`incan_stdlib`)**: `std.telemetry` must expose the span manager, no-op behavior, current-span access, attributes, span kinds, links, events, and status policy used by the vocabulary block.
- **Formatter**: span block headers and suites need stable formatting without losing descriptor-owned syntax; long headers should format into the parenthesized multiline form.
- **LSP / Tooling**: highlighting must show `span` as vocabulary syntax only when active; hover and diagnostics should explain the desugared telemetry operation and missing-vocabulary imports.

## Unresolved questions

- What exact import or activation spelling should enable `std.telemetry.vocab`?
- Should `span "name" as current:` be included, or should users rely on `telemetry.current_span()`?
- What is the exact rule that distinguishes relative and fully qualified span names?
- Should bare `span:` be accepted, or should all span blocks require an explicit name?
- Which span options belong in the grammar: `attributes`, `kind`, `links`, `events`, `schema`, `record_exception`, or a smaller set?
- Which async-safety model should span blocks use when async-aware propagation is not available?
- How should source-location metadata be controlled so useful debugging metadata does not leak sensitive paths by default?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
