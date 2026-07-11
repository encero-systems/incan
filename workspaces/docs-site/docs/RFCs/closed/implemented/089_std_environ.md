# RFC 089: `std.environ` runtime environment access

- **Status:** Implemented
- **Created:** 2026-05-05
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 017 (validated newtypes with implicit coercion)
    - RFC 033 (`ctx` typed configuration context)
    - RFC 063 (`std.process` process spawning and command execution)
    - RFC 067 (`std.ci` deterministic CI and automation scripting primitives)
    - RFC 070 (Result combinators)
- **Issue:** https://github.com/encero-systems/incan/issues/557
- **RFC PR:** https://github.com/encero-systems/incan/pull/825
- **Written against:** v0.3
- **Shipped in:** v0.5

## Summary

This RFC introduces `std.environ`, a small standard-library module for explicit runtime access to the current process environment. The module provides string accessors (`get`, `get_optional`, `get_or`) and typed accessors (`get_as`) that compose with Incan parsing and validated newtypes, including `from_underlying` validation hooks. `std.environ` is intentionally lower-level than RFC 033 `ctx`: it reads runtime process state directly, while `ctx` remains the typed application-configuration surface built from defaults, environment overrides, and application-specific structure.

## Core model

1. **Environment access is runtime I/O-like state:** environment variables are not compile-time facts and must not be available in `const` evaluation.
2. **The base environment value is a string:** the operating-system boundary exposes string keys and string values; typed values are parsed from that string boundary.
3. **Missing and malformed are distinct:** a missing variable is not the same as a present variable that cannot be parsed or validated.
4. **Typed reads compose with the type system:** `get_as[T]` should parse supported primitive targets and should validate newtypes through their `from_underlying` hooks where applicable.
5. **Application configuration remains higher-level:** ordinary programs may use `std.environ` directly, but structured app config should prefer RFC 033 `ctx` once available.
6. **CI wrappers stay narrow:** RFC 067 `std.ci.env` may wrap or re-export this surface, but CI should not be the only route to reading environment variables.

## Motivation

Environment variables are a normal runtime boundary for applications, CLIs, automation scripts, and deployment platforms. In Python, users commonly reach for `os.environ` or `os.environ.get(...)`; when they come to Incan, they should have an obvious equivalent that does not require Rust interop, shell glue, CI-only APIs, or a full application `ctx` declaration.

The need surfaced while documenting compile-time versus runtime behavior. A `const` example involving environment variables is conceptually useful because environment variables are runtime state, but the docs should not show a fake helper or imply that users must wait for `ctx` before they can read one variable. Incan needs a minimal, real runtime environment surface.

The design should also use Incan's strengths rather than copying Python's stringly style directly. A raw read should be available, but users should be able to parse and validate runtime environment values into stronger types:

```incan
from std.environ import get_as

type PortNumber = newtype int:
    def from_underlying(value: int) -> Result[PortNumber, ValidationError]:
        if value < 1 or value > 65535:
            return Err(ValidationError("port must be between 1 and 65535"))
        return Ok(PortNumber(value))

port = get_as[PortNumber]("PORT", default=8080)?
```

The example is the desired distinction: the environment is runtime input, but the resulting value can still be checked against a domain type.

## Goals

- Provide a general-purpose `std.environ` module for reading current-process environment variables.
- Keep the base API small, explicit, and easy for Python users to recognize.
- Distinguish missing variables from malformed or invalid present values.
- Provide typed reads through `get_as[T]`.
- Make typed reads compose with validated newtypes and `from_underlying`.
- Keep secrets and diagnostics conservative.
- Make `std.ci.env` and `ctx` able to build on or align with the same underlying semantics.

## Non-Goals

- Replacing RFC 033 `ctx` for structured application configuration.
- Replacing RFC 063 `std.process.Command.env(...)` for child-process environment construction.
- Defining a dotenv file loader.
- Defining secret management, vault integration, or encrypted environment variables.
- Standardizing all possible parsing targets in this RFC.
- Making environment variables available to `const` evaluation.
- Requiring current-process environment mutation (`put`, `set`, `unset`, `clear`).
- Introducing new language syntax.

## Guide-level explanation

Use `std.environ` when ordinary runtime code needs to read the current process environment directly.

```incan
from std.environ import get, get_optional, get_or, get_as

token = get("API_TOKEN")?
mode = get_optional("APP_MODE").unwrap_or("dev")
region = get_or("APP_REGION", "eu-west-1")
port = get_as[int]("PORT", default=8080)?
```

Use `get` when the variable is required:

```incan
from std.environ import get

token = get("API_TOKEN")?
```

If `API_TOKEN` is missing, `get` returns an error. It does not silently return an empty string.

Use `get_optional` when absence is meaningful:

```incan
from std.environ import get_optional

match get_optional("APP_MODE"):
    Some(mode) => println(f"mode={mode}")
    None => println("mode=default")
```

Use `get_or` for string defaults:

```incan
from std.environ import get_or

region = get_or("APP_REGION", "eu-west-1")
```

Use `get_as[T]` when the variable should be parsed into a type:

```incan
from std.environ import get_as

port: Option[int] = get_as[int]("PORT")?
```

If `PORT` is absent, this returns `None`. If `PORT=3000`, this returns `Some(3000)`. If `PORT=abc`, this returns a parse error.

When a default is provided, `get_as[T]` returns `T` directly:

```incan
from std.environ import get_as

port = get_as[int]("PORT", default=8080)?
```

The positional spelling is equivalent:

```incan
port = get_as[int]("PORT", 8080)?
```

If `PORT` is absent, the result is `8080`. If `PORT=3000`, the result is `3000`. If `PORT=abc`, the result is still an error; a malformed explicit value must not be hidden by the default.

Typed reads also work with validated newtypes when the underlying parse path and `from_underlying` hook are available:

```incan
from std.environ import get_as

type PortNumber = newtype int:
    def from_underlying(value: int) -> Result[PortNumber, ValidationError]:
        if value < 1 or value > 65535:
            return Err(ValidationError("port must be between 1 and 65535"))
        return Ok(PortNumber(value))

port = get_as[PortNumber]("PORT", default=8080)?
```

Here `PORT=70000` is not accepted just because it parses as an integer. The parsed integer must also pass `PortNumber.from_underlying(...)`.

`std.environ` is still runtime code. It belongs inside functions, setup paths, or configuration initialization, not in `const` declarations:

```incan
from std.environ import get

const TOKEN = get("API_TOKEN")?  # rejected: environment access is runtime behavior
```

## Reference-level explanation

### Module surface

`std.environ` must provide these functions:

```incan
def get(key: str) -> Result[str, EnvironError]
def get_optional(key: str) -> Option[str]
def get_or(key: str, default: str) -> str
def get_as[T with TryFrom[str]](key: str) -> Result[Option[T], EnvironError]
def get_as[T with TryFrom[str]](key: str, default: T) -> Result[T, EnvironError]
```

The second `get_as` overload also supports keyword spelling:

```incan
get_as[T](key, default=value)
```

### Missing variables

`get(key)` must return `Err(EnvironError.Missing(key))` or an equivalent structured missing-variable error when `key` is absent.

`get_optional(key)` must return `None` when `key` is absent or no Unicode value can be read. Callers that need to distinguish absence, an invalid key, and a non-Unicode host value must use `get(key)`.

`get_or(key, default)` must return `default` when `key` is absent or no Unicode value can be read. Callers that need the precise failure category must use `get(key)`.

`get_as[T](key)` must return `Ok(None)` when `key` is absent.

`get_as[T](key, default)` must return `Ok(default)` when `key` is absent, after validating that the default is a valid `T` value at the ordinary call site.

### Present variables

For string accessors, if `key` is present, the returned value must be the environment variable value as a `str`.

For typed accessors, if `key` is present, the implementation must attempt to parse and validate the string value as `T`. If parsing or validation fails, the function must return `Err(...)`; it must not fall back to the supplied default.

### `get_as[T]` parsing and validation

For primitive targets, `get_as[T]` must use the standard string-to-`T` parsing behavior exposed through `TryFrom[str]`. This RFC requires `str`, `bool`, `int`, `float`, and the exact-width integer and floating-point types to support that conversion. Boolean parsing accepts the canonical `true` and `false` spellings. String parsing is identity. Numeric parsing follows the target type's ordinary lexical and range rules.

User-defined models, classes, enums, and newtypes may opt into typed environment reads by implementing `TryFrom[str]` explicitly. A target that neither has compiler-provided `TryFrom[str]` support nor explicitly adopts the trait is unsupported.

For newtype targets, `get_as[T]` should behave as follows:

1. identify the newtype's underlying type;
2. parse the environment string into that underlying type;
3. construct or validate the newtype through the canonical checked-construction path;
4. if the newtype defines `from_underlying`, use that hook and propagate validation failure as `EnvironError.InvalidValue` or an equivalent typed validation variant;
5. return the validated newtype value on success.

If a target type has no supported parse path, `get_as[T]` must be rejected at typecheck time when possible. If the unsupported target is only discovered later through library metadata or backend capability, the diagnostic must name `T` and explain that it cannot be read from an environment string.

### Defaults for newtype targets

For `get_as[Newtype]("KEY", default=value)`, `default` must be type-compatible with the return type. If existing newtype coercion rules allow the newtype's underlying type at this call site, the compiler may apply that checked construction path. Invalid defaults are ordinary compile-time or runtime validation failures according to the existing newtype rules; they are not special to `std.environ`.

### Key rules

Environment variable keys must be `str`.

`get` and `get_as` must reject empty keys and keys containing `=` or NUL with `EnvironError.InvalidKey` or an equivalent error. The non-throwing convenience functions `get_optional` and `get_or` collapse invalid-key and non-Unicode failures into their documented absence/default behavior. Additional platform-specific key restrictions may be reported at runtime.

On platforms with case-insensitive environment keys, the module must follow the platform's native behavior. The language-level contract must not promise cross-platform case sensitivity.

### Error shape

`EnvironError` must distinguish at least:

- missing required key;
- invalid key;
- invalid Unicode or unsupported platform encoding, if applicable;
- parse or validation failure for typed reads.

Errors should include the key name and expected type where relevant. Errors must not include secret values by default.

### Const evaluation

Calls into `std.environ` must not be const-evaluable. They must be rejected in `const` initializers and any other compile-time-only expression context.

### Side effects

The `std.environ` surface defined by this RFC is read-only. Reading an environment variable must not mutate the current process environment.

## Design details

### Why `std.environ`

The name is intentionally `std.environ`, not `std.env`. `environ` matches Python's familiar `os.environ` spelling, while avoiding confusion with project lifecycle environments (`incan env`, `[tool.incan.envs.*]`) and with RFC 033 `Env` enum axes.

### Relationship to `ctx`

RFC 033 `ctx` remains the preferred surface for typed application configuration. `std.environ` is lower-level. It is appropriate for small scripts, library code that needs one runtime variable, and building blocks for higher-level configuration surfaces.

`ctx` may use `std.environ` semantics internally, but this RFC does not require a particular implementation relationship.

### Relationship to `std.ci.env`

RFC 067 `std.ci.env` is CI-oriented. It may wrap, re-export, or mirror `std.environ` functions, but CI should not be the only namespace where environment access exists.

Provider-specific behavior must remain outside `std.environ`. `std.environ` reads process environment variables; it does not know about GitHub Actions, GitLab CI, buildkite, or any other runner.

### Relationship to `std.process`

RFC 063 `std.process` owns child-process environment construction through command builder methods such as `env`, `env_remove`, and `env_clear`. `std.environ` owns current-process environment reads. The two surfaces should use compatible key/value expectations but must not be conflated.

### Secrets and diagnostics

Environment variables often carry secrets. `std.environ` errors should name keys and expected types, but should not print the observed value by default. Debug or tracing integrations may provide opt-in redaction-aware diagnostics later, but this RFC only requires conservative default behavior.

### Why no `put`

Python exposes environment mutation through `os.environ`, and also has lower-level `putenv` / `unsetenv` behavior. Incan should not copy that casually. The current process environment is global process state, and some target runtimes treat mutation as unsafe or platform-constrained once a program is multithreaded. That makes `put` materially different from `get`: reads are ordinary runtime observation, while writes mutate ambient state that libraries, child processes, tests, and platform calls may observe.

The `std.environ` surface is therefore read-only. Code that needs to pass environment variables to a child process should use the child-process environment controls from RFC 063 `std.process`. Tests that need temporary environment changes should use test fixtures or a dedicated testing surface with scoped restoration. A future RFC may add current-process mutation, but it should specify lifecycle restrictions, thread-safety rules, test isolation, and platform behavior explicitly.

### Compatibility / migration

This feature is additive. Existing code using Rust interop, project lifecycle environment injection, or future `ctx` declarations remains valid.

## Alternatives considered

1. **Use only `ctx` for environment variables** — Rejected because `ctx` is a structured configuration feature. Programs and libraries still need a small runtime primitive for direct environment reads.
2. **Use only `std.ci.env`** — Rejected because environment variables are not CI-specific. CI should be a specialized wrapper, not the only environment namespace.
3. **Name the module `std.env`** — Rejected because `env` already appears heavily in project lifecycle terminology and RFC 033 axis examples. `std.environ` is more explicit and Python-familiar.
4. **Single `get(key, default=...)` function** — Rejected because it blurs required, optional, defaulted, and typed access into one overloaded call shape. Separate helpers keep call sites obvious.
5. **Return empty string for missing variables** — Rejected because it hides configuration mistakes and collapses absence into a valid value.
6. **Default hides parse errors** — Rejected because an explicitly configured invalid value should fail loudly. Defaults only handle absence.
7. **Expose the whole environment as a mutable mapping** — Rejected because mutation semantics, platform behavior, secret exposure, and concurrency deserve separate design.
8. **Add `put(key, value)`** — Rejected because current-process environment mutation is global ambient state. Child-process environment construction belongs in RFC 063 `std.process`; scoped test mutation belongs in testing support; general mutation needs a separate safety policy.

## Drawbacks

- The surface adds another configuration-adjacent API next to `ctx`, project lifecycle environments, `std.ci.env`, and `std.process` env builders.
- `get_as[T]` introduces protocol complexity around parsing, newtypes, validation errors, and default handling.
- A read-only environment API may frustrate users who expect Python-like mutation through `os.environ`.
- Platform differences around key casing, encoding, and process environment mutation remain visible at the boundary.

## Implementation architecture

*(Non-normative.)* The Incan source module should own the public API, error model, and missing/default control flow. The host boundary should only read current-process Unicode values and return stable, non-secret failure categories. `get_as[T]` should reuse the source-owned `TryFrom[str]` contract, with compiler-provided implementations for supported primitives and validated newtypes. Newtype conversion should reuse RFC 017 checked-construction metadata rather than adding an environment-specific validation path.

## Layers affected

- **Stdlib / runtime (`incan_stdlib`)**: must expose `std.environ`, provide current-process environment reads, define `EnvironError`, and keep diagnostics conservative around secret values.
- **Typechecker / symbol resolution**: must resolve the generic `get_as[T]` surface, reject unsupported target types where possible, and reject `std.environ` calls in const-only contexts.
- **Lowering / emission**: must lower environment reads to the target runtime's process-environment API and preserve `Result` / `Option` behavior.
- **Validated newtypes / conversions**: must allow `get_as[T]` to compose with supported parse paths and `from_underlying` checked construction for newtype targets.
- **Docs / examples**: must teach `std.environ` as runtime state, distinguish it from `ctx`, and avoid showing environment reads in `const` examples except as rejected code.
- **LSP / tooling**: should provide hover and completion for `std.environ` functions and should surface precise diagnostics for unsupported `get_as[T]` targets.

## Implementation Plan

### Phase 1: source and host boundary

- Expose the read-only string accessors and structured, redacted environment errors from `std.environ`.
- Keep the Rust host boundary limited to current-process Unicode reads and stable failure categories.

### Phase 2: typed conversion contract

- Make supported primitive targets satisfy the source-owned `TryFrom[str]` protocol.
- Route validated-newtype targets through their underlying parse path and RFC 017 `from_underlying` hook.
- Add optional and defaulted `get_as` overloads and preserve malformed-present-value errors.
- Reject unsupported typed targets during typechecking with a target-specific diagnostic.

### Phase 3: compiler and tooling boundaries

- Preserve typed environment behavior through lowering, generated Rust, facades, test batches, and public package consumers.
- Verify that ordinary const evaluation rejects environment reads.
- Expose the complete module through LSP completion and hover metadata.

### Phase 4: documentation and release integration

- Update authored reference documentation, generated feature inventory, and the central 0.5 release notes.
- Advance the development version and complete the RFC lifecycle after all verification gates pass.

## Progress Checklist

### Spec / design

- [x] Define the read-only Unicode environment boundary and distinguish it from `ctx`, `std.ci`, and `std.process`.
- [x] Settle typed reads on the source-owned `TryFrom[str]` protocol.
- [x] Define supported primitive targets and validated-newtype construction semantics.
- [x] Keep mutation and bytes-oriented access outside this RFC.

### Stdlib / runtime

- [x] Implement `get`, `get_optional`, and `get_or`.
- [x] Implement structured missing, invalid-key, non-Unicode, invalid-value, and fallback error categories without secret values.
- [x] Implement optional trait-based `get_as[T](key)` reads.
- [x] Implement positional and keyword defaulted `get_as[T](key, default)` reads.

### Typechecker / conversions

- [x] Provide `TryFrom[str]` for all RFC-required primitive targets.
- [x] Compose `get_as` with explicit user `TryFrom[str]` implementations.
- [x] Compose `get_as` with validated newtypes through underlying parsing and `from_underlying`.
- [x] Apply ordinary checked newtype coercion to underlying default arguments where valid.
- [x] Reject unsupported `get_as[T]` targets at typecheck time with a target-specific diagnostic.
- [x] Reject `std.environ` calls in const-only contexts.

### Lowering / emission / boundaries

- [x] Preserve overload selection and conversion behavior in generated Rust.
- [x] Verify direct imports and facade reexports.
- [x] Verify multi-file and test-batch compilation.
- [x] Verify public package consumers for explicit converters and validated newtypes.

### Tooling

- [x] Verify LSP module and import-item completion for the complete surface.
- [x] Verify LSP hover documentation for `get_as` and `EnvironError`.

### Tests

- [x] Cover required, optional, defaulted string, missing-key, and invalid-key behavior.
- [x] Cover explicit `TryFrom[str]` success, missing, invalid, and redacted error behavior.
- [x] Cover primitive success, malformed values, range failures, and typed defaults.
- [x] Cover validated-newtype success, validation failure, missing/default behavior, and invalid defaults.
- [x] Cover non-Unicode host values on supported platforms.
- [x] Run the complete repository verification gate.

### Docs / release

- [x] Publish complete user-facing `std.environ` reference documentation and examples.
- [x] Regenerate the language feature inventory and RFC indexes.
- [x] Update the central 0.5 release note without partial-scope wording.
- [x] Advance the 0.5 development version.

## Design Decisions

- Current-process environment mutation remains outside the general runtime API. A future RFC may define it only with explicit thread-safety, lifecycle, and test-isolation rules.
- `get_as[T]` uses the existing source-owned `TryFrom[str]` protocol. The compiler provides conformance for the primitive and validated-newtype families required here; user-defined targets opt in by implementing the trait.
- `std.environ` intentionally exposes Unicode `str` values only. Platform byte-oriented access requires a separate proposal.
- A default for a newtype target may use the underlying type when ordinary RFC 017 implicit checked construction permits that call-site coercion. Otherwise callers must pass an already constructed target value.
- `std.ci.env` may retain CI-specific wrappers and diagnostics. This RFC requires semantic alignment, not direct reexports.
- `get_optional` and `get_or` are deliberately lossy convenience functions. Precise host failure categories remain available through `get` and `get_as`.
