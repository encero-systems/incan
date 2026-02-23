# RFC 017: Validated newtypes with implicit coercion (pydantic-like feel)

- **Status:** In Progress
- **Created:** 2026-01-12
- **Author:** Danny Meijer (@dannymeijer)
- **Issue:** #75
- **Target:** v0.2  

## 0.1 vs 0.2 (what’s implemented when)

This RFC describes the intended **v0.2 direction**.

For **v0.1 stabilization**, we may land only the minimum foundations needed so newtype construction and model/class
construction semantics “make sense” (e.g., reserved hook recognition, no validation bypass paths, better diagnostics).

## Area

- Incan Language (syntax/semantics)
- Runtime / Core crates (stdlib/core/derive)

## Summary

Introduce a pydantic-like “validated boundary” for `newtype` by standardizing a canonical validation/conversion hook on
every newtype and allowing the compiler to perform implicit coercion from the underlying type in well-defined contexts
(function arguments, typed initializers, and model/class field initialization). Invalid input fails fast with a
`ValidationError` panic by default to preserve “pydantic feel”.

Because panics can be a poor fit for reusable library code and long-running services, the proposal also keeps an explicit
`Result`-returning constructor for recoverable flows, and introduces opt-out controls (e.g. `@no_implicit_coercion`) plus
structured/aggregated validation errors for model/class initialization.

## Motivation

We want guardrails for common primitives (especially numerics) without forcing boilerplate everywhere. In practice, many
values originate from boundaries (CLI/config/env/API), and teams want “it just works”-ergonomics with strong invariants
and high-quality error messages.

Today, users can implement validation manually (e.g. helper constructors returning `Result`), but this requires explicit
calls at every use site. The desired “pydantic feel” is:

- pass raw values at boundaries
- have them coerced + validated automatically
- get a clear `ValidationError` on invalid inputs

Concrete “pydantic-like” types this model can express (v1 targets):

- Constrained numerics (pure): `PositiveInt`, `NonNegativeInt`, “finite float”, etc.
- Parsing/normalization types (pure): e.g. `HttpUrl`, `UUID4`, `IPvAnyAddress` implemented as parse + normalize (no DNS/network
  IO).
- Human-friendly boundary parsing stays explicit: e.g. `BytesCount = newtype int[ge=0]` plus an explicit
  `parse_byte_size(str) -> Result[BytesCount, ValidationError]` (v1 does not introduce implicit `str -> int` parsing).
- Redaction/masking types: e.g. `SecretStr` can be a normal `newtype str`; masking is a formatter/serializer concern (outside
  this RFC’s compiler changes).

Conversely, some pydantic types are intentionally out-of-scope for v1 implicit coercion because they are non-deterministic
(filesystem existence checks like `FilePath`, clock-based checks like `PastDate`, etc.). Those can still exist as explicit
constructors, or via a future “impure validation” extension.

## Proposal sketch

### Newtype validation/conversion hook (canonical)

For a newtype:

```incan
type Attempts = newtype int:
    def from_underlying(n: int) -> Result[Attempts, ValidationError]:
        if n <= 0:
            return Err(ValidationError("attempts must be >= 1"))
        return Ok(Attempts(n))
```

**Notes**:

- `from_underlying` is the canonical “validated conversion” from the newtype’s underlying type.
- It is defined in the newtype body (supported today by parser); there is no separate `impl` block in Incan.
- If a newtype does not define `from_underlying`, the compiler auto-generates a default implementation:
    - `from_underlying(x) = Ok(T(x))` (no extra validation).
    - If the underlying type includes constraints (e.g. `int[gt=0]`), the generated `from_underlying` enforces those constraints.
- `from_underlying` is treated as a reserved hook name for newtypes (compiler-recognized for coercion/validation).
- Contract (important for predictability and tooling):
    - `from_underlying` must be deterministic and side-effect free (no IO, no global mutation, no time/random dependence,
      no randomized hashing dependence).
    - `from_underlying` must not panic; it returns `Err(ValidationError)` on invalid input.
    - On `Ok(...)`, it must return exactly the validated newtype instance corresponding to the input (no “partial” values).
    - This intentionally rules out validators that consult the filesystem, the clock, randomness, or the network; those
      require explicit validation, or a future opt-in extension with tighter guardrails.

Relationship to `TryFrom[T]` (stdlib):

- Incan already has a `TryFrom[T]` trait with `@classmethod def try_from(cls, value: T) -> Result[Self, str]`.
- This RFC introduces `from_underlying(...) -> Result[T, ValidationError]` as a dedicated hook because:
    - we want a structured `ValidationError` (paths, chains, constraint metadata) rather than a plain `str`, and
    - Incan does not yet have `impl` blocks for newtypes, so a compiler-recognized hook inside the newtype body keeps the
      surface area minimal.
- Future-friendly: the compiler could auto-provide `TryFrom[U]` for a newtype by delegating to `from_underlying` and
  stringifying the error, but `from_underlying` remains the canonical validation contract.

### Constrained underlying type syntax (nice surface form)

To avoid the semantic implication that `int(...)` accepts constraint keyword arguments at runtime, represent constraints
as type-level parameters using bracket syntax:

(Inspiration: this is similar in spirit to Python's `typing.Annotated[...]` constraints and pydantic's constrained types.
The goal is “feel”, not copying pydantic syntax.)

```incan
type NonNegativeInt = newtype int[ge=0]
type PositiveInt = newtype int[gt=0]
```

**Semantics**:

- `int("5")` remains a plain conversion to `int` (no constraints).
- The constraints live on the constrained type (`int[ge=0]`) and are enforced by the newtype’s validation hook (`from_underlying`)
  and by implicit coercion sites.
- This keeps constraint syntax in the “type parameter” family (similar to `list[T]`, `Result[T, E]`) rather than looking
  like a runtime call with keyword arguments.
- For v1, constrained primitive types are intended to be used as the underlying type of a `newtype` (not as a general
  “drop-in replacement” for `int`/`float` everywhere). This keeps the surface area small while preserving the feel.

Constraint vocabulary (v1, proposal):

- `int[...]`: `ge`, `gt`, `le`, `lt` (values must be compile-time constants: literals or named `const`s)
- `float[...]` (optional for v1): `ge`, `gt`, `le`, `lt` (values must be compile-time constants: literals or named `const`s)
    - IEEE note: comparisons with NaN are always false, so NaN should fail all constraints by default.

Constraint semantics:

- `ge`: greater than or equal (\(\ge\))
- `gt`: greater than (\(>\))
- `le`: less than or equal (\(\le\))
- `lt`: less than (\(<\))

Syntax notes:

- Multiple constraints are allowed: `int[ge=0, lt=10]` (order-insensitive; all constraints must hold).
- Constraint parameters are compile-time constants (not arbitrary expressions).
- Only a single constraint block is permitted on a primitive type (e.g. `int[...]` cannot be followed by another `[...]`).
- Duplicate keys are rejected (e.g. `int[gt=0, gt=1]` is an error).

### Newtype-on-newtype (transitive coercion)

Newtypes should be able to wrap other newtypes:

```incan
type PositiveInt = newtype int[gt=0]
type RetryAttempts = newtype PositiveInt
```

In this case, implicit coercion should be transitive:

- expected `RetryAttempts`, got `int`
    - coerce `int -> PositiveInt` (validate `gt=0`)
    - then coerce `PositiveInt -> RetryAttempts`

Because each newtype has exactly one underlying type, the coercion chain is deterministic. The compiler should reject cycles
(if they can be formed) with a clear diagnostic.

### Implicit coercion sites (compiler rewriting)

When a context expects a validated newtype `T_target` but receives a value of some type `S0`, the compiler may insert
implicit coercions consisting **only** of validated newtype conversions (`from_underlying`), potentially chained across
nested newtypes.

Critically, the compiler **does not** insert ambient primitive conversions (e.g. `str -> int`, `int -> float`) as part of
implicit coercion. If you want parsing/coercion between primitives, write it explicitly at the call site (e.g. `int("5")`).

#### Proposed contexts

1. Function arguments
2. Typed initializers
3. Model/class field initialization
4. Newtype construction (checked by default; `T(x)` treated as a coercion/validation site)

### Controls & policy (avoid hidden panics)

Implicit coercion is intended as a boundary ergonomics feature, but it can introduce hidden panics in reusable library
code (especially when used via function arguments). Provide opt-out controls so library/service authors can make call
sites explicit.

### Failure behavior (pydantic-like)

If `from_underlying(...)` returns `Err(e)` during implicit coercion, panic with a `ValidationError` (or wrap the returned
`e` into a `ValidationError`), similar to pydantic raising/throwing a validation exception.

Aggregation policy:

- Function arguments and typed initializers: fail fast (panic on first invalid coercion).
- Model/class field initialization: aggregate all field coercion errors and panic once with a multi-error
  `ValidationError` for better diagnostics.

## Impact / compatibility

- Behavioral: introduces implicit conversions at specific boundaries; must be carefully scoped to avoid “magic everywhere”.
- Runtime: implicit coercion failure panics; this must be clearly documented as a boundary feature.
- Performance: newtypes should remain zero-cost wrappers in generated code; validation cost is paid only at coercion sites
  and should not allocate/box unless the user’s `from_underlying` does so.

## Implementation notes (high-level)

Likely layers:

- Syntax/parser: support constrained primitive type syntax like `int[gt=0]` as a type-level annotation.
- Frontend/typechecker: detect coercion sites and insert validated coercion nodes.
- Lowering/codegen: emit calls to `from_underlying` and panic with rich errors on failure.
- Stdlib/core: define `ValidationError` shape and formatting.
- Docs: document coercion behavior and explicit alternatives.
- Tests: parser + typechecker + codegen + runtime behavior.

## Unresolved questions

- Panic strategy: unwind vs abort for `ValidationError` panics.
- Constraint catalog scope for v1.
- Non-panicking coercion mode (`Result` mode).
- Serialization/FFI boundaries.

## Checklist (comprehensive)

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

- [ ] Introduce an explicit IR representation for “validated coercion” (preferred) **or** document the exact rewriting strategy.
- [ ] Ensure lowering avoids recursion traps (e.g. `T(x)` inside `impl T` / inside the hook itself).
- [ ] Preserve deterministic evaluation order for default values and coercion calls.

### Codegen (Rust emission)

- [ ] Emit `from_underlying(...)` calls at coercion sites.
- [ ] Emit failure behavior:
    - [ ] fail-fast sites: panic with `ValidationError` (include message + path context)
    - [ ] aggregated sites: collect multiple errors and panic once
- [ ] Keep generated code **zero-cost** outside coercion sites (no extra allocations on happy path beyond user code).
- [ ] Decide and document when generated code may use `.expect(...)` / `.unwrap(...)` vs structured error helpers.

### Runtime / core crates

- [ ] Define canonical `ValidationError` type:
    - [ ] structured fields (path, kind, message, optional nested causes)
    - [ ] stable formatting (human-readable + optionally machine-readable)
- [ ] Provide helper APIs for:
    - [ ] creating per-field errors
    - [ ] aggregating multiple errors
    - [ ] panic/raise helpers with `#[track_caller]`

### Tooling / docs

- [ ] Update Book + Reference docs:
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
