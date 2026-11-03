# RFC 066: `std.http` — Incan-first HTTP client and request/response surface

- **Status:** Planned
- **Created:** 2026-04-16
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 022 (namespaced stdlib modules and compiler handoff)
    - RFC 023 (compilable stdlib and Rust module binding)
    - RFC 037 (native web stdlib redesign)
    - RFC 051 (`JsonValue` for `std.json`)
    - RFC 055 (`std.fs` path-centric filesystem APIs)
    - RFC 063 (`std.process` process spawning and command execution)
    - RFC 019 (test runner and fixtures)
    - RFC 078 (tool execution and typed workflow actions)
    - RFC 093 (`std.telemetry` and OpenTelemetry observability)
    - RFC 094 (context managers and scoped cleanup)
    - RFC 103 (`std.secrets` secret strings and bytes)
    - RFC 142 (async composition, iteration, and streams)
    - RFC 146 (typed JSON decoding and complete validation)
- **Issue:** <https://github.com/encero-systems/incan/issues/84>
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC proposes `std.http` as Incan's standard library module for explicit HTTP client work. The module standardizes a request or response model, one-shot and client-based request APIs with one async contract and a blocking facade over the same types, client lifecycle through RFC 094 scopes, protocol negotiation, bounded default timeouts, opt-in retries with backoff, opt-in redirects, streaming bodies as RFC 142 streams, multipart bodies, an explicit cookie jar, structured errors, JSON convenience surfaces, response summaries for workflows, and a test transport with a provided fixture, so ordinary programs, tools, and automation workflows do not need to fall through to `rust::reqwest`-shaped APIs or ad hoc wrappers.

## Core model

Read this RFC as one foundation plus five mechanisms:

1. **Foundation:** HTTP is a general-purpose stdlib capability, not a CI-only or framework-only helper surface.
2. **Mechanism A:** `std.http` provides explicit `Request`, `Response`, `Body`, `Method`, and `HttpError` types with predictable behavior and no panic-driven network contract.
3. **Mechanism B:** the module supports both one-shot convenience helpers and a reusable `Client` surface so simple scripts and heavier integrations share one coherent model. Requests are async; `std.http.blocking` offers the same model with blocking calls.
4. **Mechanism C:** client lifecycle and pooling are explicit enough that repeated calls do not depend on hidden global connection state; clients and streamed responses are RFC 094 scopes.
5. **Mechanism D:** JSON, timeout, redirect, and retry behavior remain explicit policy surfaces rather than ambient magic, with bounded timeouts by default and redirects and retries off until requested.
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
- timeout policy should be present by default, with separate connect, read, write, and total limits
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

### pytest-httpx lessons

pytest-httpx provides an `httpx_mock` fixture that intercepts every HTTPX request in a test, fails requests nobody declared a response for, and by default fails the test when a declared response was never requested. Testing code that makes HTTP calls then needs no wiring and cannot reach the network by accident.

Source: [pytest-httpx](https://colin-b.github.io/pytest_httpx/).

The Incan lesson is that a provided fixture belongs with the HTTP module, scoped to one test, with the same checks.

## Goals

- Provide a first-class `std.http` module for client-side HTTP work.
- Standardize explicit request and response types rather than centering shell calls or Rust interop.
- Keep timeout behavior first-class and non-ambient.
- Define a structured `HttpError` model so network failures, status failures, timeout failures, decoding failures, and policy failures are distinguishable.
- Provide JSON convenience helpers that compose cleanly with RFC 051 `JsonValue`.
- Support both one-shot request helpers and a reusable `Client` surface.
- Make `Client` lifecycle, cleanup, and reuse explicit enough to support connection pooling without hidden globals.
- Make negotiated HTTP protocol information visible on responses, while avoiding a requirement that every backend support HTTP/2.
- Keep request and response models structured enough to compose with typed workflow actions, pipeline steps, logs, tests, and generated reports.
- Make retry behavior explicit and policy-shaped rather than automatic and invisible, with jittered exponential backoff that respects the overall deadline.
- Provide streaming bodies, multipart bodies, an explicit cookie jar, and test transports without leaking backend-specific transport types.
- Provide one async request contract and a blocking facade over the same types.
- Provide a test-only fixture that routes every request in a test through a mock transport.
- Require safe default treatment of sensitive headers in diagnostics and debug-facing representations.
- Accept secret value types for authentication and header-building APIs so callers do not need to reveal tokens into plain strings before sending requests.

## Non-Goals

- Defining server-side HTTP routing or handler APIs here; that belongs with RFC 037 and related web-platform work.
- Shipping a full browser fetch surface in this RFC; browser-oriented HTTP behavior may reuse the same contract but is not defined here.
- Making HTTP a language intrinsic or keyword surface.
- Introducing a GitHub- or cloud-specific SDK into the standard library.
- OAuth and other token-acquisition flows; auth libraries obtain tokens, and `std.http` sends them through its authentication helpers.
- WebSockets, which change protocol after the upgrade and need their own contract.
- HTTP/3.
- Service-specific pagination, polling, or SDKs; API packages build those on `std.http` primitives.
- Requiring HTTP/2 support from every implementation.

## Guide-level explanation

### One-shot requests

For simple scripts, users should be able to write:

```incan
from std.http import HttpError, get

async def main() -> Result[None, HttpError]:
    response = (await get("https://api.example.com/health"))?
    println(response.text()?)
    return Ok(None)
```

Requests are async: they wait on the network, and RFC 142 streams carry their bodies. A program that does not use async calls the same API through `std.http.blocking`, which takes and returns the same types:

```incan
from std.http.blocking import get

response = get("https://api.example.com/health")?
text = response.text()?
```

Ordinary request code stays inside `std.http`, uses `Result[..., HttpError]`, and does not require dropping into `rust::`.

### Explicit requests

For more control, users should be able to build a request directly:

```incan
from std.http import Auth, Body, Method, Request, Timeout, send

request = Request(
    method=Method.POST,
    url="https://api.example.com/events",
    headers={"Content-Type": "application/json"},
    auth=Auth.bearer(token),
    body=Body.json(payload),
    timeout=Timeout(total=10.0),
)

response = (await send(request))?
```

This makes policy visible:

- the method is explicit
- the body is explicit
- the timeout is explicit
- the caller chooses whether to inspect status, body bytes, text, or JSON

### Reusable clients

For workflows that share headers, retries, or transport settings, users should be able to use a `Client`. A client owns a connection pool, so it is an RFC 094 scope that closes its pool when the block ends:

```incan
from std.http import Auth, Client, RetryPolicy

async with Client(auth=Auth.bearer(token), retry=RetryPolicy.transient()) as client:
    response = (await client.get("https://api.example.com/items"))?
    items = response.json()?
```

Outside a `with` block, `client.close()` releases the pool explicitly, and dropping the client releases it too. The blocking client uses a plain `with`. One-shot helpers are still valuable for scripts and probes, but repeated calls, service-to-service integrations, crawlers, SDKs, and long-running tools should use one client rather than create a transport stack in a hot loop.

### Timeouts

Every client has a bounded default timeout, so no request can hang forever by omission:

```incan
client = Client()                                    # connect 10 s, read 30 s, write 30 s, total 60 s
client = Client(timeout=Timeout(total=120.0))        # change one limit, keep the others
response = (await client.get(url, timeout=Timeout(connect=2.0)))?
client = Client(timeout=Timeout.unbounded())         # only ever on purpose
```

- `connect` covers DNS, TCP, and TLS.
- `read` and `write` are the longest silence allowed while receiving or sending, so a slow but steady 2 GB download never trips them.
- `total` covers the whole exchange, and it does not restart on redirects or retries. For a streamed body, `total` stops when the response headers arrive, and `read` guards the rest.

An RFC 142 `deadline=` shares one absolute budget across several requests. For comparison, HTTPX defaults to 5 s for every phase, which is often too tight, while requests, Go, and Node default to no limit and can hang forever.

### Retries and backoff

Retries are off until requested. `RetryPolicy.transient()` is the common choice:

```incan
client = Client(retry=RetryPolicy.transient())
client = Client(retry=RetryPolicy(max_attempts=5, backoff=Backoff.exponential(base=1.0, cap=30.0)))
```

- `transient()` makes at most 3 attempts. It waits 0.5 s, then 1 s, then 2 s, doubling up to a 10 s cap, and adds random jitter to each wait so many clients do not retry in lockstep.
- A server's `Retry-After` on a 429 or 503 response is honored, up to the cap.
- Connection failures, timeouts before any response arrives, and the statuses 429, 502, 503, and 504 are retried.
- Only idempotent methods are retried: GET, HEAD, PUT, DELETE, and OPTIONS. A POST is retried only when it carries an idempotency key or is marked idempotent.
- Waits count against `total` and any `deadline`, and no retry starts that cannot finish in the remaining budget.

### Redirects

Redirects are not followed unless the caller asks:

```incan
client = Client(redirects=Redirects.follow(max_hops=10))
```

A followed redirect never carries credentials, cookies, or other sensitive headers to a different origin. The response records each hop.

### Protocol negotiation

HTTP/2 support is explicit without being mandatory for every implementation:

```incan
from std.http import Client, HttpProtocol

client = Client(protocol=HttpProtocol.Http2Preferred)
response = (await client.get("https://api.example.com/items"))?
println(response.protocol)
```

`HttpProtocol` has the cases `Auto`, `Http1Only`, `Http2Preferred`, and `Http2Required`. A client sets the default, and a request may override it. With `Http2Required`, a target or peer that cannot provide HTTP/2 produces a typed `HttpError`, never a silent downgrade.

### Status handling stays explicit

The response model does not hide status behavior behind panics. Users opt into strict status expectations:

```incan
response = (await client.get(url))?
response = response.require_success()?
data = response.json()?
```

or branch explicitly:

```incan
response = (await client.get(url))?

if response.status.is_success:
    return response.json()?
else:
    return Err(HttpError.unexpected_status(response.status))
```

### JSON

`response.json()` decodes into a `JsonValue`. Typed decoding composes with RFC 146, so a model's canonical validation runs:

```incan
weather = Weather.from_json(response.text()?)?
```

`Body.json(value)` accepts a `JsonValue` or any value whose type derives `json`.

### Streaming bodies

A buffered request returns after the whole body has been read. For large or open-ended bodies, `stream()` hands the body over as an RFC 142 async stream of bytes:

```incan
async with client.stream("GET", "https://downloads.example.com/archive.tar") as response:
    async for chunk in response.body()?:
        archive.write(chunk)?
```

The stream is bounded and applies backpressure: the client reads the next chunk only when the loop asks for it. Leaving the block releases the connection, even if the loop stopped early. A request body can likewise stream from an async source.

### Multipart forms and cookies

File uploads use multipart bodies:

```incan
body = Body.multipart([
    Part.text("title", "Quarterly report"),
    Part.file("report", Path("report.pdf"), content_type="application/pdf"),
])
```

There is no implicit cookie jar. A client that should keep cookies says so, and persisting them is a separate explicit choice:

```incan
client = Client(cookies=CookieJar.memory())
client = Client(cookies=CookieJar.file(Path("cookies.json")))
```

### Response summaries for workflows

A pipeline step, typed action, log, or test can record what happened without re-reading the body or exposing it. This follows Koheesio's step outputs:

```incan
summary = response.summary()
println(summary.status)          # 200
println(summary.protocol)        # HTTP/2
println(summary.final_url)       # after any followed redirects
println(summary.attempts)        # 2, after one retry
println(summary.elapsed)         # seconds for the whole exchange
println(summary.body_size)       # bytes, when known
```

The summary never includes the body or credentials. A step that needs the body asks for it explicitly with `text()`, `bytes()`, or `json()`.

### Sensitive data does not print carelessly

Headers such as `Authorization` are redacted in debug-facing output by default:

```incan
println(request)
```

does not dump bearer tokens or secrets into logs. When the caller uses `SecretStr` or `SecretBytes` from RFC 103, redaction comes from the value type as well as from conservative header-name rules: a header value derived from a secret wrapper stays redacted even if the header name is custom.

### Testing with the provided fixture

`std.http.testing` provides an `http_mock` fixture. For the length of a test, every client and one-shot helper sends through the mock, so the code under test needs no wiring:

```incan
from std.http import get
from std.http.testing import HttpMock
from std.testing import assert_eq, assert_true, fail

async def test_today(http_mock: HttpMock) -> None:
    http_mock.reply("GET", "https://api.example.com/today", status=200, json={"temp": 21})
    match await get("https://api.example.com/today"):
        Ok(response) => assert_eq(response.status.code, 200)
        Err(error) => fail(error.message())

async def test_slow_endpoint_times_out(http_mock: HttpMock) -> None:
    http_mock.fail_with_timeout("GET", "https://api.example.com/slow")
    match await get("https://api.example.com/slow"):
        Err(error) => assert_true(error.is_timeout())
        Ok(_) => fail("expected a timeout")
```

A request without a matching reply fails with a typed error and never reaches the network. A declared reply that the test never requested fails the test unless it is marked optional. Code that builds its own client can pass an explicit transport instead:

```incan
from std.http import Client
from std.http.testing import MockTransport
from std.testing import fixture

@fixture
def weather_api() -> Client:
    mock = MockTransport()
    mock.reply("GET", "https://api.example.com/today", status=200, json={"temp": 21})
    return Client(transport=mock)
```

Either way the test uses the same `Client`, `Request`, `Response`, and `HttpError` as production.

## Reference-level explanation

### Module surface

`std.http` must provide:

- `Method`, `Body`, `Part`, `Request`, `Response`, `StatusCode`, and `HttpError`
- `Client`
- `HttpProtocol` and the negotiated protocol recorded on each response
- `Timeout`, `RetryPolicy`, `Backoff`, and `Redirects`
- `Auth` helpers for bearer, basic, and sensitive-header credentials
- `CookieJar`
- `ResponseSummary`
- one-shot helpers `get`, `post`, `put`, `patch`, `delete`, `head`, `options`, and `send`

`std.http.blocking` must provide the same helpers and a blocking `Client` over the same types. `std.http.testing` must provide `MockTransport`, `HttpMock`, and the `http_mock` fixture.

The user-facing model is request- and response-centric rather than shell-centric or backend-centric.

### Async contract and the blocking facade

Every request operation of `std.http` must be async. The blocking facade must expose the same operations with blocking calls and must use the same `Request`, `Response`, `HttpError`, policy, and summary types; it must not be a second client model with its own feature set. A call into `std.http.blocking` inside an async function or method must be rejected with a diagnostic, because it would stall the other tasks on the executor. In the blocking facade, a streamed body is a synchronous fallible iterator of bytes; in `std.http` it is an RFC 142 `AsyncFallibleIterator[bytes, HttpError]`.

### Request model

A `Request` must carry:

- method
- URL
- headers, including credentials set through `Auth`
- query parameters if modeled separately from the URL
- body
- timeout policy and an optional RFC 142 `deadline`
- redirect, retry, and protocol policy when the caller overrides the client defaults

A request must be constructible without requiring a `Client`. A request's policy fields must override the client's defaults field by field: `Timeout(connect=2.0)` changes only the connect limit.

### Response model

A `Response` must expose:

- status code
- negotiated protocol version
- response headers
- the final URL and the redirect history
- body access: `bytes()`, `text()`, and `json()` on a buffered response, and `body()` on a streamed one
- `summary()`, returning a `ResponseSummary`

A buffered request must read the whole body before returning, within its timeouts. `ResponseSummary` must carry the method, the final URL with sensitive query values redacted, the redirect history, the status, the negotiated protocol, the number of attempts and the backoff waits, the elapsed time, the body size when known, and the content type. It must never carry body content, credentials, or sensitive header values.

A response must not panic on unsuccessful status codes. Status-based failure stays explicit through `require_success()` or equivalent APIs.

### Error model

`std.http` operations must return `Result[..., HttpError]`.

`HttpError` must distinguish at least:

- connection failures
- timeout failures, with the limit that expired
- redirect-policy failures
- TLS or transport failures
- unsupported or failed protocol negotiation
- decode failures
- explicit status-policy failures
- exhausted retries, carrying the last attempt's failure
- requests a mock transport has no reply for
- use of a closed client

It must provide predicates such as `is_timeout()` and must not collapse failures into one undifferentiated string.

### Client lifecycle

A `Client` owns reusable transport state, including its connection pool and cookie jar. `Client` must adopt RFC 094's `AsyncContextManager`, and the blocking client its `ContextManager`, closing the pool on exit. `close()` must release the pool explicitly; an operation on a closed client must fail with `HttpError` for a closed client. Dropping a client must release its resources. One-shot helpers must scope any temporary client state to the single request.

A streamed response must adopt the same protocols. Leaving its scope must release the connection, whether or not the body was read to the end.

### Timeouts

`Timeout` must have the limits `connect`, `read`, `write`, and `total`, in seconds. The client default must be `connect` 10 s, `read` 30 s, `write` 30 s, and `total` 60 s.

- `connect` must cover name resolution, the TCP connection, and the TLS handshake.
- `read` and `write` must bound the longest interval without progress while receiving or sending.
- `total` must bound the whole exchange of a buffered request, including redirects, retries, and backoff waits, and must not restart on any of them. For a streamed response, `total` must end when the response headers arrive; `read` then bounds each wait for the next chunk.

A request-level `Timeout` must override only the limits it sets. An RFC 142 `deadline` must bound the request in addition to its own limits. Disabling a limit must use `Timeout.unbounded()` or an explicit unbounded value for that limit; omission must never mean unbounded. An expired limit must produce a timeout `HttpError` naming that limit.

### Redirects

Redirects must not be followed by default. `Redirects.follow(max_hops=n)` must follow up to `n` redirects and must fail with a redirect-policy `HttpError` beyond that. A followed redirect to a different origin must drop credentials, cookies not scoped to the new origin, and every sensitive header. The response must record the final URL and each hop.

### Protocol policy

`HttpProtocol` must have the cases `Auto`, `Http1Only`, `Http2Preferred`, and `Http2Required`. The client sets the default, and a request may override it.

An implementation that cannot provide HTTP/2 may reject `Http2Preferred` up front, or accept it and fall back to HTTP/1.x. `Http2Required` must fail with a typed `HttpError` when the implementation, target, or peer cannot provide HTTP/2, and must never silently downgrade. Every response must record the protocol actually used. An implementation must report its protocol capabilities through diagnostics, so a program that requires HTTP/2 on a target without it is told before it runs where that can be determined.

### Retries and backoff

Retries must be opt-in. A `RetryPolicy` must specify the maximum attempts, the `Backoff`, the retryable statuses, and the retryable transport failures.

`RetryPolicy.transient()` must allow 3 attempts, use exponential backoff with full jitter starting at 0.5 s, doubling, and capped at 10 s, and retry connection failures, timeouts before any response, and the statuses 429, 502, 503, and 504. A `Retry-After` header on a 429 or 503 response must be honored up to the cap.

Only GET, HEAD, PUT, DELETE, and OPTIONS requests may be retried, unless a request carries an idempotency key or is explicitly marked idempotent. Backoff waits must count against `total` and any `deadline`, and a retry must not start when the remaining budget cannot cover it. A client may set a default policy, and a request may override it. The module must not retry any request by default.

### JSON integration

`Body.json(value)` must accept a `JsonValue` or a value whose type derives `json`. `Response.json()` must decode into `JsonValue`. Typed decoding into models must go through RFC 146 decoding, such as `Model.from_json(response.text()?)`, so canonical validation runs; `std.http` must not define a second typed decoder.

### Authentication and redaction

`Auth.bearer`, `Auth.basic`, and `Auth.header` must accept `SecretStr` or `SecretBytes` directly and must encode the secret only at the trusted wire sink.

Implementations must preserve RFC 103 secret non-disclosure in debug-facing request and response displays, errors, retry records, summaries, logs, telemetry, and receipts. Sensitive header values such as Authorization, Proxy-Authorization, and similarly sensitive token-bearing headers must be omitted or redacted on those surfaces. The HTTP wire boundary may intentionally transmit a secret through RFC 103's trusted encoding contract; that authorization does not make the plaintext safe for diagnostic reuse.

Header values constructed from RFC 103 `SecretStr` or `SecretBytes` must be treated as sensitive regardless of header name. Sensitive-header treatment by name must be conservative and documented. Header-name heuristics are a fallback; value-level secret typing is the stronger contract.

### Streaming bodies

A streamed response body must be an RFC 142 `AsyncFallibleIterator[bytes, HttpError]` with a bounded buffer: the client must not read further from the connection than the buffer allows until the consumer advances. A request body may stream from an async source of bytes under the same bounds. A caller may set a maximum body size, and exceeding it must fail with a typed error. Cleanup must follow the response's scope, as specified under client lifecycle.

### Multipart bodies

`Body.multipart(parts)` must build a `multipart/form-data` body from `Part.text(name, value)` and `Part.file(name, path, content_type=...)` parts, streaming file parts rather than loading them whole. Part names and file names must be encoded safely.

### Cookies

A client must not store cookies unless it was given a `CookieJar`. `CookieJar.memory()` must keep cookies for the client's lifetime, and `CookieJar.file(path)` must persist them explicitly. A jar must apply the standard domain, path, expiry, `Secure`, and `HttpOnly` rules and must never send a cookie to a host it is not scoped to. Cookie values must be treated as sensitive in every debug-facing surface.

### Test transports

A `Client` must accept a `transport`. `MockTransport` must return declared synthetic responses and controlled transport failures, such as timeouts, without network access, and must fail every request it has no declared reply for with a typed error. Selecting a mock transport must never fall back to the real network.

The `http_mock` fixture must be available only in test code. For the duration of a test that requests it, every `Client` and one-shot helper must send through the fixture's mock transport. At the end of the test, a declared reply that was never requested must fail the test unless it was declared optional.

Any other transport, such as one that serves a local `std.web` application, must implement the same transport seam and preserve `Request`, `Response`, `HttpError`, timeout, protocol, redaction, and policy semantics. Backend-specific transport handles must not become the public API.

### Telemetry

HTTP client spans and metrics must follow RFC 093's per-module instrumentation policy and OpenTelemetry's HTTP client conventions, with RFC 103 redaction applied to every attribute.

### Pagination and workflow composition

`std.http` does not standardize pagination or polling. API packages build them on its primitives, typically as RFC 142 async sources that fetch the next page on demand. Pipeline or workflow integrations must depend on `std.http` request, response, and summary models, not backend transport objects.

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

- **RFC 019 (test runner)**: `http_mock` is an ordinary fixture, injected by parameter name, and exists only in test code.
- **RFC 051 (`JsonValue`)**: `Response.json()` and `Body.json()` use `JsonValue` as the dynamic JSON type.
- **RFC 055 (`std.fs`)**: multipart file parts and streamed downloads compose with `std.fs` paths and files.
- **RFC 063 (`std.process`)**: HTTP stays a direct network API, not a wrapper over shelling out to `curl`.
- **RFC 037 (native web stdlib redesign)**: this RFC covers client-side HTTP. Server-side web contracts remain separate even if they share types such as methods or status codes, and a local `std.web` application can serve tests through the transport seam.
- **RFC 078 (tool execution and typed workflow actions)**: HTTP-capable tools and actions surface network access, protocol policy, and remote data flow through action metadata, policy checks, and response summaries.
- **RFC 093 (`std.telemetry`)**: client spans and metrics follow its per-module instrumentation policy.
- **RFC 094 (context managers)**: clients and streamed responses are scopes that release their resources on exit.
- **RFC 103 (`std.secrets`)**: authentication helpers, header builders, diagnostics, retries, summaries, telemetry, and workflow output preserve `SecretStr` and `SecretBytes` redaction.
- **RFC 142 (async streams)**: request operations are async, bodies stream as its fallible async iterators, and its `deadline` bounds requests.
- **RFC 146 (typed JSON decoding)**: typed response decoding goes through its `from_json` decoders and their canonical validation.

### Compatibility / migration

This feature is additive. Existing Rust-interop HTTP wrappers remain valid, but new code, docs, and examples should prefer `std.http`.

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
    - Rejected because HTTP is a general-purpose stdlib capability. Step and workflow libraries compose over `std.http`; they do not own the base transport contract.
- **Separate public sync and async client models**
    - Rejected because two client hierarchies drift apart, as HTTPX's `Client` and `AsyncClient` show. One async contract plus a blocking facade over the same types keeps one model.
- **Async only, with no blocking calls**
    - Rejected because simple scripts would have to become async programs to make one request.
- **No default timeout**
    - Rejected because a request can then hang forever by omission, as with requests, Go, and Node.
- **Follow redirects and retry by default**
    - Rejected because both repeat or redirect side effects and credentials without the caller asking.
- **Mandatory HTTP/2**
    - Rejected because the API should not block on backend coverage or target support. Protocol policy and negotiated protocol metadata have a place to live instead.
- **Hide protocol version entirely**
    - Rejected because service-to-service clients, debugging, performance work, and policy checks sometimes need to know whether HTTP/1.x or HTTP/2 was actually used.
- **Expose backend transport types directly**
    - Rejected because it would reintroduce the `rust::reqwest`-shaped leakage this RFC is trying to remove.
- **Patch the network globally for tests without a fixture**
    - Rejected because tests would change behavior through hidden state. The `http_mock` fixture scopes the mock to one test and exists only in test code.

## Drawbacks

- HTTP is a deceptively broad domain, and the API can sprawl if the module tries to cover every advanced transport concern.
- Timeout, retry, redirect, and status behavior need very careful wording or users will make conflicting assumptions.
- The default timeout values will be wrong for some workloads, which must override them.
- Protocol negotiation adds visible surface area even though not every implementation supports every protocol.
- The blocking facade is a second calling style that must stay in step with the async one.
- Redaction rules and debug output need discipline or the module will create accidental secret leakage.

## Implementation architecture

*(Non-normative.)* A practical implementation uses a Rust-native HTTP stack underneath through Rust interop, while the public contract remains request- and response-shaped. The blocking facade can drive the async client on a dedicated runtime.

## Layers affected

- **Stdlib / runtime**: `std.http`, `std.http.blocking`, and `std.http.testing` must provide the request, response, client, policy, summary, cookie, multipart, streaming, and error surfaces promised by this RFC.
- **Typechecker / diagnostics**: calls into `std.http.blocking` inside async functions must be rejected.
- **Test runner**: the `http_mock` fixture must scope its transport to one test and report unrequested replies.
- **Execution handoff**: implementations must preserve timeout, retry, protocol, status, and decoding semantics without leaking backend-specific APIs as the public contract.
- **Docs / tooling**: examples and documentation must standardize safe defaults, explicit status handling, and redaction expectations.

## Design Decisions

- Clients have a bounded default timeout: connect 10 s, read and write 30 s of inactivity, and total 60 s, which covers redirects, retries, and backoff and stops at the headers for a streamed body. Requests override individual limits; unbounded operation is only ever explicit, and an RFC 142 `deadline` shares one budget across requests.
- Typed JSON decoding composes through RFC 146's `from_json`; `Response.json()` returns `JsonValue`.
- Redirects are off by default. `Redirects.follow(max_hops=n)` enables them, and credentials and sensitive headers never cross to another origin.
- Retries are opt-in, set as a client default with per-request overrides. `RetryPolicy.transient()` makes 3 attempts with jittered exponential backoff from 0.5 s, capped at 10 s, honoring `Retry-After`, retrying only idempotent requests, and staying within the total timeout and deadline.
- Protocol policy is `HttpProtocol` (`Auto`, `Http1Only`, `Http2Preferred`, `Http2Required`), a client default with per-request overrides. `Http2Required` fails with a typed error rather than downgrading, and every response records the protocol used.
- HTTP/2 is part of the contract, with capability diagnostics; not every target must implement it.
- Tests use a checked transport seam: `MockTransport` for explicit clients, and the provided test-only `http_mock` fixture that routes every client in a test through a mock, never falls back to the network, and fails on unrequested replies. A local `std.web` transport implements the same seam.
- Bodies stream as bounded RFC 142 `AsyncFallibleIterator[bytes, HttpError]` streams with backpressure, released when their scope ends.
- `Response.summary()` returns status, protocol, final URL, redirect history, attempts and waits, elapsed time, body size, and content type, never body content or credentials, following Koheesio's step outputs.
- Pagination and polling belong to API packages, built as RFC 142 async sources.
- There is no implicit cookie jar; `CookieJar.memory()` and `CookieJar.file(path)` are explicit choices.
- `Auth.bearer`, `Auth.basic`, and `Auth.header` accept `SecretStr` or `SecretBytes` and encode only at the wire.
- Request operations are async, and `std.http.blocking` offers the same model with blocking calls, refused inside async functions.
- Clients and streamed responses are RFC 094 scopes; client telemetry follows RFC 093.
- Multipart bodies are part of the contract. OAuth flows, WebSockets, and HTTP/3 are not.
