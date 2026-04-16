# RFC 037: native web and HTTP stdlib redesign

- **Status:** Draft
- **Created:** 2026-03-07
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 023 (Compilable stdlib and `rust.module` handoff)
    - RFC 027 (Vocabulary/desugaring infrastructure)
    - RFC 031 (Library system)
    - RFC 035 (First-class named function references)
    - RFC 036 (User-defined decorators)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/329
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

Redesign Incan's API platform so the stdlib can both provide APIs cleanly and consume APIs cleanly.

This RFC defines the intended end-state for two closely related surfaces:

- `std.web` for serving HTTP applications and APIs
- `std.http` for calling external HTTP and REST APIs

The developer experience should be native to Incan rather than shaped by backend interop constraints. The primary server-side experience is FastAPI-like: `app = App()`, `@app.get(...)`, typed parameters, plain return values, framework-owned serialization, and first-class platform features such as auth, middleware, validation, lifecycle, and docs. The platform should also support a Django-style organization layer and a declarative DSL when those provide real ergonomic value.

This RFC is intentionally an umbrella design RFC. It defines the product shape, semantics direction, capability boundaries, and migration goals for the web/http platform. If a sub-area later proves deep enough to require its own precise RFC, that follow-up RFC should refine this design rather than replace it.

## Motivation

The current `std.web` proves that Incan can compile web programs, but it does not yet provide a native, coherent API platform experience.

Current problems:

- Users still encounter backend leakage such as explicit wrapper types and
backend-oriented handoff details.
- Routing behavior is split across compiler logic, macros, and runtime helpers.
- The platform does not yet present one complete framework story for auth,
middleware, validation, docs, and lifecycle.
- There is no equally clean standard-library story for consuming REST APIs from Incan code.

The goal is not just "web works." The goal is that Incan developers can build networked applications and APIs end to end, with one coherent mental model.

This means:

- serving APIs must feel native
- consuming APIs must feel native
- schemas, validation, errors, auth, and docs must compose cleanly across both
- the compiler core should provide primitives, while framework ownership lives in stdlib and libraries

## Guide-level explanation (how users think about it)

### Providing an API with `std.web`

The primary experience should feel close to FastAPI:

```incan
from std.web import App

app = App()

@app.get("/")
async def index() -> dict[str, str]:
    return {"message": "Hello World"}

@app.get("/users/{id}")
async def get_user(id: int) -> User:
    return load_user(id)?

@app.post("/users")
async def create_user(body: CreateUser) -> User:
    return save_user(body)?

def main():
    app.run(port=8080)
```

From a user's point of view:

- route decorators are real decorators, not marker-only hacks
- handler signatures describe extraction behavior
- plain Incan values are returned, and the framework handles response conversion
- auth, validation, middleware, and docs are framework features, not ad hoc patterns

### Consuming an API with `std.http`

The outgoing side should feel equally native. `std.http` is a general-purpose HTTP client: fluent, composable, and suitable for any Incan code that needs to make HTTP calls:

```incan
from std.http import Client, ExponentialBackoff

client = Client(
    base_url="https://api.example.com",
    auth=BearerToken(token),
    retry=ExponentialBackoff(max=3, initial=2),
    timeout=10
)

# Single typed request
user = client.get(f"/users/{id}").json[User]()

# Paginated collection
users = client.get("/users").paginate(limit=100).json[List[User]]()

# POST with body
created = client.post("/users", body=CreateUser(name="Alice")).json[User]()
```

Error handling distinguishes transport failures, protocol failures (non-2xx), and decode/validation failures. Each is a distinct error type so callers can handle them precisely.

The key point is symmetry: Incan should be good at both sides of API work. A language that can expose an API but cannot cleanly consume one is still missing half the story.

### Shared concepts across both sides

The serving side and client side should reuse the same broad ideas where that improves the user experience:

- shared schema and validation conventions
- shared auth building blocks
- shared error and serialization conventions
- shared middleware/interceptor ideas where appropriate

They do not need identical APIs, but they should feel like parts of one platform.

### Optional organizational surfaces

The primary model should remain FastAPI-like, but the platform may also offer:

- a Django-style organizational layer for larger projects
- a declarative DSL for route or service declarations

These should be facades over the same underlying platform model, not separate frameworks with separate semantics.

## Reference-level explanation (precise rules)

### Scope

This RFC covers the end-state design for Incan's standard-library API platform.

In scope:

- `std.web` for serving HTTP applications and APIs
- `std.http` for calling HTTP and REST APIs
- shared platform concepts for schemas, auth, middleware, validation, docs, and lifecycle
- the extension boundary for future transports such as gRPC and Arrow-oriented data/RPC scenarios

Out of scope for this RFC:

- exact grammar for every future DSL form
- exact wire-level details for gRPC or Arrow integrations
- exact implementation strategy inside the compiler/runtime

Those may be refined later if needed, but the end-state described here is the target they must fit.

### Design constraints

1. **FastAPI-first server UX:** the primary serving experience is `App`-owned, decorator-driven, and typed.
2. **Complete platform scope:** serving, consuming, auth, validation, middleware, lifecycle, and docs are all part of the intended platform, not optional afterthoughts.
3. **No backend leakage in public APIs:** users should not have to think in
backend-runtime terms for ordinary API work.
4. **One platform, multiple surfaces:** FastAPI-style APIs, Django-style organization, and DSLs must reduce to one coherent underlying model.
5. **Library ownership over compiler ownership:** framework behavior belongs in stdlib/libraries; compiler support should remain primitive and general where possible.
6. **Symmetry matters:** the platform must be good at both exposing and calling APIs.
7. **HTTP is the primary baseline:** other transports may extend the model, but HTTP remains the core target.

### Platform capabilities

The redesigned platform must support all of the following capabilities as part of the intended end-state:

- routing and endpoint registration
- request extraction and response conversion
- schema and validation conventions
- authentication and authorization
- middleware/interceptor pipelines
- request context and dependency injection
- application lifecycle hooks
- standardized error modeling
- documentation and OpenAPI-style metadata
- outgoing HTTP client workflows

### Canonical concepts

The platform should converge on a shared conceptual model, even if the exact runtime representation evolves over time.

Key concepts include:

- `App`: a serving application that owns routes, middleware, policies, docs metadata, and startup behavior
- `Route`: a typed handler bound to a path/method combination and associated metadata
- `Client`: an outgoing HTTP client with configuration, auth, retry policy, pagination, interceptors, and typed response helpers
- `Schema`: a type-level contract used for validation, serialization, and docs
- `AuthProvider`: a component that establishes identity or credentials
- `Guard`: a component that decides whether a request may proceed
- `Context`: request-scoped state visible to handlers and middleware
- `Middleware`: server-side ordered behavior around request handling
- `Interceptor`: client-side or transport-neutral ordered behavior around request execution

This RFC intentionally names the concepts without freezing their exact internal implementation.

### Serving model (`std.web`)

The serving side is centered on `App`.

Expected semantics:

- route registration belongs to the app
- route decorators are real decorators or decorator factories
- handler signatures drive extraction behavior
- plain return values are legal; the framework owns coercion into concrete responses
- auth and middleware can be applied at app, group, or route scope
- docs metadata can be inferred from signatures and augmented explicitly

The primary user-facing default should be ergonomic API development, not low-level response plumbing.

### Client model (`std.http`)

The consuming side is centered on `Client`. `std.http` is intentionally general-purpose: the right primitive for any Incan code that needs to make HTTP calls, not a framework-specific abstraction.

Expected semantics:

- base URL, headers, auth, timeout, retry, and transport settings are client-level concepts
- fluent request building: `client.get(path).json[T]()`, `client.post(path, body=...).json[T]()`
- pagination is a first-class operation on the request chain: `.paginate(limit=n)`
- retry with configurable backoff is a client-level option, not middleware
- typed response decoding is first-class; the caller names the expected type
- error handling distinguishes transport failure, protocol failure, and
decode/validation failure, each as a distinct error type
- auth and interceptor concepts compose cleanly with request execution

The goal is not merely "HTTP requests are possible." The goal is that calling a REST API feels like standard language work rather than framework escape hatches.

**Boundary with higher-level libraries:** step-oriented HTTP abstractions such
as `HttpGetStep` and `PaginatedHttpGetStep` belong in purpose-built libraries, not `std.http`. Those exist for the data-pipeline mental model, "I need a step that fetches data from an API," and should be implemented as thin wrappers over
`std.http` primitives in whichever library needs them. `std.http` stays lean
and general; pipeline ergonomics live in the library that owns that concern.

### Shared semantics

The serving and consuming sides should align on the following where useful:

- schema and validation conventions
- auth primitives and token/session building blocks
- middleware/interceptor composition ideas
- standardized error surfaces
- documentation and metadata vocabulary

Alignment does not require identical syntax; it requires a coherent mental model.

### Authentication and authorization

Authentication and authorization are first-class platform capabilities, not peripheral utilities.

The platform must support:

- route-level and group-level auth requirements
- reusable guards and policies
- request-scoped identity/principal access
- session- and token-oriented flows
- auth metadata usable by docs and client tooling

This RFC does not yet fix the exact auth API surface, but it does fix that auth belongs in the platform design itself.

### Middleware, context, and lifecycle

The platform must support:

- deterministic middleware/interceptor ordering
- short-circuiting, enrichment, and error transformation
- request-scoped context
- dependency provision/override patterns
- startup and shutdown hooks
- background task and long-lived resource lifecycle integration

### Documentation and contracts

The platform must support:

- schema-aware request and response contracts
- OpenAPI-style docs generation for HTTP APIs
- explicit metadata for summaries, tags, examples, and security requirements
- stable error contract documentation where possible

### Transport extensions

HTTP is the primary target of this RFC.

However, the platform should be designed so that future RFCs can extend it toward:

- gRPC-style service transports
- Arrow-oriented data and RPC transports

The intent is not to force all transports into one fake-HTTP abstraction. The intent is to avoid designing the HTTP platform in a way that blocks adjacent transports later.

### Compatibility and migration

Migration from today's `std.web` should be progressive rather than disruptive.

Expected migration direction:

1. keep current `@route` behavior working during the transition
2. move the recommended path to `App`-owned route decorators and richer framework capabilities
3. provide compatibility paths for existing response/extractor patterns where reasonable
4. deprecate global `@route` once the native `App` model reaches practical parity

## Design details

### Primary and secondary surfaces

This RFC defines a primary surface and secondary surfaces.

Primary surface:

- FastAPI-like `std.web` for serving
- ergonomic `std.http` for consuming

Secondary surfaces:

- Django-style organizational layers
- declarative DSLs
- future transport-specific facades

The primary surface should be the reference mental model. Secondary surfaces should lower to it or align tightly with it, rather than growing independent semantics.

### Relationship to existing RFCs

- **RFC 023** establishes the current stdlib/runtime handoff baseline, but this RFC aims to remove more user-facing interop leakage from the final experience.
- **RFC 035** is important because function values are a natural part of handler, middleware, and decorator systems.
- **RFC 036** is foundational because proper decorators are central to the desired `@app.get(...)` model.
- **RFC 027** matters because future DSL forms should prefer vocab/desugaring over new compiler special-cases.
- **RFC 031** matters because long-term framework growth should live comfortably in the library ecosystem.

### Why this is one RFC

This RFC is broad on purpose because the problem is broad on purpose.

Splitting "routing," "auth," "docs," and "HTTP client" into separate RFCs too early would risk designing them in isolation and then stitching together a platform after the fact. That is exactly what this RFC is trying to avoid.

At the same time, follow-up RFCs are still appropriate when a sub-area needs deeper precision. The rule should be:

- do not split merely to split
- do split when a sub-area needs a real semantic deep dive

### Follow-up RFC boundary

If follow-up RFCs are needed later, they should refine one of these areas:

- auth/security semantics
- HTTP client semantics
- docs/schema generation rules
- transport-specific integrations such as gRPC
- declarative DSL syntax and desugaring

They should inherit this RFC's product direction rather than reopen it from scratch.

## Alternatives considered

- **Stay with the current hybrid model:** rejected; too much leakage and too little coherence.
- **Treat serving and consuming as unrelated stdlib features:** rejected; that would fragment the mental model.
- **Design only the routing core now:** rejected; the platform problem is broader than routing.
- **Make the compiler own the web framework semantics:** rejected; that would work against stdlib/library evolution.
- **Make Django-style organization the primary model:** rejected; FastAPI-style ergonomics are the better default for modern API development.
- **Model `std.http` as a step-oriented API (`HttpGetStep`, `PaginatedHttpGetStep`, etc.):** rejected for the stdlib. The step model is optimized for data pipeline thinking and belongs in a purpose-built library that can wrap `std.http` primitives. The stdlib should remain general-purpose.

## Drawbacks

- This RFC is intentionally broad, which means some sub-areas remain directional rather than fully specified.
- The end-state is ambitious and will take time to reach.
- Maintaining compatibility while reshaping the developer experience will require care.

## Outcome phases

These phases describe user-visible outcomes, not mandated implementation sequences.

### Outcome A — Native API serving

Incan can expose HTTP APIs cleanly through `std.web`.

This includes:

- `App`-owned route registration
- typed extraction
- plain return values
- framework-owned response conversion
- a serving experience that feels native rather than Rust-shaped

### Outcome B — Native API consumption

Incan can call HTTP and REST APIs cleanly through `std.http`.

This includes:

- standard client configuration
- typed response decoding
- auth-aware requests
- a clear error model for network/protocol/decode failures

### Outcome C — Security and policy

Incan's API platform has first-class auth and policy support.

This includes:

- sessions and/or token-based auth
- guards and policies
- route/group/app-level enforcement
- auth metadata that composes with docs and tooling

### Outcome D — Contracts, validation, and documentation

Incan APIs have a coherent contract story.

This includes:

- schema-aware validation and serialization
- standardized error contracts
- docs/OpenAPI-style output for HTTP APIs
- a consistent story between serving and consuming sides

### Outcome E — Organization and DSLs

The platform can support richer organizational layers without redesigning the foundations.

This includes:

- Django-style project organization where useful
- declarative DSLs where they improve clarity

### Outcome F — Advanced transports

The platform can extend beyond HTTP without redesigning the foundations.

This includes:

- future gRPC integrations built on compatible concepts
- future Arrow-oriented data and RPC integrations built on compatible concepts

## Layers affected

- **Stdlib / runtime**: must provide the HTTP-serving and HTTP-consuming surfaces that this redesign standardizes, without leaking backend crate APIs as the primary contract.
- **Language surface**: the web-platform surface must be recognized and validated coherently across serving, routing, request/response types, and client usage.
- **Execution handoff**: implementations must preserve the language-level web semantics while mapping onto the chosen runtime substrate underneath.
- **Docs / tooling**: the relationship between platform primitives, decorators, routing, validation, and future transport extensions must be explained clearly.

## Unresolved questions

1. What should the first-class `std.http` client ergonomics look like?
2. What is the minimum auth/authz surface that belongs in the first complete platform milestone?
3. How much validation should happen at compile time versus runtime?
4. Which parts of docs/schema generation should be implicit, and which should require explicit metadata?
5. Which Django-style organizational ideas belong in the stdlib, and which belong in higher-level libraries?
6. What is the right boundary between the HTTP platform and future gRPC or Arrow-oriented follow-up RFCs?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
