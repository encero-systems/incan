# RFC 095: `span` vocabulary blocks

- **Status:** Planned
- **Created:** 2026-05-11
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 019 (test runner and fixtures)
    - RFC 036 (user-defined decorators)
    - RFC 040 (scoped DSL surface forms)
    - RFC 045 (scoped DSL symbol surfaces)
    - RFC 072 (`std.logging`)
    - RFC 081 (language-shaped DSL embeddings)
    - RFC 093 (`std.telemetry`)
    - RFC 094 (context managers)
    - RFC 103 (`std.secrets`)
    - RFC 142 (async composition and task context)
    - RFC 160 (vocabulary in Incan)
    - OpenTelemetry specification and semantic conventions (external)
- **Issue:** [#561](https://github.com/encero-systems/incan/issues/561)
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC defines `span` as a standard vocabulary block layered over context managers and `std.telemetry`. A module that imports `std.telemetry` may write `span "operation": ...` to create an OpenTelemetry-aligned scoped span for an arbitrary block of work. The block is exactly `with telemetry.span(...)`, so it inherits RFC 094's cleanup, cancellation and failure rules. Span names follow ordinary Incan dotted namespacing, attributes are typed models whose keys come from a declared namespace, the current span travels with the task across `await`, ending a span never fails, and `std.telemetry.testing` provides a `span_recorder` fixture. The spelling is a soft keyword, so ordinary identifiers named `span` keep working.

## Core model

Read this RFC as seven foundations:

1. **Span blocks are vocabulary syntax:** `span` is active only in a module that imports `std.telemetry`; elsewhere it is an ordinary name.
2. **A span is a scoped unit of work:** the block starts a span before the suite, makes it current for the suite, and ends it after the suite on every exit path.
3. **A span block is a `with` block:** `span "name", ...:` means `with telemetry.span("name", ...):`, using RFC 094's scoped entry and exit instead of telemetry-only cleanup semantics.
4. **Names follow Incan namespacing:** a span's name extends the dotted path of its enclosing declaration or parent span, like any nested name.
5. **Attributes are typed models:** a model declares its attribute namespace, its fields become dotted keys, and a misspelled attribute is a compile error.
6. **Telemetry remains opt-in:** span blocks are valid when telemetry is not configured, but they must not export data or start background work unless `std.telemetry.configure(...)` has installed a provider.
7. **The current span belongs to the task:** span context is part of a task's execution context, so it survives `await` and a spawned task starts as a child of the span that spawned it.

## Motivation

OpenTelemetry spans are not just library calls; they describe the causal structure of a program. A span represents a unit of work, may have a parent span, records timestamps and attributes, and can be nested into a trace. That maps naturally to an indented block. Function decorators cover whole-function spans, but they do not cover the sub-operation scopes that make traces useful: validation, cache lookup, queue publish, database query, remote call, model invocation, serialization, file write, or retry loop.

Manual span handles are too easy to misuse. Users can forget to end a span, return early before cleanup, lose the current context, or create inconsistent naming. Incan has a chance to make the common case structurally correct: if a block is a span, the compiler can guarantee that it starts before the block and ends after the block while preserving ordinary control flow.

Attribute maps have the same weakness. A dictionary such as `{"cache.key": key}` accepts any misspelled key and any value, and nothing connects it to the semantic conventions it claims to follow. A typed model gives attributes the same checking as the rest of the program.

The construct should not become a hard telemetry keyword. `span` is a standard, high-value example of vocabulary syntax: domain syntax that is explicit, scoped, typed, and backed by ordinary stdlib APIs.

## Goals

- Define `span` as a telemetry vocabulary block, not as a globally reserved hard keyword.
- Let users create block-level spans without manual `start_span()` / `end()` pairing.
- Define a span block as `with telemetry.span(...)`, reusing RFC 094 for guaranteed scoped cleanup.
- Align the block with `std.telemetry` and OpenTelemetry span concepts.
- Name spans through ordinary Incan dotted namespacing.
- Give spans typed attribute models whose keys come from a declared namespace, and ship the OpenTelemetry semantic conventions as such models.
- Make `std.logging` records emitted inside a span automatically correlatable when telemetry is configured.
- Keep telemetry export disabled unless the application explicitly configures a provider.
- Carry the current span with the task across `await` and into spawned tasks.
- Provide a test-only fixture that records spans in memory.

## Non-Goals

- Replacing `@telemetry.trace` for ordinary whole-function tracing.
- Defining the `std.telemetry` provider, exporter, and metrics APIs, which RFC 093 owns.
- Defining `with` statements or context managers, which RFC 094 owns.
- Making `span` a hard keyword in all Incan modules.
- Requiring applications to configure telemetry merely to compile or run code containing span blocks.
- Treating every block in Incan as implicitly traced.
- Defining a full macro system for arbitrary vocabulary statements.

## Guide-level explanation

### Activation

Importing `std.telemetry` turns on the `span` block:

```incan
import std.telemetry
```

An import alias such as `import std.telemetry as telemetry` activates it as well. A module that does not import `std.telemetry` can still use `span` as an ordinary variable name, and a `span "name":` statement there produces a diagnostic that points at the missing import.

### A span is a scoped block

```incan
span "cache_lookup", attributes=CacheAttributes(key=key):
    result = cache.get(key)
```

The block starts the span before `cache.get(key)`, makes it current while the suite runs, and ends it after the suite exits. If the suite returns, breaks, continues, propagates an error with `?`, panics, or is cancelled, the span still ends and records the outcome. That is because a span block is a `with` block:

```incan
with telemetry.span("cache_lookup", attributes=CacheAttributes(key=key)):
    result = cache.get(key)
```

Both spellings mean the same thing. The `span` form says directly that the block is an operation, and it is the form the compiler names from its surroundings.

### Names follow Incan namespacing

A span's name extends the dotted path of the declaration it is in, the same way a nested name would:

```incan
# reports/daily.incn
import std.telemetry

def build() -> None:
    span "read":                  # reports.daily.build.read
        rows = load()
        span "parse":             # reports.daily.build.read.parse
            records = parse(rows)
```

A dotted name adds several segments: `span "gateway.authorize":` inside `checkout.submit` is `checkout.submit.gateway.authorize`. Nothing is guessed from punctuation.

A bare `span:` takes its name from the enclosing function, so inside `reports.daily.build` it is `reports.daily.build`. Outside any named declaration, a bare `span:` is refused with a diagnostic asking for a name.

Function decorators remain the best spelling for whole-function spans, and nested span blocks extend the function's name:

```incan
import std.telemetry as telemetry

@telemetry.trace
def submit(order: Order) -> Result[Receipt, Error]:   # checkout.submit
    span "validate":                                   # checkout.submit.validate
        validate(order)?

    span "charge":                                     # checkout.submit.charge
        return charge(order)
```

### Attributes are typed models

A model becomes an attribute set by declaring its namespace. Its fields become dotted keys under that namespace:

```incan
@telemetry.attributes("cache")
model CacheAttributes:            # keys: cache.key, cache.hit
    key: str
    hit: bool = false
```

A nested model extends the namespace with its field name, which is how OpenTelemetry's dotted semantic-convention keys arise:

```incan
@telemetry.attributes("http")
model HttpServer:                 # keys: http.request.method, http.route
    request: HttpRequest
    route: str

model HttpRequest:
    method: str
```

`std.telemetry.semconv` ships the OpenTelemetry semantic conventions as such models, so `semconv.HttpServer(request=semconv.HttpRequest(method="GET"), route="/users")` produces the standard keys. `CacheAttributes(kye=key)` is a compile error, and so is a field whose type OpenTelemetry cannot carry.

### Updating a span from inside the block

`as` binds the span handle. It is typed by the span's attribute model, so updates are checked too:

```incan
span "cache_lookup" as current, attributes=CacheAttributes(key=key):
    result = cache.get(key)
    current.set(hit=result.is_some())
    if result.is_none():
        current.add_event("cache.miss")
```

`current.set(hti=true)` is a compile error. Helper functions that are not inside the block reach the same span through `telemetry.current_span()`.

### Kind, links and long headers

```incan
span (
    "authorize",
    kind=telemetry.SpanKind.CLIENT,
    attributes=semconv.HttpClient(request=semconv.HttpRequest(method="POST"), url=gateway_url),
    links=[upstream_context],
):
    response = gateway.authorize(request)?
```

A long header uses parentheses, and a trailing comma is allowed.

### Async code

The current span is part of the task's context, so a span block may contain `await`:

```incan
async def refresh(client: Client) -> Result[None, HttpError]:
    span "refresh":
        response = (await client.get(feed_url))?
        span "store":
            await store(response)
    return Ok(None)
```

While `refresh` waits, other tasks run without seeing its span. A task spawned inside the block starts as a child of the span that spawned it, the same way an RFC 142 spawned task keeps the validation policy it was spawned under.

### Logging

```incan
span "query", attributes=semconv.Db(system=semconv.DbSystem(name="postgresql")):
    log.info("query started")
    rows = db.query(sql)?
```

When telemetry is configured, logs emitted inside the block carry the active trace and span identifiers. When it is not configured, the block still behaves correctly and cheaply, and nothing is exported.

### Testing spans

`std.telemetry.testing` provides a `span_recorder` fixture. During a test that requests it, every span is recorded in memory instead of exported:

```incan
# shop/cart.incn
import std.telemetry

def lookup(cache: Cache, key: str) -> Option[str]:
    span "cache_lookup" as current, attributes=CacheAttributes(key=key):
        result = cache.get(key)
        current.set(hit=result.is_some())
        return result

# tests/test_cart.incn
from shop.cart import lookup
from std.telemetry.testing import SpanRecorder
from std.testing import assert_eq

def test_lookup_records_cache_attributes(span_recorder: SpanRecorder) -> None:
    lookup(Cache.containing("a"), "a")
    recorded = span_recorder.only("shop.cart.lookup.cache_lookup")
    assert_eq(recorded.attributes, {"cache.key": "a", "cache.hit": true})
    assert_eq(recorded.parent, None)
```

The recorder also exposes each span's events, status, kind, links and parent, so a test can check the shape of a whole trace.

## Reference-level explanation

### Activation and soft-keyword behavior

`span` is a soft statement keyword owned by `std.telemetry`. It must be parsed as a span block only in statement position and only in a module that imports `std.telemetry`, with or without an alias. The vocabulary descriptor is an implementation detail of `std.telemetry` under RFC 160; it must not be a separate public module.

These remain ordinary identifier uses:

```incan
span = telemetry.current_span()
return span
fields = {"span": span.id}
```

Without the import, a statement-position `span "name":` must produce a diagnostic that names the missing `import std.telemetry`.

### Syntax

```incan
span:
    suite

span string_literal:
    suite

span string_literal [as name], span_options:
    suite

span (span_header):
    suite
```

`span_header` is the payload of the compact form: an optional string-literal name, an optional `as name` binding, and zero or more named options, with a trailing comma allowed. The options are:

- `attributes=expr`, whose type is a `@telemetry.attributes` model;
- `kind=expr`, a `telemetry.SpanKind`, defaulting to `INTERNAL`;
- `links=expr`, a list of span contexts.

Option expressions must be evaluated once, before the span is entered. Events and status are set through the span handle, not the header.

### Desugaring

A span block must mean exactly:

```incan
with telemetry.span(name, attributes=..., kind=..., links=...) as binding:
    suite
```

where `name` is the resolved span name and `binding` is the `as` name or a hidden binding. It therefore follows RFC 094 in full: the setup is evaluated once, exit runs exactly once when entry succeeded, exit runs on fallthrough, `return`, `break`, `continue`, `?` propagation, panic and cancellation, and exit never suppresses or transforms the suite's control flow. Inside an async function, a span block is an `async with` block. `with telemetry.span(...)` written by hand must behave identically, except that it receives no lexical name: its name argument is used as written.

### Naming

A span block's name must be the canonical dotted path of its innermost enclosing span block or, when there is none, of its enclosing declaration, followed by `.` and the block's name. A dotted block name contributes each of its segments. Each segment must be a valid identifier. The name must never be interpreted as already qualified because of its punctuation.

A bare `span:` must take the canonical path of its enclosing declaration as its name, extended by any enclosing span blocks. A bare `span:` outside a named declaration must be refused with a diagnostic asking for a name. Function-level `@telemetry.trace` spans must use the function's canonical path, so nested span blocks extend it.

### Attribute models

`@telemetry.attributes(namespace)` must mark a model as an attribute set with the given dotted namespace. Each field must produce an attribute key formed from the namespace, followed by the field's name. A field whose type is another model must extend the key with its field name and contribute that model's fields beneath it, so the namespace `http` with a field `request` of a model with field `method` produces `http.request.method`. A nested model need not be marked itself.

Field types must be OpenTelemetry attribute value types: `str`, `bool`, `int`, `float`, lists of one of these, an `Option` of one of these, or a nested model. A field of any other type must be refused at compile time. An `Option` field that is `None` must be omitted. Two fields that would produce the same key must be refused at compile time. A field of type `SecretStr` or `SecretBytes` must be recorded only as a redaction marker under RFC 103, never as its value.

`std.telemetry.semconv` must provide the OpenTelemetry semantic conventions as attribute models, so that their keys match the specification exactly.

### Span handle

A span handle must be typed by its attribute model as `Span[A]`. `set(field=value, ...)` must accept only fields of `A`, with their declared types. `add_event(name, attributes=...)` must record an event, whose attributes are themselves an attribute model. `set_status(...)` must set the status explicitly. `telemetry.current_span()` must return the handle of the current span, typed by a general attribute view, or a non-recording handle when there is none.

### Semantics and status

A span block must start a span before running the suite, make it current for the suite, and end it after the suite exits, preserving the suite's original control flow.

Status must follow the RFC 094 `ScopeExit`:

- `Success`, `Return` with a successful value, `Break`, and `Continue` must leave the status unset unless the user set it.
- `Error`, including an explicit return whose evaluated `Result` is `Err`, must set the status to error and record the failure.
- `Panic` must set the status to error. Record the exception event once at the panic’s origin; an enclosing span unwound by that panic refers to the same failure under RFC 093.
- `Cancelled` must leave the status unset and record a `cancelled` event, because cancellation is not an application failure under RFC 142.

Ending a span must never fail. A provider or exporter failure must be reported through telemetry diagnostics and must never enter the program's control flow or replace the suite's outcome.

If telemetry has not been configured, span blocks must still be valid. A no-op span must preserve block execution and must not export data.

### OpenTelemetry mapping

A span block maps to a `std.telemetry` span with:

- its resolved name;
- a parent derived from the current span of the task, if present;
- start and end timestamps;
- its kind, defaulting to internal;
- its attributes and any set through the handle;
- its links;
- events added through the handle;
- its status.

Logs emitted through `std.logging` inside a span block must be correlated with the active span when telemetry is configured, through the `std.telemetry.core` record model.

### Task context

The current span must be part of a task's execution context. An `await` inside a span block must leave the span current for that task when it resumes, and must not make it current for any other task meanwhile. A task spawned while a span is current must start with that span as its parent. Ordinary calls, awaited operations, and `join` branches run in the caller's context and see its current span.

Holding a thread-bound guard from foreign code, such as a Rust interop span guard, across an `await` must be refused under RFC 094's suspension rule.

### Source metadata

Span creation may attach the logical source identity: package, module, declaration, and a path relative to the package root. Physical filesystem paths must not be attached unless a telemetry policy explicitly enables them, in which case they are subject to the same redaction rules as other sensitive values.

### Testing

`std.telemetry.testing` must provide a `span_recorder` fixture, available only in test code. During a test that requests it, every span must be recorded in memory rather than exported, with its name, attributes, events, status, kind, links, and parent. The recorder must support looking up spans by name and must fail the lookup when the expected number of spans is not found.

### Sampling

Span blocks must create the same logical span request whether or not the provider samples it. If the provider decides the span is not recording, the block still executes normally and the handle is a valid non-recording handle.

## Design details

### Why vocabulary syntax instead of only decorators

Decorators are good for declaration-shaped spans:

```incan
@telemetry.trace
def submit(order: Order) -> Result[Receipt, Error]:
    return charge(order)
```

Many important spans are not declarations. They are sub-operations inside a function. A block-level vocabulary form covers those without forcing users into manual span handles.

### Why `span` when it is a `with` block

`with telemetry.span(...)` is valid and identical in behavior. The `span` form is justified because it states the intent directly and because the compiler names it from its surroundings, which a function call cannot do.

### Why soft keyword

`span` is a common domain word and a likely variable name in observability code. Making it a hard keyword would be unnecessary damage. Import-activated soft syntax lets modules opt into the block form while ordinary code can still bind and pass values named `span`.

### Why typed attribute models

A dictionary of attributes accepts misspelled keys, wrong value types, and keys that drift from the semantic conventions they imitate. A model with a declared namespace is checked like any other model, its nested structure produces OpenTelemetry's dotted keys without anyone writing them by hand, and the semantic conventions become importable, documented types.

### Why task context

In runtimes that keep the current span in a thread-local, a task that awaits leaves its span installed on the thread, and the next task to run there inherits the wrong parent. Making the span part of the task's own context removes that failure instead of forbidding `await`, and it matches how RFC 142 carries validation policy into spawned tasks.

## Alternatives considered

1. Use only `@telemetry.trace`. Rejected because whole-function tracing does not cover sub-operation spans inside a function.

2. Use only `with telemetry.span(...)`. Rejected as the primary surface because it hides telemetry intent behind a generic resource abstraction and cannot be named from its surroundings. It remains available with identical behavior.

3. Make `span` a hard keyword. Rejected because import activation provides the syntax without taking an ordinary identifier from all programs.

4. Activate through a separate `std.telemetry.vocab` module. Rejected because the vocabulary belongs to `std.telemetry` itself; a second import adds ceremony without meaning.

5. Mark fully qualified names explicitly, or guess them from dots. Rejected because span names follow ordinary Incan namespacing like every other nested name.

6. Use dictionaries for attributes. Rejected because nothing checks their keys or values.

7. Give each attribute field its own key alias. Rejected because every dotted key would be written out by hand; a namespace and nested models produce the same keys from structure.

8. Forbid `await` inside span blocks, or keep the current span thread-local. Rejected because async code needs spans and task context makes them correct.

9. Make span finalization fallible. Rejected because instrumentation must never break the program it observes.

10. Auto-create spans for every function or block. Rejected because uncontrolled ambient telemetry creates noise, cost, and privacy risk. Instrumentation should be explicit in source or explicit through configured stdlib policy.

## Drawbacks

- Adding vocabulary syntax for telemetry increases parser, formatter, and tooling complexity.
- A native `span` block may encourage over-instrumentation if docs do not emphasize meaningful operation boundaries.
- Span names change when the enclosing declaration is renamed or moved, because they follow its path.
- Task-scoped span context requires every executor and spawn path to carry it.
- Attribute models are more ceremony than a dictionary for a one-off attribute.
- A no-op unconfigured span must be cheap enough that libraries can contain span blocks without surprising runtime cost.

## Implementation architecture

This section is non-normative. The telemetry vocabulary descriptor parses `span` blocks and lowers them to the `with telemetry.span(...)` form with the resolved name attached. The compiler resolves attribute-model keys at compile time, so recording an attribute set needs no reflection at runtime.

## Layers affected

- **Parser / AST**: import-activated statement-position `span` blocks are needed without reserving `span` globally.
- **Typechecker / Symbol resolution**: activation, option expression types, attribute models and their key collisions, typed span handles, resolved names, and the foreign-guard suspension rule must be validated.
- **IR Lowering**: span blocks must lower to the RFC 094 `with` or `async with` form while preserving original control flow.
- **Stdlib / Runtime**: `std.telemetry` must expose the span manager, no-op behavior, current-span access, attribute models, `semconv` models, span kinds, links, events, status policy, and task-scoped span context; `std.telemetry.testing` must expose the `span_recorder` fixture.
- **Formatter**: span block headers and suites need stable formatting; long headers should format into the parenthesized multiline form.
- **LSP / Tooling**: highlighting must show `span` as vocabulary syntax only when active; hover should show the resolved span name and attribute keys, and diagnostics should explain missing imports.

## Design Decisions

- `import std.telemetry`, with or without an alias, activates `span`; there is no separate `.vocab` module, and `span` stays an ordinary name elsewhere.
- `span "name" as current:` binds a handle typed by the span's attribute model; helpers use `telemetry.current_span()`.
- Span names follow Incan dotted namespacing: a block's name extends the path of its enclosing span or declaration, segment by segment, with no qualification flag and no guessing from punctuation.
- A span block is exactly `with telemetry.span(...)`, so RFC 094's rules apply. A bare `span:` takes its enclosing declaration's name and is refused outside a named declaration.
- Header options are the name, an `as` binding, `attributes`, `kind`, and `links`. Attributes are `@telemetry.attributes(namespace)` models whose fields and nested models produce dotted keys, with compile-time checks for unknown fields, unsupported types, and key collisions; `std.telemetry.semconv` provides the semantic conventions as such models. Events and status go through the typed handle.
- The current span is part of the task's context: it survives `await`, spawned tasks start as its children, and only foreign thread-bound guards across `await` are refused.
- Source metadata defaults to logical identity and package-relative paths; physical paths need an explicit policy and are redacted.
- Ending a span never fails; provider and exporter failures go to telemetry diagnostics.
- `std.telemetry.testing` provides a test-only `span_recorder` fixture that records spans in memory.
