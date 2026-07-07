# RFC 037: native web stdlib redesign

- **Status:** Planned
- **Created:** 2026-03-07
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 023 (Compilable stdlib and `rust.module` handoff)
    - RFC 027 (Vocabulary/desugaring infrastructure)
    - RFC 031 (Library system)
    - RFC 035 (First-class named function references)
    - RFC 036 (User-defined decorators)
    - RFC 066 (`std.http` HTTP client surface)
- **Issue:** [#171](https://github.com/encero-systems/incan/issues/171)
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

Redesign Incan's `std.web` platform so the stdlib can provide HTTP applications and APIs cleanly.

This RFC defines the intended end-state for `std.web`: Incan's standard-library surface for serving HTTP applications and APIs.

The developer experience should be native to Incan rather than shaped by backend interop constraints. The primary server-side experience is FastAPI-like: `app = App()`, `@app.get(...)`, typed parameters, plain return values, framework-owned serialization, and first-class platform features such as auth, middleware, validation, lifecycle, and docs. The platform should also support a Django-style organization layer and a declarative DSL when those provide real ergonomic value.

This RFC is intentionally an umbrella design RFC for the serving side. It defines the product shape, semantics direction, capability boundaries, and migration goals for `std.web`. If a sub-area later proves deep enough to require its own precise RFC, that follow-up RFC should refine this design rather than replace it.

## Motivation

The current `std.web` proves that Incan can compile web programs, but it does not yet provide a native, coherent API platform experience.

Current problems:

- Users still encounter backend leakage such as explicit wrapper types and backend-oriented handoff details.
- Routing behavior is split across compiler logic, macros, and runtime helpers.
- The platform does not yet present one complete framework story for auth, middleware, validation, docs, and lifecycle.

The goal is not just "web works." The goal is that Incan developers can expose networked applications and APIs with one coherent server-side mental model.

This means:

- serving APIs must feel native
- schemas, validation, errors, auth, and docs must compose cleanly
- the compiler core should provide primitives, while framework ownership lives in stdlib and libraries

## Goals

- Provide a native `std.web` surface for serving HTTP applications and APIs.
- Center the server model on `App`-owned routing, typed handlers, request extraction, response conversion, middleware, lifecycle, auth, validation, and docs metadata.
- Make backend framework details an implementation concern rather than the public API contract.
- Keep `std.web` compatible with RFC 066 where server and client concepts overlap, without making RFC 037 responsible for the `std.http` client surface.
- Leave room for Django-style organization, declarative route DSLs, and future transport facades as secondary surfaces over the same serving model.

## Non-Goals

- Defining the `std.http` client API, retry model, redirect behavior, timeout policy, or response decoding surface; those belong to RFC 066.
- Defining exact grammar for every future DSL form.
- Defining wire-level details for gRPC, Arrow-oriented data transports, or other future protocols.
- Mandating the exact implementation strategy inside the compiler, stdlib runtime, or backend framework bridge.

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

### Boundary with `std.http`

`std.http` is owned by RFC 066. This RFC does not define the HTTP client API, retry model, redirect behavior, timeout policy, or response decoding surface.

The serving side should still align with `std.http` where that improves the user experience:

- shared schema and validation conventions
- shared auth building blocks
- shared error and serialization conventions
- compatible request/response vocabulary where appropriate

Alignment does not require identical syntax or a shared implementation schedule. It requires that `std.web` does not make choices that block `std.http` from providing a coherent client-side model later.

### Optional organizational surfaces

The primary model should remain FastAPI-like, but the platform may also offer:

- a Django-style organizational layer for larger projects
- a declarative DSL for route or service declarations

These should be facades over the same underlying platform model, not separate frameworks with separate semantics.

## Reference-level explanation (precise rules)

### Scope

This RFC covers the end-state design for Incan's standard-library web-serving platform.

In scope:

- `std.web` for serving HTTP applications and APIs
- shared web-serving concepts for schemas, auth, middleware, validation, docs, and lifecycle
- the extension boundary for future transports such as gRPC and Arrow-oriented data/RPC scenarios

Out of scope for this RFC:

- `std.http` client APIs, timeout/retry semantics, redirect behavior, and response decoding; these belong to RFC 066
- exact grammar for every future DSL form
- exact wire-level details for gRPC or Arrow integrations
- exact implementation strategy inside the compiler/runtime

Those may be refined later if needed, but the end-state described here is the target they must fit.

### Design constraints

1. **FastAPI-first server UX:** the primary serving experience is `App`-owned, decorator-driven, and typed.
2. **Complete server platform scope:** serving, auth, validation, middleware, lifecycle, and docs are all part of the intended platform, not optional afterthoughts.
3. **No backend leakage in public APIs:** users should not have to think in backend-runtime terms for ordinary API work.
4. **One platform, multiple surfaces:** FastAPI-style APIs, Django-style organization, and DSLs must reduce to one coherent underlying model.
5. **Library ownership over compiler ownership:** framework behavior belongs in stdlib/libraries; compiler support should remain primitive and general where possible.
6. **Client compatibility without client ownership:** `std.web` should align with RFC 066 where concepts overlap, but must not own the `std.http` implementation schedule.
7. **HTTP is the primary baseline:** other transports may extend the model, but HTTP remains the core target.

### Platform capabilities

The redesigned platform must support all of the following capabilities as part of the intended end-state:

- routing and endpoint registration
- request extraction and response conversion
- schema and validation conventions
- authentication and authorization
- middleware pipelines
- request context and dependency injection
- application lifecycle hooks
- standardized error modeling
- documentation and OpenAPI-style metadata

### Canonical concepts

The platform should converge on a shared conceptual model, even if the exact runtime representation evolves over time.

Key concepts include:

- `App`: a serving application that owns routes, middleware, policies, docs metadata, and startup behavior
- `Route`: a typed handler bound to a path/method combination and associated metadata
- `Schema`: a type-level contract used for validation, serialization, and docs
- `AuthProvider`: a component that establishes identity or credentials
- `Guard`: a component that decides whether a request may proceed
- `Context`: request-scoped state visible to handlers and middleware
- `Middleware`: server-side ordered behavior around request handling

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

### Shared semantics with `std.http`

RFC 066 owns the `std.http` client contract. `std.web` should align with it on the following where useful:

- schema and validation conventions
- auth primitives and token/session building blocks
- standardized error surfaces
- documentation and metadata vocabulary

Alignment does not require identical syntax; it requires a coherent mental model across server and client libraries without coupling their release schedules.

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

- deterministic middleware ordering
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

HTTP serving is the primary target of this RFC.

However, the platform should be designed so that future RFCs can extend it toward:

- gRPC-style service transports
- Arrow-oriented data and RPC transports

The intent is not to force all transports into one fake-HTTP abstraction. The intent is to avoid designing the `std.web` platform in a way that blocks adjacent transports later.

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

Splitting "routing," "auth," and "docs" into separate RFCs too early would risk designing them in isolation and then stitching together a platform after the fact. That is exactly what this RFC is trying to avoid for `std.web`.

At the same time, follow-up RFCs are still appropriate when a sub-area needs deeper precision. The rule should be:

- do not split merely to split
- do split when a sub-area needs a real semantic deep dive

### Follow-up RFC boundary

If follow-up RFCs are needed later, they should refine one of these areas:

- auth/security semantics
- docs/schema generation rules
- transport-specific integrations such as gRPC
- declarative DSL syntax and desugaring

They should inherit this RFC's product direction rather than reopen it from scratch.

## Alternatives considered

- **Stay with the current hybrid model:** rejected; too much leakage and too little coherence.
- **Let `std.web` ignore `std.http` entirely:** rejected; server-side choices around schemas, errors, auth metadata, and docs should stay compatible with RFC 066 where concepts overlap.
- **Design only the routing core now:** rejected; the platform problem is broader than routing.
- **Make the compiler own the web framework semantics:** rejected; that would work against stdlib/library evolution.
- **Make Django-style organization the primary model:** rejected; FastAPI-style ergonomics are the better default for modern API development.

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

### Outcome B — Security and policy

Incan's web platform has first-class auth and policy support.

This includes:

- sessions and/or token-based auth
- guards and policies
- route/group/app-level enforcement
- auth metadata that composes with docs and tooling

### Outcome C — Contracts, validation, and documentation

Incan web APIs have a coherent contract story.

This includes:

- schema-aware validation and serialization
- standardized error contracts
- docs/OpenAPI-style output for HTTP APIs
- compatibility with RFC 066 where server and client contracts overlap

### Outcome D — Organization and DSLs

The platform can support richer organizational layers without redesigning the foundations.

This includes:

- Django-style project organization where useful
- declarative DSLs where they improve clarity

### Outcome E — Advanced transports

The platform can extend beyond HTTP without redesigning the foundations.

This includes:

- future gRPC integrations built on compatible concepts
- future Arrow-oriented data and RPC integrations built on compatible concepts

## Layers affected

- **Stdlib / runtime**: must provide the HTTP-serving surface that this redesign standardizes, without leaking backend crate APIs as the primary contract.
- **Language surface**: the web-platform surface must be recognized and validated coherently across serving, routing, and request/response types.
- **Execution handoff**: implementations must preserve the language-level web semantics while mapping onto the chosen runtime substrate underneath.
- **Docs / tooling**: the relationship between platform primitives, decorators, routing, validation, and future transport extensions must be explained clearly.

## Design Decisions

1. The minimum complete `std.web` platform includes route- and group-level auth requirements, reusable guards and policies, request-scoped identity access, session- or token-oriented flows, and auth metadata that composes with docs and tooling. The exact API spelling may be refined in follow-up work, but auth is part of the platform rather than a later utility layer.
2. Validation should be split by responsibility: compile-time validation should cover names, signatures, decorator shape, metadata shape, and other statically knowable contracts; runtime validation should handle request payloads, path/query values, auth decisions, and serialization/deserialization failures that depend on incoming data.
3. Docs and schema generation should infer contracts from checked handler signatures, models, auth metadata, and response declarations. Explicit metadata should be available for summaries, tags, examples, security requirements, and cases where inference would be ambiguous or misleading.
4. Django-style organization is a secondary surface, not the primary model. `std.web` should own the serving primitives and any lightweight grouping concepts needed by the core platform; larger project-layout conventions may live in higher-level libraries unless they prove necessary as standard-library facades over the same `App` model.
5. RFC 037 owns HTTP serving. Future gRPC, Arrow-oriented, or other transport RFCs may reuse concepts such as `App`, `Route`, `Schema`, `Guard`, and lifecycle hooks, but must not be forced into a fake-HTTP abstraction or make RFC 037 responsible for their wire-level details.
