# RFC 066: `std.http` — Incan-first HTTP client and request/response surface

- **Status:** Draft
- **Created:** 2026-04-16
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 022 (namespaced stdlib modules and compiler handoff)
    - RFC 023 (compilable stdlib and Rust module binding)
    - RFC 037 (native web stdlib redesign)
    - RFC 051 (`JsonValue` for `std.json`)
    - RFC 055 (`std.fs` path-centric filesystem APIs)
    - RFC 063 (`std.process` process spawning and command execution)
    - RFC 078 (tool execution and typed workflow actions)
    - RFC 103 (`std.secrets` secret strings and bytes)
- **Issue:** https://github.com/encero-systems/incan/issues/84
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC proposes `std.http` as Incan's standard library module for explicit HTTP client work. The module standardizes a request or response model, one-shot and client-based request APIs, client lifecycle, protocol negotiation, timeout and retry policy, structured errors, and JSON convenience surfaces so ordinary programs, tools, and automation workflows do not need to fall through to `rust::reqwest`-shaped APIs or ad hoc wrappers.

## Core model

Read this RFC as one foundation plus five mechanisms:

1. **Foundation:** HTTP is a general-purpose stdlib capability, not a CI-only or framework-only helper surface.
2. **Mechanism A:** `std.http` provides explicit `Request`, `Response`, `Body`, `Method`, and `HttpError` types with predictable behavior and no panic-driven network contract.
3. **Mechanism B:** the module supports both one-shot convenience helpers and a reusable `Client` surface so simple scripts and heavier integrations share one coherent model.
4. **Mechanism C:** client lifecycle and pooling are explicit enough that repeated calls do not depend on hidden global connection state.
5. **Mechanism D:** JSON, timeout, redirect, and retry behavior remain explicit policy surfaces rather than ambient magic.
6. **Mechanism E:** HTTP protocol negotiation, streaming, and test transports remain inspectable seams instead of backend-specific escape hatches.

## Motivation

Networking is a recurring boundary for ordinary Incan programs: API clients, release tooling, CI automation, ingestion pipelines, service-to-service calls, health checks, and migration scripts all need HTTP. Today, the practical escape hatch is Rust interop. That works, but it leaks Rust-shaped APIs, Rust-shaped errors, and inconsistent conventions into user code precisely where the standard library should provide one stable model.

This matters for more than ergonomics. HTTP boundaries are policy-heavy: timeouts, retries, redirect handling, header redaction, JSON decoding, and error reporting all need explicit behavior. If every project rebuilds these choices differently, the language ends up with a fragmented story for one of the most common integration surfaces.

`std.http` should therefore do for network requests what `std.fs`, `std.process`, and the newer stdlib RFCs are doing in their domains: define an Incan-first contract while still allowing the runtime to map onto Rust-native implementations underneath.

## HTTP client prior art

### Requests baseline

Python's `requests` is useful as the ergonomic baseline. Its quickstart frames ordinary HTTP verbs as obvious one-line calls, while still returning a response object the caller can inspect. Incan should keep that floor: a health check, webhook call, artifact fetch, or small API client should not require building a full framework object graph.

Source: [Requests quickstart](https://docs.python-requests.org/en/latest/user/quickstart/).

The Incan lesson is:

- method-specific helpers such as `get`, `post`, `put`, and `delete` are worth keeping
- helpers should return the same response model as explicit requests
- simple does not mean ambient: timeouts, errors, redaction, and policy still need defined behavior
- the public API should be obvious before it is powerful

### HTTPX lessons

HTTPX is useful prior art because it modernizes the `requests` shape without reducing the design to convenience helpers. Its documentation presents a fully featured client with sync and async APIs, HTTP/1.1 and HTTP/2 support, strict timeouts, async clients for async frameworks, and opt-in HTTP/2 with response-level protocol visibility.

Sources: [HTTPX introduction](https://www.python-httpx.org/), [HTTPX async support](https://www.python-httpx.org/async/), and [HTTPX HTTP/2 support](https://www.python-httpx.org/http2/).

The Incan lesson is not to copy Python's split between `Client` and `AsyncClient` literally. The useful design pressure is:

- a reusable client is a real resource, not just a namespace for functions
- connection pooling and cleanup should be visible in the API contract
- one-shot helpers are useful, but repeated requests should have an obvious client-owned path
- timeout policy should be present by default and refinable later into connect/read/write/overall timeout fields
- HTTP/2 should be an explicit protocol policy, not an accidental backend behavior
- responses should expose the negotiated protocol version
- streaming and test transports should fit the same `Request` / `Response` / `HttpError` vocabulary

Incan should go further than HTTPX where the language gives it leverage: typed errors instead of exception families, model-aware JSON decoding, capability-gated network access, and policy-visible remote data flow for tools, CI, and AI-backed actions.

### Koheesio lessons

Koheesio is useful prior art because it treats HTTP as a pipeline step concern, not only as an ad hoc client call. Its HTTP step surface includes method-specific steps, a shared request configuration shape, timeout options, retry behavior, response outputs such as raw payload, JSON payload, and status code, paginated HTTP GET support, and explicit masking for sensitive authorization headers. Its async HTTP step also makes session, retry, and connector state visible.

Sources: [Koheesio HTTP steps](https://engineering.nike.com/koheesio/0.10.0/api_reference/steps/http.html) and [Koheesio async HTTP steps](https://engineering.nike.com/koheesio/0.10.0/api_reference/asyncio/http.html).

The Incan lesson is:

- `std.http` should be general-purpose, but its request and response types must compose cleanly with step, pipeline, and typed-action systems
- retry, timeout, pagination, and authorization are operational concerns, not just transport knobs
- response projections should be stable enough for workflow outputs, logs, quality checks, and tests
- sensitive header handling belongs in the core design, not only in logging docs
- async execution should make session and connector ownership visible without forcing backend-specific types into user code

Incan should not copy Koheesio's Python/Pydantic runtime boundary literally. The stdlib contract should preserve the step-friendly shape while using `Result[..., HttpError]`, typed request/response models, compile-time metadata, and Rust-native execution underneath.

## Goals

- Provide a first-class `std.http` module for client-side HTTP work.
- Standardize explicit request and response types rather than centering shell calls or Rust interop.
- Keep timeout behavior first-class and non-ambient.
- Define a structured `HttpError` model so network failures, status failures, timeout failures, decoding failures, and policy failures are distinguishable.
- Provide JSON convenience helpers that compose cleanly with RFC 051 `JsonValue`.
- Support both one-shot request helpers and a reusable `Client` surface.
- Make `Client` lifecycle, cleanup, and reuse explicit enough to support connection pooling without hidden globals.
- Make negotiated HTTP protocol information visible on responses, while avoiding a v1 requirement that every backend support HTTP/2.
- Keep request and response models structured enough to compose with typed workflow actions, pipeline steps, logs, tests, and generated reports.
- Make retry behavior explicit and policy-shaped rather than automatic and invisible.
- Leave room for streaming bodies and test transports without leaking backend-specific transport types.
- Require safe default treatment of sensitive headers in diagnostics and debug-facing representations.
- Accept secret value types for authentication and header-building APIs so callers do not need to reveal tokens into plain strings before sending requests.

## Non-Goals

- Defining server-side HTTP routing or handler APIs here; that belongs with RFC 037 and related web-platform work.
- Shipping a full browser fetch surface in this RFC; browser-oriented HTTP behavior may reuse the same contract later but is not defined here.
- Making HTTP a language intrinsic or keyword surface.
- Introducing a GitHub- or cloud-specific SDK into the standard library.
- Standardizing cookies, OAuth flows, multipart forms, WebSockets, or HTTP/3-specific behavior in the first version.
- Requiring HTTP/2 support from every v1 implementation.

## Guide-level explanation

### One-shot requests

For simple scripts, users should be able to write:

```incan
from std.http import get

response = get("https://api.example.com/health", timeout=5s)?
text = response.text()?
```

The important point is not the exact helper spelling. The important point is that ordinary request code stays inside `std.http`, uses `Result[..., HttpError]`, and does not require dropping into `rust::`.

### Explicit requests

For more control, users should be able to build a request directly:

```incan
from std.http import Body, Method, Request, send

request = Request(
    method=Method.POST,
    url="https://api.example.com/events",
    headers={"Authorization": token, "Content-Type": "application/json"},
    body=Body.json(payload),
    timeout=10s,
)

response = send(request)?
```

This makes policy visible:

- the method is explicit
- the body is explicit
- the timeout is explicit
- the caller chooses whether to inspect status, body bytes, text, or JSON

### Reusable clients

For workflows that share headers, retries, or transport settings, users should be able to use a `Client`:

```incan
from std.http import Client, RetryPolicy

client = Client(
    default_headers={"Authorization": token},
    timeout=15s,
    retry=RetryPolicy.transient(max_attempts=3),
)

response = client.get("https://api.example.com/items")?
items = response.json()?
```

This does not change the basic model. It only moves repeated policy into one reusable value.

### Client lifecycle and pooling

A `Client` should be treated as a resource that owns transport state such as connection pools, default headers, timeout policy, retry policy, redirect policy, and protocol preferences. The exact cleanup spelling is left to the implementation, but the API must make deterministic cleanup possible.

One-shot helpers are still valuable for scripts and probes. Repeated calls, service-to-service integrations, crawlers, SDKs, and long-running tools should have an obvious client path so code does not create a fresh transport stack in a hot loop.

### Protocol negotiation

HTTP/2 support should be explicit without making it mandatory for all implementations. A client or request should be able to declare a protocol policy:

```incan
from std.http import Client, Protocol

client = Client(protocol=Protocol.Http2Preferred)
response = client.get("https://api.example.com/items")?

println(response.protocol)
```

The exact names can change, but the shape should support "use the backend default", "HTTP/1 only", "prefer HTTP/2", and "require HTTP/2." If HTTP/2 is required and the implementation cannot provide it, the result should be a structured `HttpError`, not a silent downgrade.

### Status handling should stay explicit

The response model should not hide status behavior behind panics. Users should opt into strict status expectations:

```incan
response = client.get(url)?
response = response.require_success()?
data = response.json()?
```

or branch explicitly:

```incan
response = client.get(url)?

if response.status.is_success:
    return response.json()?
else:
    return Err(HttpError.unexpected_status(response.status))
```

### Sensitive data should not print carelessly

Headers such as `Authorization` should be redactable in debug-facing output by default:

```incan
println(request)
```

should not casually dump bearer tokens or secrets into logs.

When the caller uses `SecretStr` or `SecretBytes` from RFC 103, redaction should come from the value type as well as from conservative header-name rules. A header value derived from a secret wrapper must remain redacted even if the header name is custom.

## Reference-level explanation

### Module surface

`std.http` must provide, at minimum:

- `Method`
- `Body`
- `Request`
- `Response`
- `StatusCode`
- `HttpError`
- `Client`
- protocol policy and negotiated protocol-version metadata, or equivalent types
- one-shot request helpers or a functionally equivalent request entry surface
- explicit retry-policy types if retry behavior is part of the request contract

The exact spelling of all helpers is part of the module API, but the contract is that the user-facing model is request- and response-centric rather than shell-centric or backend-centric.

### Request model

A `Request` must carry:

- method
- URL
- headers
- query parameters if modeled separately from the URL
- body
- timeout policy
- redirect policy if separately configurable
- protocol policy if the caller needs to override the client default
- retry policy when the caller opts into retries

A request must be constructible without requiring a `Client`.

### Response model

A `Response` must expose:

- status code
- negotiated protocol version when available
- response headers
- body bytes
- helpers for decoding text and JSON

The response model should also define stable, tool-friendly projections for common workflow outputs, such as status code, raw text or bytes, parsed JSON when requested, and redacted diagnostic summaries. These projections let pipeline steps, typed actions, tests, and reports use HTTP results without scraping backend-specific response objects.

A response must not silently panic on unsuccessful status codes. Status-based failure should remain explicit through helpers such as `require_success()` or equivalent APIs.

### Error model

`std.http` operations must return `Result[..., HttpError]`.

`HttpError` must distinguish at least:

- connection failures
- timeout failures
- redirect-policy failures
- TLS or transport failures
- unsupported or failed protocol negotiation
- decode failures
- explicit status-policy failures

The module may include richer variants, but it must not collapse all failures into one undifferentiated string.

### Client lifecycle

A `Client` owns reusable transport state. The contract must define:

- how a client is closed or otherwise released
- whether operations after cleanup fail with a structured error
- which options are client defaults versus per-request overrides
- how one-shot helpers scope any temporary client state

The API should make client reuse the natural path for repeated requests. One-shot helpers may internally create and dispose of clients, but the docs should not encourage creating new reusable clients inside tight loops.

### Timeouts

Timeouts must be first-class and explicit. The contract must define:

- how request timeouts are attached
- whether a client-level timeout can be overridden per request
- what error variant a timeout produces

This RFC intentionally does not hardcode one exact default timeout yet; see unresolved questions.

Timeouts may start as one total request timeout, but the API should not block later support for distinct connect, read, write, and overall timeout fields.

### Protocol negotiation

The public contract should not assume that HTTP/1.1 is the only possible transport. It should standardize a small protocol-policy vocabulary, exact names pending:

- backend default / automatic negotiation
- HTTP/1 only
- HTTP/2 preferred
- HTTP/2 required

Implementations that do not support HTTP/2 may reject HTTP/2-preferred policies up front, or accept them and fall back to HTTP/1.x. HTTP/2-required policies must fail with a structured `HttpError` when the implementation, target, or peer cannot provide HTTP/2. If an implementation accepts a preferred policy and downgrades to HTTP/1.x, the `Response` must expose the protocol that was actually used.

### Retries

Retries must be opt-in and policy-shaped. A retry policy may cover:

- maximum attempts
- backoff strategy
- which status codes are retryable
- which transport failures are retryable

The module must not silently retry every request by default.

### Pagination and workflow composition

The base `std.http` module does not need to standardize one pagination framework. It should, however, keep request construction, response decoding, and client reuse composable enough for libraries to build paginated fetchers, polling loops, and API-specific steps on top of the same primitives.

Pipeline or workflow integrations should depend on `std.http` request/response models, not backend transport objects. A workflow action that fetches remote data should be able to report its URL policy, timeout, retry policy, status code, body shape, and redacted diagnostics through machine-readable action output.

### JSON integration

`Body.json(value)` or an equivalent API may accept `JsonValue` and, where later RFCs standardize model-oriented JSON encoding, other serializable values.

`Response.json()` must decode into `JsonValue` at minimum. Typed decode into models may be added through compatible follow-up RFCs, but this RFC's floor is a coherent `JsonValue` path.

### Redaction and debug-facing behavior

Implementations should redact sensitive header values such as `Authorization`, `Proxy-Authorization`, and similarly sensitive token-bearing headers in debug-facing request or response displays.

Header values constructed from RFC 103 `SecretStr` or `SecretBytes` must be treated as sensitive regardless of header name. Authentication helpers should accept secret value types directly so user code does not need to expose a token as a plain string before constructing a request.

The public contract does not need to prescribe every redacted header name exhaustively in v1, but it must require that sensitive-header treatment is conservative and documented. Header-name heuristics are a fallback; value-level secret typing is the stronger contract when available.

### Streaming and transports

The first implementation does not need to support every streaming body shape, but the request and response model should leave room for:

- streaming response bodies
- streaming request bodies
- explicit body size limits
- test transports that return synthetic responses without network access
- local application transports for testing `std.web` applications through the same client vocabulary

Any transport abstraction must preserve `Request`, `Response`, `HttpError`, timeout, protocol, redaction, and policy semantics. Backend-specific transport handles must not become the public API.

## Design details

### Syntax

This RFC does not require new language syntax. It is a namespaced stdlib surface.

### Semantics

The semantic center is explicit network behavior:

- request creation is explicit
- client lifecycle is explicit
- protocol negotiation is visible
- timeout policy is explicit
- retry policy is explicit
- status handling is explicit
- failures are structured

The module should not rely on hidden ambient globals for client state, retry behavior, or timeout behavior.

### Interaction with existing features

- **RFC 051 (`JsonValue`)**: JSON request and response helpers should compose with `JsonValue` as the baseline dynamic JSON type.
- **RFC 055 (`std.fs`)**: file uploads or downloads may later compose with path or file surfaces, but this RFC does not require multipart or streaming file-transfer APIs.
- **RFC 063 (`std.process`)**: HTTP should remain a direct network API, not a wrapper over shelling out to `curl`.
- **RFC 037 (native web stdlib redesign)**: this RFC covers client-side HTTP. Server-side web contracts remain separate even if they eventually share types such as methods or status codes.
- **RFC 078 (tool execution and typed workflow actions)**: HTTP-capable tools and actions should be able to surface network access, protocol policy, and remote data flow through action metadata and policy checks.
- **RFC 103 (`std.secrets`)**: authentication helpers, header builders, diagnostics, retries, telemetry, and workflow output should preserve `SecretStr` and `SecretBytes` redaction semantics.

### Compatibility / migration

This feature is additive. Existing Rust-interop HTTP wrappers remain valid, but the design claim is that new code, docs, and examples should prefer `std.http` once it exists.

## Alternatives considered

- **Rust interop only**
  - Rejected because it leaves a common boundary with Rust-shaped APIs, Rust-shaped errors, and inconsistent conventions.
- **Shell out to `curl`**
  - Rejected because it weakens safety, portability, and structured error handling.
- **Only one-shot helpers, no `Client`**
  - Rejected because real tooling and API clients need reusable policy and shared headers.
- **Only `Client`, no one-shot helpers**
  - Rejected because it makes simple scripts too ceremonious.
- **A pipeline-specific HTTP step as the primary API**
  - Rejected because HTTP is a general-purpose stdlib capability. Step and workflow libraries should compose over `std.http`; they should not own the base transport contract.
- **Separate public sync and async client models**
  - Rejected for now because Incan should keep one conceptual client contract. Implementations may still provide blocking convenience helpers or async-only methods where the runtime requires them.
- **Mandatory HTTP/2 in v1**
  - Rejected because the API should not block on backend coverage or target support. The important v1 contract is that protocol policy and negotiated protocol metadata have a place to live.
- **Hide protocol version entirely**
  - Rejected because service-to-service clients, debugging, performance work, and policy checks sometimes need to know whether HTTP/1.x or HTTP/2 was actually used.
- **Expose backend transport types directly**
  - Rejected because it would reintroduce the `rust::reqwest`-shaped leakage this RFC is trying to remove.

## Drawbacks

- HTTP is a deceptively broad domain, and the API can sprawl if the module tries to cover every advanced transport concern immediately.
- Timeout, retry, redirect, and status behavior need very careful wording or users will make conflicting assumptions.
- Protocol negotiation adds visible surface area before every implementation can support every protocol.
- Streaming and transport seams are easy to over-design if they are not tied to concrete tests and `std.web` integration cases.
- Redaction rules and debug output need discipline or the module will create accidental secret leakage.

## Implementation architecture

*(Non-normative.)* A practical implementation likely uses a Rust-native HTTP stack underneath, but the public contract should remain request- and response-shaped. A sensible rollout would start with one-shot requests, explicit request objects, reusable clients, structured errors, timeouts, protocol metadata, and `JsonValue` helpers before expanding into richer transport features such as multipart, streaming bodies, cookie persistence, or HTTP/2 enforcement.

## Layers affected

- **Stdlib / runtime**: must provide the request, response, method, body, client, and error surfaces promised by this RFC.
- **Language surface**: the module and its helper types must be available as specified.
- **Execution handoff**: implementations must preserve timeout, retry, protocol, status, and decoding semantics without leaking backend-specific APIs as the public contract.
- **Docs / tooling**: examples and documentation must standardize safe defaults, explicit status handling, and redaction expectations.

## Unresolved questions

- Should `std.http` expose a default timeout at the module or client level, or should callers be required to choose one explicitly?
- Should timeout policy start as one total timeout, or should v1 expose connect/read/write/overall timeout fields immediately?
- Should `Response.json()` standardize only `JsonValue` decoding in this RFC, or should typed model decoding be part of the base contract too?
- Which redirect policy should be the default: follow a bounded number of redirects, or require explicit opt-in?
- Should retry policies live on `Request`, `Client`, or both?
- Should protocol policy live on `Request`, `Client`, or both?
- Should HTTP/2 support be a v1 implementation feature, a v1 API shape with optional backend support, or a follow-up RFC?
- What is the minimum useful test transport: synthetic responses only, local `std.web` app transport, or a trait-like transport provider surface?
- What streaming body API is small enough for v1 while still compatible with large downloads and uploads later?
- Which response projections should be standardized for typed actions, pipeline steps, logs, and test assertions?
- Should pagination and polling helpers live in `std.http`, in workflow/step libraries, or in API-specific packages?
- How much of cookie handling belongs in the initial contract versus a follow-up RFC?
- Which authentication helper shapes should accept `SecretStr` and `SecretBytes` directly in v1?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
