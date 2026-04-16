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
- **Issue:** https://github.com/dannys-code-corner/incan/issues/84
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC proposes `std.http` as Incan's standard library module for explicit HTTP client work. The module standardizes a request or response model, one-shot and client-based request APIs, timeout and retry policy, structured errors, and JSON convenience surfaces so ordinary programs, tools, and automation workflows do not need to fall through to `rust::reqwest`-shaped APIs or ad hoc wrappers.

## Core model

Read this RFC as one foundation plus three mechanisms:

1. **Foundation:** HTTP is a general-purpose stdlib capability, not a CI-only or framework-only helper surface.
2. **Mechanism A:** `std.http` provides explicit `Request`, `Response`, `Body`, `Method`, and `HttpError` types with predictable behavior and no panic-driven network contract.
3. **Mechanism B:** the module supports both one-shot convenience helpers and a reusable `Client` surface so simple scripts and heavier integrations share one coherent model.
4. **Mechanism C:** JSON, timeout, redirect, and retry behavior remain explicit policy surfaces rather than ambient magic.

## Motivation

Networking is a recurring boundary for ordinary Incan programs: API clients, release tooling, CI automation, ingestion pipelines, service-to-service calls, health checks, and migration scripts all need HTTP. Today, the practical escape hatch is Rust interop. That works, but it leaks Rust-shaped APIs, Rust-shaped errors, and inconsistent conventions into user code precisely where the standard library should provide one stable model.

This matters for more than ergonomics. HTTP boundaries are policy-heavy: timeouts, retries, redirect handling, header redaction, JSON decoding, and error reporting all need explicit behavior. If every project rebuilds these choices differently, the language ends up with a fragmented story for one of the most common integration surfaces.

`std.http` should therefore do for network requests what `std.fs`, `std.process`, and the newer stdlib RFCs are doing in their domains: define an Incan-first contract while still allowing the runtime to map onto Rust-native implementations underneath.

## Goals

- Provide a first-class `std.http` module for client-side HTTP work.
- Standardize explicit request and response types rather than centering shell calls or Rust interop.
- Keep timeout behavior first-class and non-ambient.
- Define a structured `HttpError` model so network failures, status failures, timeout failures, decoding failures, and policy failures are distinguishable.
- Provide JSON convenience helpers that compose cleanly with RFC 051 `JsonValue`.
- Support both one-shot request helpers and a reusable `Client` surface.
- Make retry behavior explicit and policy-shaped rather than automatic and invisible.
- Require safe default treatment of sensitive headers in diagnostics and debug-facing representations.

## Non-Goals

- Defining server-side HTTP routing or handler APIs here; that belongs with RFC 037 and related web-platform work.
- Shipping a full browser fetch surface in this RFC; browser-oriented HTTP behavior may reuse the same contract later but is not defined here.
- Making HTTP a language intrinsic or keyword surface.
- Introducing a GitHub- or cloud-specific SDK into the standard library.
- Standardizing cookies, OAuth flows, multipart forms, WebSockets, or HTTP/3-specific behavior in the first version.

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
- retry policy when the caller opts into retries

A request must be constructible without requiring a `Client`.

### Response model

A `Response` must expose:

- status code
- response headers
- body bytes
- helpers for decoding text and JSON

A response must not silently panic on unsuccessful status codes. Status-based failure should remain explicit through helpers such as `require_success()` or equivalent APIs.

### Error model

`std.http` operations must return `Result[..., HttpError]`.

`HttpError` must distinguish at least:

- connection failures
- timeout failures
- redirect-policy failures
- TLS or transport failures
- decode failures
- explicit status-policy failures

The module may include richer variants, but it must not collapse all failures into one undifferentiated string.

### Timeouts

Timeouts must be first-class and explicit. The contract must define:

- how request timeouts are attached
- whether a client-level timeout can be overridden per request
- what error variant a timeout produces

This RFC intentionally does not hardcode one exact default timeout yet; see unresolved questions.

### Retries

Retries must be opt-in and policy-shaped. A retry policy may cover:

- maximum attempts
- backoff strategy
- which status codes are retryable
- which transport failures are retryable

The module must not silently retry every request by default.

### JSON integration

`Body.json(value)` or an equivalent API may accept `JsonValue` and, where later RFCs standardize model-oriented JSON encoding, other serializable values.

`Response.json()` must decode into `JsonValue` at minimum. Typed decode into models may be added through compatible follow-up RFCs, but this RFC's floor is a coherent `JsonValue` path.

### Redaction and debug-facing behavior

Implementations should redact sensitive header values such as `Authorization`, `Proxy-Authorization`, and similarly sensitive token-bearing headers in debug-facing request or response displays.

The public contract does not need to prescribe every redacted header name exhaustively in v1, but it must require that sensitive-header treatment is conservative and documented.

## Design details

### Syntax

This RFC does not require new language syntax. It is a namespaced stdlib surface.

### Semantics

The semantic center is explicit network behavior:

- request creation is explicit
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

## Drawbacks

- HTTP is a deceptively broad domain, and the API can sprawl if the module tries to cover every advanced transport concern immediately.
- Timeout, retry, redirect, and status behavior need very careful wording or users will make conflicting assumptions.
- Redaction rules and debug output need discipline or the module will create accidental secret leakage.

## Implementation architecture

*(Non-normative.)* A practical implementation likely uses a Rust-native HTTP stack underneath, but the public contract should remain request- and response-shaped. A sensible rollout would start with one-shot requests, explicit request objects, reusable clients, structured errors, timeouts, and `JsonValue` helpers before expanding into richer transport features such as multipart, streaming bodies, or cookie persistence.

## Layers affected

- **Stdlib / runtime**: must provide the request, response, method, body, client, and error surfaces promised by this RFC.
- **Language surface**: the module and its helper types must be available as specified.
- **Execution handoff**: implementations must preserve timeout, retry, status, and decoding semantics without leaking backend-specific APIs as the public contract.
- **Docs / tooling**: examples and documentation must standardize safe defaults, explicit status handling, and redaction expectations.

## Unresolved questions

- Should `std.http` expose a default timeout at the module or client level, or should callers be required to choose one explicitly?
- Should `Response.json()` standardize only `JsonValue` decoding in this RFC, or should typed model decoding be part of the base contract too?
- Which redirect policy should be the default: follow a bounded number of redirects, or require explicit opt-in?
- Should retry policies live on `Request`, `Client`, or both?
- How much of cookie handling belongs in the initial contract versus a follow-up RFC?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
