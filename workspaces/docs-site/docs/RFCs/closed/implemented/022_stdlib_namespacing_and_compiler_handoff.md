# RFC 022: Namespaced stdlib modules and compiler→stdlib handoff

**Status:** Implemented **Created:** 2026-02-06 **Author(s):** Danny Meijer (@dannymeijer) **Related:** RFC 000 (core imports/modules), RFC 005 (Rust interop), RFC 051 (`JsonValue`), RFC 013 (Rust crate dependencies), RFC 020 (offline/locked policy) **Issue:** [#120](https://github.com/dannys-code-corner/incan/issues/120)

## Summary

This RFC proposes three related changes to reduce global-namespace pollution and decouple language evolution from compiler internals:

1. **A canonical Incan stdlib root**: stdlib modules live under `std` (e.g. `std.web`, `std.http`, `std.datetime`, `std.foo`, etc).
2. **Namespaced decorators**: decorators are resolved by path (e.g. `@std.web.route`), rather than by a global string like `@route`.
3. **A minimal compiler→stdlib handoff boundary**: when a feature needs compile-time metadata (e.g. route tables), the compiler extracts *only that metadata* and hands it off to `incan_stdlib` for framework/runtime-specific lowering.

Goal: the compiler knows less about “web/http/async/etc”, while the stdlib can grow into multiple modules without colliding in a single global vocabulary.

## Motivation

Incan v0.1 already shows pressure on the **global namespace** and on the compiler’s “special cases”:

- “Framework-y” features (web, async) are implemented via a mix of:
    - global surface vocabulary (e.g. `Json`, `Query`, `App`)
    - compiler feature detection (e.g. “if web is used, enable tokio/axum/serde”)
    - backend emission glue (route wrapper generation, import rewriting)
- As stdlib expands (e.g. `http`, `datetime`, `frozen`), global names will collide or become confusing (`Json` in web vs `Json` in http, etc).
- Adding new stdlib modules today often requires touching multiple compiler layers (typechecker, lowering, emitter, cargo generation), increasing both implementation cost and long-term maintenance.

We want:

- a **clean stdlib module tree** (`std.web`, `std.http`, ...)
- a **namespaced surface** where users can choose which module a name comes from
- a compiler that **does not encode domain logic** for each stdlib module (web/http/datetime/...)

## Guide-level explanation (how users think about it)

### Canonical stdlib imports

Stdlib modules are imported from `std` (e.g. `std.web`, `std.http`):

```incan
from std.web import App, Response, Json, route
from std.http import get, Request
from std.datetime import DateTime
from std.foo import Something
```

`rust::...` remains the explicit Rust interop namespace (RFC 005), so there is no ambiguity:

```incan
from rust::std::time import Instant
from std.datetime import DateTime
```

Note: JSON is intentionally treated as a cross-cutting stdlib concern via `std.json`.

### Namespaced decorators

Decorators can be qualified:

```incan
from std.web import Response

@std.web.route("/")
async def index() -> Response:
    return Response.text("ok")
```

Users can also introduce a local alias for readability:

```incan
import std.web as web
from std.web import Json

@web.route("/api/users")
async def users() -> Json[List[User]]:
    return Json([])
```

### Handoff: compiler extracts metadata, stdlib owns runtime/framework lowering

Some features require compile-time “metadata extraction”. For web routing, the compiler’s role becomes:

- parse and resolve `@std.web.route(...)` (or an alias like `@web.route(...)`)
- collect a route table: `(method(s), path, handler_symbol)`
- emit a single handoff to the stdlib (details are implementation-defined)

The stdlib owns:

- router construction
- framework adapters (e.g. Axum)
- extractor/response conversions
- runtime bootstrap (server start)

## Reference-level explanation (precise rules)

### 1) Canonical stdlib module root: `std`

Normative:

- `std` is the root namespace for **Incan stdlib modules**.
- `rust::std` is Rust’s standard library per RFC 005.
- The language does not require a single monolithic “global stdlib”; instead, it grows via submodules under `std` (for example `std.web`, `std.http`).

This RFC does not require any specific list of stdlib modules to exist immediately, but establishes the canonical shape so that new modules can be added without global collisions.

### 2) Decorator paths (syntax)

Decorators are extended to accept a module path, not only a single identifier.

Proposed grammar change (building on RFC 000):

```ebnf
decorator      = "@" decorator_path [ "(" decorator_args ")" ] ;
decorator_path = module_path ;
```

Where `module_path` is the same path grammar used by imports (RFC 000): segments separated by `::` or `.` with support for absolute `crate::...` paths and parent-navigation via `super`/`..`.

Decorator arguments (`decorator_args`) are unchanged: namespacing affects only how the decorator is identified and resolved, not how its arguments are parsed or type-checked.

Notes:

- Decorator paths use the same `module_path` rules as imports (RFC 000): both `.` and `::` separators are supported. This RFC **recommends dot-style** for decorators because it reads more like a “qualified annotation” than a Rust path.
- The last segment is the decorator name; preceding segments form its namespace path.

### 3) Decorator resolution (semantics)

Normative:

- Decorator names are resolved by the compiler using the same scoping/import machinery as normal names.
- A decorator may refer to:
    - a canonical stdlib decorator (e.g. `std.web.route`)
    - a local alias of a stdlib module (e.g. `web.route` after `import std.web as web`)

> note: user-defined decorator hooks (out of scope; see [“Decisions / direction”])

Diagnostics:

- If a decorator path cannot be resolved, the compiler emits an error naming the full decorator path and suggesting:
    - adding an import (`import std.web as web`)
    - or, using the canonical path (`@std.web.route`)

### 4) Compiler→stdlib handoff boundary (normative direction)

Some stdlib features require compile-time participation (e.g. routing tables, test fixtures, derives).

This RFC standardizes a design principle:

- The compiler may recognize a **minimal set** of stdlib-provided “intrinsics” by their *resolved namespaced identity*.
- The compiler must limit itself to:
    - validating syntax/typing constraints needed to produce correct metadata
    - extracting structured metadata
    - emitting a single handoff boundary to `incan_stdlib`
- Framework/runtime details must live in `incan_stdlib`, not in compiler emission code.

This is a constraint on implementation style: it prevents reintroducing “web scattered everywhere” as new stdlib modules arrive.

#### Example: web routing handoff

Normative contract (shape; exact API is implementation-defined):

- `@std.web.route(...)` produces a route metadata entry containing:
    - path string
    - HTTP method(s) (default GET, or via a `methods=[...]` argument)
    - handler symbol (fully qualified)
- The compiler emits a single stdlib handoff that constructs a router from those entries.

Non-normative implementation sketch:

- Compiler emits a macro invocation such as:

  ```rust
  incan_stdlib::web::__incan_router!((GET, "/a", crate::a::handler), /* ... */)
  ```

- The macro expands inside `incan_stdlib` into any necessary wrapper functions and framework glue.

Important note (implementation reality):

- The handoff must be expressive enough to support **type-safe extraction** (e.g. `Json[T]` / `Query[T]`) and **response conversion** without reintroducing framework logic into the compiler.
- Two acceptable implementation strategies are:
    - **Stdlib-driven typing**: make `std.web` wrapper types implement the underlying framework traits (e.g. Axum extractors / `IntoResponse`) so the route macro needs only `(method, path, handler)` and Rust’s typechecker enforces correctness.
    - **Richer metadata**: have the compiler emit a richer route definition (handler signature/shape) and have the stdlib macro generate wrappers internally. The stdlib-driven typing approach is preferred when feasible, as it minimizes compiler responsibility per this RFC's design principle.

This RFC intentionally does not standardize the exact metadata shape; it standardizes the *boundary*: framework glue belongs in `incan_stdlib`.

### Interaction with existing RFCs / features

- **RFC 005 (Rust interop)**: reinforces the separation: `std` (Incan stdlib) vs `rust::...` (Rust interop).
- **RFC 013 / RFC 020 (dependency policy)**: stdlib handoff should reduce generated-project dependency sprawl by making the generated project depend primarily on `incan_stdlib` (with features), while user `rust::...` imports continue to be governed by `incan.toml` and lock policy.
- **RFC 051 (`JsonValue`)**: a dedicated `std.json` module provides a natural home for a future dynamic JSON value type and JSON parsing/serialization APIs, while keeping web wrappers (`std.web.Json[T]`) separate.
- **Testing RFCs (018/019)**: namespaced `@std.testing.fixture` etc can follow the same pattern as web routing.

## Design details

### Stdlib module map and feature gating

Implementation policy (recommended):

- The compiler maintains a mapping from “imported stdlib module” → “required `incan_stdlib` Cargo feature”.
    - `std.web` → `incan_stdlib` feature `"web"`
    - `std.json` → feature `"json"`
    - etc

This mapping should live in a shared, versioned registry (likely `incan_core`) so the frontend/backend and tooling don’t drift. Feature activation is driven by import resolution: when the compiler resolves an import from a std.* module, it activates the corresponding `incan_stdlib` Cargo feature.

Dependency model (normative direction):

- Generated Rust projects should depend on `incan_stdlib` (with features) rather than depending directly on framework crates like `axum`, `tokio`, or `serde_json` when those are purely implementation details of stdlib modules.
- User-specified Rust dependencies via `rust::...` imports and `incan.toml` remain separate and explicit (RFC 005 /RFC 013).

### Stdlib stub discovery and wiring

This RFC’s goal is that adding a new stdlib module should require touching **as little compiler code as possible**.

Implementation guidance (v0.x):

- Stdlib `.incn` sources are the **authoritative surface** (docs + signatures) for tooling and navigation.
- The compiler’s semantic wiring should be driven by a small, versioned registry (likely in `incan_core`) that maps:
    - `std.<module>` → `incan_stdlib` Cargo feature(s)
    - known `@std.*` decorators / `@rust.extern` stubs → compiler/runtime lowering entrypoints

This keeps the compiler deterministic and avoids requiring it to parse stdlib sources at compile-time, while still making the stdlib “Incan-first” for developer experience.

Future direction (non-normative): the compiler/LSP may optionally use a well-known stdlib source location to improve go-to-definition and hover text, but that should not be required for correctness.

### Multi-module projects and route collection

In multi-file projects, decorators are resolved in their **local module scope** (aliases are per-module), but web routing is a **whole-program concern**:

- The compiler collects all resolved `@std.web.route(...)` entries across the compilation unit (entry module + dependency modules) into a single route table.
- Each route entry references the handler by its fully qualified symbol path so the stdlib handoff can build a single router regardless of where the handler is declared.

### `@rust.extern` — Rust-backed function marker

> **Note:** This section originally introduced `@std.builtin`. RFC 023 renamed it to `@rust.extern`
> and removed the stdlib-only restriction. All references below use the current name.

Stdlib modules are **Incan-first at the surface**: every stdlib module should have an Incan source file that declares its public vocabulary (types, signatures, docs). Where the implementation is provided by a Rust module rather than written in Incan, the source marks those items with `@rust.extern` (this replaces the `@compiler_expand` marker).

Definition:

- `@rust.extern` means **the function's body is provided by a Rust module** (declared via `rust.module()`). It does *not* mean "globally available language builtin" (e.g. `print`, `len`). The two concepts are unrelated!
- `@rust.extern` is allowed in **all Incan source categories** (stdlib, library, application).
- Apply `@rust.extern` at the **smallest granularity possible** — individual functions or methods, not entire modules or types. Type definitions (`model`, `enum`, `class` in the example below) are compiled normally from their Incan source; only the items whose *bodies* are Rust-provided carry the marker.

This serves two purposes:

- **Tooling/UX**: users can Cmd-click into stdlib items and read the intended API and docs in Incan, even when the implementation is compiled from Rust.
- **Clean compiler boundary**: the handoff points are explicit and greppable, rather than scattered as special cases across the compiler pipeline.

Example (non-normative): a future `std.http` surface stub:

```incan
import std.json as json

enum Method:
    GET
    POST

model Request:
    method: Method
    url: str
    body: json.JsonValue = json.JsonValue.Null

model Response:
    status: int
    body: json.JsonValue

@rust.extern
def request(req: Request) -> Result[Response, str]: ...
```

Here `Method`, `Request`, and `Response` are ordinary Incan types compiled by the standard pipeline. Only `request()` is marked `@rust.extern` — its body is provided by the Rust module (e.g. delegating to an HTTP client in `incan_stdlib`).

Migration note (normative):

- This RFC originally renamed the legacy marker `@compiler_expand` to `@std.builtin`; RFC 023 further renamed it to `@rust.extern` and removed the stdlib-only restriction.
- `@compiler_expand` must be treated as deprecated and **removed as part of implementing this RFC**.

### Surface vocabulary: reduce global injections (follow-up)

Today some stdlib-ish types/functions are globally available as “surface vocabulary” (e.g. `Json`, `Query`, async helpers). This RFC’s direction implies a follow-up change:

- stdlib surface names should enter scope **via imports** (`from std.web import Json`) rather than being implicitly defined in the global symbol table.

> Note: Users will have to update their code to use the new import syntax once this RFC is implemented.

## Compatibility / migration

This RFC is intentionally **not prescriptive** about staging vs big-bang rollout.

Given Incan’s early stage, a “big bang” is acceptable and often preferable:

- Switch documentation/examples to `std` namespacing and namespaced decorators (`@std.web.route`) immediately.
- Treat unqualified decorators like `@route` as **breaking removal**.

The core requirement is the *direction*: decorators must be resolvable by a namespace path so new stdlib modules can be added without expanding a global decorator vocabulary.

## Alternatives considered

- **Keep global decorator names** (`@route`) and only namespace modules: helps somewhat, but does not solve “which module owns this decorator?” and reintroduces collisions as decorator space expands.
- **Move everything into compiler builtins** (more surface vocabulary): fastest short-term path, worst long-term maintainability; does not scale with stdlib growth.
- **Runtime-only routing registration** (no compiler metadata): hard in Rust due to typed handler signatures; tends to force dynamic dispatch/boxing and leaks framework details.

## Drawbacks

- Adds complexity to the parser/AST/resolver (decorator paths).
- Requires a clear compatibility story (docs + warnings) to avoid user confusion.
- “Handoff” boundaries must be designed carefully so the stdlib can evolve without forcing frequent compiler changes.

## Implementation plan

This RFC is intentionally high-level about the implementation, to allow the fastest path to the target architecture.

At a minimum, an implementation must:

- **Frontend**: parse decorators as a path and resolve them through imports/aliases.
- **Compiler boundary**: for known stdlib decorators (e.g. web routing), extract only structured metadata and avoid framework-specific emission logic.
- **Stdlib**: provide stable entrypoints/macros for consuming that metadata and owning framework/runtime details.
- **Tests** (minimum bar):
    - Parser tests: decorator paths using `.` and `::`, nested paths, and argument parsing for namespaced decorators.
    - Resolution tests: canonical path vs aliased path, and high-quality diagnostics for unresolved decorator paths.
    - Codegen snapshot tests: ensure the emitted Rust references `incan_stdlib::...` handoff entrypoints and does **not** directly reference framework crates (e.g. no `axum::` in generated output for web programs).
    - Negative tests: `std` / `rust` roots cannot be shadowed.

## Decisions / direction (v0.x)

This section records concrete decisions so the implementation stays coherent and deterministic.

### Canonical spelling vs accepted syntax

- **Accepted syntax**: Incan module paths accept both dot and double-colon separators (RFC 000). Therefore the following are equivalent at the language level:
    - `std.web` and `std::web`
    - the same equivalence applies to decorator paths (dot vs `::` separators)
- **Canonical spelling**: The formatter should normalize Incan module paths (including decorators) to a single canonical spelling by default (recommended: **dot-style**).

Rationale:

- Users can write what they prefer, but **formatted output must be deterministic** and therefore:
    - a single canonical spelling is chosen for formatter output
    - downstream codegen/snapshots must not depend on the user’s original separator choice
- Rust interop paths follow RFC 005: `rust::...` is `::`-only and must not be normalized to dot notation.
- Any future configurability of formatter output (if desired) is out of scope for this RFC and should live in the formatter/configuration RFC(s). The requirement here is determinism, not customization.

### Stdlib handoff compatibility model

- `incan_stdlib` is **toolchain-locked**: it is always tied to the Incan compiler version (like Python’s stdlib is tied to the Python interpreter version).
- The “handoff API” between compiler and stdlib is therefore allowed to evolve alongside the compiler, without a promise of supporting arbitrary historical stdlib versions.
- Implementation should include a simple, explicit **version/compatibility check** so mismatches fail fast with a clear diagnostic.

### User-defined decorators

- For now, decorators are reserved for **stdlib/compiler vocabulary**; unknown decorators are a compile-time error.
- This RFC intentionally keeps the *syntax and resolution model* open to future user-defined decorators by treating decorators as **resolved paths** (not a closed set of global strings).
- A future RFC may introduce user-defined decorators via an explicit macro/plugin system and determinism/safety policy.

### Reserved roots and shadowing

- `std` and `rust` are reserved root namespaces and must not be shadowed.
- Module path qualifiers like `crate`, `super`, and `..` are also reserved and must retain their special meaning in paths.
- We intentionally avoid any “shadowing” or alternate package-root behavior in v0.x. If the community wants that later, it should be addressed in a dedicated packaging/modules RFC.

## Checklist (comprehensive)

This RFC can be considered "implemented" when the following are complete.

### Spec / semantics

- [x] `std` and `rust` are reserved roots and cannot be shadowed.
- [x] Path qualifiers `crate`, `super`, and `..` remain reserved and retain their special meaning in paths.
- [x] Incan module paths accept both `.` and `::` separators as equivalent spellings (RFC 000).
- [x] Rust interop remains `rust::...` and is `::`-only (RFC 005); dot-notation is rejected for `rust::...` imports.
- [x] Decorators accept a namespaced path (`@<module_path>`); `decorator_args` behavior is unchanged.
- [x] Decorator identity is established by *resolution* (module path + name), not by a global string match.
- [x] `@rust.extern` semantics are enforced (RFC 023 supersedes the stdlib-only restriction):
    - [x] Recognized in all Incan source categories (stdlib, library, application).
    - [x] Legacy `@compiler_expand` is deprecated and removed as part of this RFC.

### Syntax / AST / formatting

- [x] Parser/AST supports decorator paths (nested, mixed separators) with precise spans.
- [x] Formatter produces deterministic, canonical output for Incan module paths and decorator paths (recommended: dot-style).
- [x] Formatter does not rewrite or normalize `rust::...` interop paths (RFC 005 rules apply).

### Name resolution / frontend

- [x] Resolver/typechecker resolves decorator paths through the same import/alias machinery as other names.
- [x] High-quality diagnostics exist for unresolved decorator paths (include full path + actionable suggestions).
- [x] `@rust.extern` is recognized by the decorator registry and resolved through normal path resolution.

### Backend (IR / lowering / emission)

- [x] Web routes are collected as *structured metadata* across the full compilation unit (entry module + dependency modules).
- [x] Route entries reference handlers by fully-qualified symbol path.
- [x] Codegen emits a single stdlib handoff for routing (macro/entrypoint in `incan_stdlib`), without embedding framework glue.
- [x] Generated Rust for stdlib-driven web programs does not directly reference framework crates (e.g. no `axum::` in output).

### Runtime / stdlib

- [x] `incan_stdlib` provides stable handoff entrypoints/macros for web routing and owns framework crates behind features.
- [x] Stdlib `.incn` surfaces exist for `std.*` modules, authored in Incan.
- [x] Runtime-provided bodies in stdlib surfaces are marked with `@rust.extern` (smallest granularity).
- [x] Existing stdlib stubs and docs are migrated off `@compiler_expand` per the rename/removal policy.

### Dependency model / generated project

- [x] Generated `Cargo.toml` depends on `incan_stdlib` (with features) rather than directly depending on framework crates when those are purely stdlib implementation details.
- [x] User `rust::...` dependencies remain explicit and governed by RFC 005 / RFC 013 (`incan.toml`, version/features policy).

### Tooling / IDE

- [x] LSP go-to-definition / hover can navigate `std.*` imports and namespaced stdlib decorators to stdlib surface sources (via the same registry used for feature gating).
- [x] LSP reports decorator resolution errors without requiring backend/codegen.

### Tests (at minimum)

- [x] Parser tests: decorator paths with `.` and `::`, nested paths, and argument parsing on namespaced decorators.
- [x] Resolution/typechecker tests: canonical vs aliased decorator paths; unresolved-path diagnostics quality.
- [x] Codegen snapshot tests: routing handoff emission and absence of direct framework crate references.
- [x] Multi-file tests: routes declared in dependency modules are collected and registered correctly.
- [x] Negative tests: shadowing `std`/`rust`; dot notation rejected for `rust::...` imports.

### Docs

- [x] Documentation that mentions the legacy marker is updated to the new spelling (`@rust.extern`) and the legacy spellings (`@compiler_expand`, `@std.builtin`) are marked as removed.
- [x] Documentation, tutorials, and examples are updated to use `std.*` imports and namespaced decorators (e.g. `std.web`and `@std.web.route`), removing legacy spellings like `from web import ...` and `@route(...)`.
