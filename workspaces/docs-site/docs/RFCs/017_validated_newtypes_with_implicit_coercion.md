# RFC 017: Validated newtypes with implicit coercion (pydantic-like feel)

- **Status:** In Progress
- **Created:** 2026-01-12
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 021 (model field metadata and aliases)
- **Issue:** [#75](https://github.com/dannys-code-corner/incan/issues/75)
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

Introduce a pydantic-like validated boundary for `newtype` by standardizing a canonical validation/conversion hook on every newtype and allowing the compiler to perform implicit coercion from the underlying type in well-defined contexts (function arguments, typed initializers, and model/class field initialization). Invalid input fails fast with a validation panic by default to preserve “pydantic feel.” Because panics are a poor fit for reusable library code and long-running services, the design keeps an explicit `Result`-returning constructor for recoverable flows, introduces opt-out controls (e.g. `@no_implicit_coercion`), and structured/aggregated validation errors for model/class initialization.

## Motivation

We want guardrails for common primitives (especially numerics) without boilerplate at every use site. Many values originate at boundaries (CLI, config, env, API), and teams want strong invariants with clear errors while raw values “just work” at those boundaries.

Today, authors can validate manually (e.g. helpers returning `Result`), but that requires explicit calls everywhere. The desired ergonomics are: pass raw values at boundaries, coerce and validate automatically, and get a clear validation failure on invalid input.

Concrete shapes this model can express in v1:

- Constrained numerics (pure): `PositiveInt`, `NonNegativeInt`, finite float, etc.
- Parsing/normalization types (pure): e.g. URL/UUID/IP types as parse + normalize without DNS/network I/O.
- Human-friendly boundary parsing stays explicit: e.g. `BytesCount = newtype int[ge=0]` plus explicit `parse_byte_size(str) -> Result[BytesCount, ...]` (v1 does not introduce implicit `str -> int` parsing).
- Redaction/masking types: e.g. `SecretStr` as a normal `newtype str`; masking stays a formatter/serializer concern outside this RFC’s compiler changes.

Types that depend on non-determinism (filesystem existence, wall-clock time, network) are out of scope for v1 implicit coercion; they remain explicit constructors or a future “impure validation” extension.

### Versioning note (v0.1 vs v0.2)

This RFC describes the intended **v0.2** direction. For **v0.1** stabilization, only minimum foundations may land so newtype and model/class construction stay coherent (e.g. reserved hook recognition, no validation bypass paths, better diagnostics).

## Goals

- Standardize a canonical `from_underlying`-style hook on newtypes for validated conversion from the underlying type.
- Allow implicit coercion from the underlying type (and transitive newtype chains) at explicit, listed language sites.
- Support constrained underlying primitives via type-level bracket syntax (e.g. `int[ge=0]`).
- Preserve explicit, non-panicking construction for library and service code.
- Provide opt-out controls and aggregated errors for model/class initialization where appropriate.

## Non-goals

- Implicit ambient primitive conversions (`str -> int`, `int -> float`, etc.) as part of coercion rewriting.
- Non-deterministic or effectful validation inside the implicit-coercion contract for v1.
- Labeled control flow or other unrelated language features.

## Guide-level explanation (how users think about it)

### Newtype validation/conversion hook (canonical)

```incan
type Attempts = newtype int:
    def from_underlying(n: int) -> Result[Attempts, ValidationError]:
        if n <= 0:
            return Err(ValidationError("attempts must be >= 1"))
        return Ok(Attempts(n))
```

### Constrained underlying type syntax

```incan
type NonNegativeInt = newtype int[ge=0]
type PositiveInt = newtype int[gt=0]
```

### Newtype-on-newtype (transitive coercion)

```incan
type PositiveInt = newtype int[gt=0]
type RetryAttempts = newtype PositiveInt
```

Implicit coercion from `int` to `RetryAttempts` follows the chain `int -> PositiveInt -> RetryAttempts` when the compiler accepts the coercion sites.

## Reference-level explanation (precise rules)

### `from_underlying` (compiler-recognized hook)

- `from_underlying` is the canonical validated conversion from the newtype’s underlying type, defined in the newtype body (no separate `impl` block in Incan today).
- If a newtype does not define `from_underlying`, the compiler may auto-generate a default: `from_underlying(x) = Ok(T(x))` when there are no constraints; when the underlying type carries constraints (e.g. `int[gt=0]`), the generated hook must enforce those constraints.
- `from_underlying` is a reserved hook name for newtypes for coercion and validation.
- **Contract:** `from_underlying` must be deterministic and side-effect free (no I/O, no global mutation, no time/random dependence, no randomized hashing dependence). It must not panic; invalid input is expressed as `Err(ValidationError)`. On `Ok`, the result must be the fully validated newtype value for the input (no partial values). Validators that consult the filesystem, clock, randomness, or network require explicit validation or a future opt-in extension.

### Relationship to `TryFrom[T]` (stdlib)

- Incan has `TryFrom[T]` with `@classmethod def try_from(cls, value: T) -> Result[Self, str]`.
- This RFC uses `from_underlying(...) -> Result[T, ValidationError]` as the dedicated hook for structured validation errors and path metadata, and to avoid requiring `impl` blocks on newtypes. A future compiler could derive `TryFrom` from `from_underlying`; the hook remains canonical.

### Constrained primitive types

- `int("5")` remains a plain conversion to `int` (no constraint keyword arguments at runtime).
- Constraints live on the constrained type (`int[ge=0]`) and are enforced by `from_underlying` and by implicit coercion sites.
- For v1, constrained primitives are intended as newtype underlyings, not as universal drop-in replacements for bare `int`/`float` everywhere.

**Constraint vocabulary (v1 proposal):**

- `int[...]`: `ge`, `gt`, `le`, `lt` (values must be compile-time constants: literals or named `const`s).
- `float[...]` (optional for v1): same keys; values compile-time constants. IEEE: comparisons with NaN are false, so NaN must fail all such constraints by default.

**Constraint semantics:** `ge` / `gt` / `le` / `lt` have the usual ordered comparisons. Multiple constraints may appear in one bracket list (`int[ge=0, lt=10]`); order is irrelevant and all must hold. Constraint parameters are compile-time constants, not arbitrary expressions. Only one constraint block is permitted on a primitive. Duplicate keys are rejected.

### Implicit coercion sites

When a context expects validated newtype `T_target` but a value has type `S0`, the compiler may insert implicit coercions consisting **only** of validated newtype conversions (`from_underlying`), possibly chained across nested newtypes. The compiler **must not** insert ambient primitive conversions (e.g. `str -> int`, `int -> float`).

**Proposed contexts:**

1. Function arguments
2. Typed initializers
3. Model/class field initialization
4. Newtype construction `T(x)` (checked by default at that site)

The coercion graph must be acyclic; cycles are rejected with a clear diagnostic.

### Failure behavior

If `from_underlying` returns `Err` during implicit coercion, the runtime behavior is a validation failure (panic carrying validation diagnostics), analogous to pydantic raising on invalid input.

- Function arguments and typed initializers: fail fast (first invalid coercion).
- Model/class field initialization: aggregate field coercion errors and fail once with a multi-error report where specified.

### Controls and policy

Implicit coercion is for boundary ergonomics; it can hide panics in library APIs. Authors must be able to opt out (e.g. `@no_implicit_coercion` at type level and/or per-site controls) and to use explicit `Result`-returning construction where panics are unacceptable.

## Design details

### Why bracket syntax for constraints

Bracket syntax keeps constraints in the type-parameter family (like `list[T]`) instead of looking like a runtime call with keyword arguments on `int(...)`.

### Why keep implicit coercion when explicit `Result` exists

Multiple exit shapes (success, timeout, error) and call-site noise are reduced at boundaries; explicit hooks remain for recoverable paths.

## Alternatives considered

1. **Explicit constructors only** — Rejected: does not deliver boundary ergonomics this RFC targets.

2. **`TryFrom` only, no reserved hook** — Rejected: error type and metadata requirements favor a dedicated `ValidationError`-oriented hook without forcing newtype `impl` blocks today.

3. **Implicit primitive parsing (`str -> int`)** — Rejected: too much hidden work and failure surface; parsing stays explicit at call sites.

## Drawbacks

- Validation panics at implicit sites can surprise readers of call sites unless conventions and docs are clear.
- Unifying coercion chains and diagnostics adds compiler and runtime complexity.
- Constrained types and aggregation must be tuned to avoid confusing error volumes.

## Layers affected

- **Lexer/parser/AST/formatter:** constrained primitive syntax in type position; structured representation; stable formatting.
- **Typechecker:** reserved hook shape, coercion insertion at approved sites, cycle detection, diagnostics.
- **Lowering/IR/emission:** validated coercion representation; deterministic evaluation order; panic paths for failed coercion.
- **Runtime/stdlib:** `ValidationError` shape, aggregation helpers, stable formatting.
- **Docs/tooling:** teach hooks, coercion sites, opt-outs, and explicit alternatives.

## Implementation Plan

### Phase 1 — Stepping-stone (partial)

- Lower newtype direct constructor `T(x)` (single positional argument) through a selected static `from_*` hook when eligible (prefer `from_underlying`; otherwise a single well-shaped `from_*`), with failure surfaced at the boundary.
- Document current behavior versus full RFC in the language guides.

### Phase 2 — Type surface

- Constrained `int[...]` / `float[...]` in type positions, AST modeling, formatter behavior, duplicate-key and incompatibility diagnostics.

### Phase 3 — Coercion expansion

- Implicit coercion at function arguments, typed initializers, model/class field initialization; transitive newtype chains; opt-out attributes.

### Phase 4 — Runtime and diagnostics

- Canonical `ValidationError`, aggregated model/class failures, span-accurate diagnostics and hints (explicit `from_underlying`, try-style APIs).

## Progress Checklist

### Spec / semantics

- [ ] Lock down the **canonical hook** name and signature:
    - [ ] `from_underlying(underlying: U) -> Result[Self, ValidationError]` as the compiler-recognized entrypoint
    - [ ] deterministic / side-effect free contract, and “must not panic” rule
- [ ] Define the **failure policy** precisely per coercion site:
    - [ ] function args + typed initializers: fail-fast (panic on first invalid coercion)
    - [ ] model/class construction: aggregate all field coercion failures and panic once
- [ ] Define **transitive coercion** rules for newtype-on-newtype (chain building + cycle detection).
- [ ] Define **opt-out controls** (library-friendly):
    - [ ] `@no_implicit_coercion` (type-level) and/or per-site opt-out
    - [ ] “Result mode” / explicit construction guidance
- [ ] Define **constraint semantics** for `int[...]` / `float[...]`:
    - [ ] permitted keys (`ge/gt/le/lt`) + compile-time const rules for values
    - [ ] NaN policy for float constraints
    - [ ] duplicate keys + incompatible constraint combinations diagnostics

### Syntax / AST

- [ ] Parser: support constrained primitive type syntax in type position (e.g. `int[gt=0]`, `float[ge=0.0]`).
- [ ] AST: represent constrained types in a structured way (not as ad-hoc strings).
- [ ] Formatter: preserve / normalize constraint formatting (order-insensitive, stable output).

### Frontend (typechecker)

- [ ] Recognize `from_underlying` as a **reserved newtype hook** and validate its shape:
    - [ ] static (no receiver)
    - [ ] exactly one parameter of the underlying type (including constrained underlying types)
    - [ ] return type is `Result[Newtype, ValidationError]`
- [ ] For newtypes without `from_underlying`, define and implement the **auto-generated default** behavior:
    - [ ] `Ok(T(x))` if no constraints
    - [ ] enforce constraints from underlying type if present
- [ ] Insert implicit coercions at the approved sites:
    - [ ] function arguments
    - [ ] typed initializers
    - [ ] model/class field initialization
    - [ ] newtype construction `T(x)` (checked by default)
- [ ] Ensure the compiler does **not** insert ambient primitive conversions (no `str -> int`, no `int -> float`).
- [ ] Diagnostics quality:
    - [ ] span-accurate errors pointing at the call site / field initializer
    - [ ] actionable hints (show explicit `T.from_underlying(...)` / `try_...` alternatives)
    - [ ] include coercion chain in errors (e.g. `int -> PositiveInt -> RetryAttempts`)

### Lowering / IR

- [x] Initial rewriting for newtype `T(x)` → selected validated static hook when eligible (stepping-stone; does not cover all RFC coercion sites).
- [ ] Introduce an explicit IR representation for general “validated coercion” **or** document the full rewriting strategy for all sites.
- [ ] Ensure lowering avoids recursion traps (e.g. `T(x)` inside the hook implementation for `T`).
- [ ] Preserve deterministic evaluation order for default values and coercion calls.

### Codegen (Rust emission)

- [x] Emit validated hook invocation + failure path for stepping-stone newtype `T(x)` construction.
- [ ] Emit `from_underlying(...)` calls at all remaining coercion sites.
- [ ] Emit failure behavior:
    - [ ] fail-fast sites: panic with rich validation context
    - [ ] aggregated sites: collect multiple errors and fail once
- [ ] Keep generated code **zero-cost** outside coercion sites (no extra allocations on happy path beyond user code).
- [ ] Document the generated-code panic path for failed coercion (message shape, caller context); compiler Rust sources remain subject to the project’s own error-handling rules.

### Runtime / core crates

- [ ] Define canonical `ValidationError` type:
    - [ ] structured fields (path, kind, message, optional nested causes)
    - [ ] stable formatting (human-readable + optionally machine-readable)
- [ ] Provide helper APIs for:
    - [ ] creating per-field errors
    - [ ] aggregating multiple errors
    - [ ] panic helpers with useful caller attribution

### Tooling / docs

- [x] Document stepping-stone newtype construction behavior in the Book / reference (current vs full RFC).
- [ ] Update Book + Reference for full RFC:
    - [ ] explain `from_underlying`
    - [ ] document implicit coercion sites and opt-out controls
    - [ ] show explicit alternatives (`Result`-returning flows)
- [ ] Add examples:
    - [ ] constrained numeric newtypes
    - [ ] parsing/normalization newtypes (pure)
    - [ ] model/class aggregated validation example

### Testing (must be comprehensive)

- [ ] Parser tests for constraint syntax and errors.
- [ ] Typechecker tests:
    - [ ] correct insertion of coercions at each site
    - [ ] no ambient primitive conversions
    - [ ] transitive coercion chains + cycle detection errors
    - [ ] spans + hint text assertions for key diagnostics
- [ ] Codegen snapshot tests:
    - [ ] emitted `from_underlying` calls at each coercion site
    - [ ] emitted aggregation logic for model/class initialization
- [ ] Runtime behavior tests:
    - [ ] panic messages include correct path/field context
    - [ ] aggregated errors contain all invalid fields

## Design Decisions

1. **Panic strategy (unwind vs abort)** — Validation failures from implicit coercion use the language runtime’s normal panic mechanism (typically unwinding). A distinct abort-only policy is out of scope unless specified separately.

2. **Constraint catalog for v1** — Start with `ge` / `gt` / `le` / `lt` on `int`, with optional extension to `float` under the same keys; additional constraints or types ship via follow-up RFCs or releases.

3. **Non-panicking mode** — Explicit `Type.from_underlying(value)` (and related explicit APIs) remains the supported recoverable path. Implicit coercion stays panic-on-invalid for v1 until a dedicated opt-in “Result mode” is specified and implemented.

4. **Serialization and FFI** — Implicit coercion applies only at the Incan-language sites listed in this RFC. Serialization codecs and FFI boundaries must use explicit constructors or adapters; no implicit coercion across foreign boundaries in v1.
