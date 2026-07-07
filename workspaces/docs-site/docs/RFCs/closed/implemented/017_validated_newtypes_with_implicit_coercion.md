# RFC 017: Validated newtypes with implicit coercion (pydantic-like feel)

- **Status:** Implemented
- **Created:** 2026-01-12
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 021 (model field metadata and aliases)
- **Issue:** https://github.com/encero-systems/incan/issues/75
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** v0.3.0-dev.39

## Summary

Introduce a pydantic-like validated boundary for `newtype` by standardizing a canonical validation/conversion hook on every newtype and allowing the compiler to perform implicit coercion from the underlying type in well-defined contexts (function arguments, typed initializers, and model/class field initialization). Invalid input fails fast with a validation panic by default to preserve “pydantic feel.” Because panics are a poor fit for reusable library code and long-running services, the design keeps an explicit `Result`-returning constructor for recoverable flows, introduces opt-out controls (e.g. `@no_implicit_coercion`), and structured/aggregated validation errors for model/class initialization.

## Motivation

We want guardrails for common primitives (especially numerics) without boilerplate at every use site. Many values originate at boundaries (CLI, config, env, API), and teams want strong invariants with clear errors while raw values “just work” at those boundaries.

Today, authors can validate manually (e.g. helpers returning `Result`), but that requires explicit calls everywhere. The desired ergonomics are: pass raw values at boundaries, coerce and validate automatically, and get a clear validation failure on invalid input.

Concrete shapes this model can express in the initial validated-newtype contract:

- Constrained numerics (pure): `PositiveInt`, `NonNegativeInt`, finite float, etc.
- Parsing/normalization types (pure): e.g. URL/UUID/IP types as parse + normalize without DNS/network I/O.
- Human-friendly boundary parsing stays explicit: e.g. `BytesCount = newtype int[ge=0]` plus explicit `parse_byte_size(str) -> Result[BytesCount, ...]` (this RFC does not introduce implicit `str -> int` parsing).
- Redaction/masking types: e.g. `SecretStr` as a normal `newtype str`; masking stays a formatter/serializer concern outside this RFC’s compiler changes.

Types that depend on non-determinism (filesystem existence, wall-clock time, network) are out of scope for implicit coercion; they remain explicit constructors or a future “impure validation” extension.

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
- Non-deterministic or effectful validation inside the implicit-coercion contract.
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
- Constrained primitives are intended as newtype underlyings, not as universal drop-in replacements for bare `int`/`float` everywhere.

**Constraint vocabulary:**

- `int[...]`: `ge`, `gt`, `le`, `lt` (values must be compile-time constants: literals or named `const`s).
- `float[...]` may use the same keys; values must be compile-time constants. IEEE comparisons with NaN are false, so NaN must fail all such constraints by default.

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

## Implementation architecture

*(Non-normative.)* A practical rollout has four broad pieces:

1. **Type surface and hook recognition**: support constrained primitive syntax in type position, represent those constraints structurally, and recognize `from_underlying(...) -> Result[Self, ValidationError]` as the canonical newtype validation hook.
2. **Coercion insertion**: apply implicit validated coercion only at the approved language sites in this RFC, including newtype construction, function arguments, typed initializers, and model or class field initialization, while preserving the rule that there are no ambient primitive conversions such as `str -> int`.
3. **Failure and diagnostics model**: preserve fail-fast behavior at ordinary coercion sites, aggregated failures for model or class construction, deterministic coercion ordering, and diagnostics that explain both the coercion site and the chain being attempted.
4. **Runtime and tooling support**: provide the canonical `ValidationError` shape, aggregation helpers, documentation for explicit recovery paths, and comprehensive tests for constraints, coercion insertion, diagnostics, and aggregation behavior.

Implementation sequencing is not part of the public contract. The RFC’s design claim is the full validated-newtype boundary model, not any one stepping-stone rollout order.

## Layers affected

- **Lexer/parser/AST/formatter:** constrained primitive syntax in type position; structured representation; stable formatting.
- **Typechecker:** reserved hook shape, coercion insertion at approved sites, cycle detection, diagnostics.
- **Lowering/IR/emission:** validated coercion representation; deterministic evaluation order; panic paths for failed coercion.
- **Runtime/stdlib:** `ValidationError` shape, aggregation helpers, stable formatting.
- **Docs/tooling:** teach hooks, coercion sites, opt-outs, and explicit alternatives.

## Implementation Plan

### Phase 1: Type surface and validation metadata

- Add constrained primitive type syntax in type position and keep it as structured AST/type metadata.
- Preserve formatter and parser round-tripping for constrained primitive types.
- Recognize `from_underlying` as the canonical newtype hook and validate its deterministic, underlying-to-newtype shape.
- Provide default generated validation metadata for unconstrained newtypes and constrained primitive underlyings.

### Phase 2: Coercion eligibility and insertion

- Build a typechecker-owned validated-newtype coercion planner for underlying-to-newtype and newtype-to-newtype chains.
- Apply coercion only at approved sites: newtype construction, function arguments, typed initializers, and model/class field initialization.
- Reject coercion cycles, primitive parsing, unrelated primitive widening, and opted-out implicit coercion with precise diagnostics.

### Phase 3: Lowering, emission, and runtime errors

- Carry validated coercion decisions through lowering instead of rediscovering them during emission.
- Emit deterministic fail-fast coercion calls for ordinary sites.
- Add the runtime `ValidationError` and aggregation representation needed for model/class field initialization.
- Emit aggregated model/class construction failures where multiple fields fail validation.

### Phase 4: User-facing docs, release notes, and closeout

- Update the newtypes tutorial/reference material to describe checked construction, implicit coercion sites, opt-outs, and explicit recovery paths.
- Add active `0.3` release notes for validated-newtype work.
- Bump the active development version by one `0.3.0-dev.N` increment.
- Run focused parser/typechecker/codegen tests plus the repository pre-commit gate.

## Implementation log

### Spec / lifecycle

- [x] Review RFC 017 for In Progress hygiene and establish the implementation phases/checklist.
- [x] Keep the RFC checklist synchronized as implementation slices land.

### Parser / AST / Formatter

- [x] Parser: accept constrained primitive syntax in type position.
- [x] AST: represent primitive constraints structurally instead of as ordinary generic arguments.
- [x] Formatter: round-trip constrained primitive types stably.
- [x] Diagnostics: reject duplicate keys, unsupported keys, non-constant values, and repeated constraint blocks.

### Typechecker

- [x] Recognize `from_underlying` as the canonical newtype validation hook.
- [x] Validate hook parameter and return shape against the newtype underlying type and `Result[Self, ValidationError]`.
- [x] Generate/default validation metadata for unconstrained and constrained underlyings.
- [x] Plan validated-newtype coercion chains without ambient primitive conversions.
- [x] Insert coercion at newtype construction, function arguments, typed initializers, and model/class field initialization.
- [x] Reject cycles and opted-out implicit coercion with span-precise diagnostics.

### Lowering / IR / Emission

- [x] Carry checked coercion plans through lowering/IR.
- [x] Emit fail-fast validated coercion calls for ordinary coercion sites.
- [x] Emit aggregated validation failures for model/class field initialization.
- [x] Preserve deterministic evaluation order and avoid ad hoc emission-time type guesses.

### Runtime / Stdlib

- [x] Provide canonical `ValidationError` support for validated newtypes.
- [x] Provide aggregation helpers or data structures for model/class construction failures.
- [x] Keep explicit `Result`-returning construction available for recoverable flows.

### Tests

- [x] Parser and formatter tests for constrained primitive syntax.
- [x] Typechecker tests for valid hooks, malformed hooks, coercion chains, cycles, opt-outs, and invalid primitive conversions.
- [x] Codegen snapshots for checked construction, function arguments, typed initializers, and model/class fields.
- [x] Runtime/integration tests for successful coercion, fail-fast errors, and aggregated field errors.

### Docs / Release

- [x] Update authored user-facing newtype documentation.
- [x] Update relevant reference material for validation errors and coercion controls.
- [x] Add active `0.3` release notes.
- [x] Bump the active development version from `0.3.0-dev.38` to the next dev increment.

## Design Decisions

1. **Panic strategy (unwind vs abort)** — Validation failures from implicit coercion use the language runtime’s normal panic mechanism (typically unwinding). A distinct abort-only policy is out of scope unless specified separately.

2. **Constraint catalog** — Start with `ge` / `gt` / `le` / `lt` on `int`, with optional extension to `float` under the same keys; additional constraints or types ship via follow-up RFCs or releases.

3. **Non-panicking mode** — Explicit `Type.from_underlying(value)` (and related explicit APIs) remains the supported recoverable path. Implicit coercion stays panic-on-invalid until a dedicated opt-in “Result mode” is specified and implemented.

4. **Serialization and FFI** — Implicit coercion applies only at the Incan-language sites listed in this RFC. Serialization codecs and FFI boundaries must use explicit constructors or adapters; no implicit coercion across foreign boundaries in this RFC.
