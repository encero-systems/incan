# RFC 089: `std.environ` runtime environment access

- **Status:** Draft
- **Created:** 2026-05-05
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 017 (validated newtypes with implicit coercion)
    - RFC 033 (`ctx` typed configuration context)
    - RFC 063 (`std.process` process spawning and command execution)
    - RFC 067 (`std.ci` deterministic CI and automation scripting primitives)
    - RFC 070 (Result combinators)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

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
    def from_underlying(value: int) -> Result[PortNumber, str]:
        if value < 1 or value > 65535:
            return Err("port must be between 1 and 65535")
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
- Requiring current-process environment mutation (`put`, `set`, `unset`, `clear`) in the initial surface.
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
    def from_underlying(value: int) -> Result[PortNumber, str]:
        if value < 1 or value > 65535:
            return Err("port must be between 1 and 65535")
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
def get_as[T](key: str) -> Result[Option[T], EnvironError]
def get_as[T](key: str, default: T) -> Result[T, EnvironError]
```

The second `get_as` overload also supports keyword spelling:

```incan
get_as[T](key, default=value)
```

### Missing variables

`get(key)` must return `Err(EnvironError.Missing(key))` or an equivalent structured missing-variable error when `key` is absent.

`get_optional(key)` must return `None` when `key` is absent.

`get_or(key, default)` must return `default` when `key` is absent.

`get_as[T](key)` must return `Ok(None)` when `key` is absent.

`get_as[T](key, default)` must return `Ok(default)` when `key` is absent, after validating that the default is a valid `T` value at the ordinary call site.

### Present variables

For string accessors, if `key` is present, the returned value must be the environment variable value as a `str`.

For typed accessors, if `key` is present, the implementation must attempt to parse and validate the string value as `T`. If parsing or validation fails, the function must return `Err(...)`; it must not fall back to the supplied default.

### `get_as[T]` parsing and validation

For primitive targets, `get_as[T]` must use the standard string-to-`T` parsing behavior for supported target types.

For newtype targets, `get_as[T]` should behave as follows:

1. identify the newtype's underlying type;
2. parse the environment string into that underlying type;
3. construct or validate the newtype through the canonical checked-construction path;
4. if the newtype defines `from_underlying`, use that hook and propagate validation failure as `EnvironError.Parse` or a more specific typed validation variant;
5. return the validated newtype value on success.

If a target type has no supported parse path, `get_as[T]` must be rejected at typecheck time when possible. If the unsupported target is only discovered later through library metadata or backend capability, the diagnostic must name `T` and explain that it cannot be read from an environment string.

### Defaults for newtype targets

For `get_as[Newtype]("KEY", default=value)`, `default` must be type-compatible with the return type. If existing newtype coercion rules allow the newtype's underlying type at this call site, the compiler may apply that checked construction path. Invalid defaults are ordinary compile-time or runtime validation failures according to the existing newtype rules; they are not special to `std.environ`.

### Key rules

Environment variable keys must be `str`.

An implementation should reject empty keys with `EnvironError.InvalidKey` or an equivalent error. Platform-specific key restrictions may be reported at runtime.

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

The initial `std.environ` surface is read-only. Reading an environment variable must not mutate the current process environment.

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

### Why no `put` in the initial surface

Python exposes environment mutation through `os.environ`, and also has lower-level `putenv` / `unsetenv` behavior. Incan should not copy that casually. The current process environment is global process state, and some target runtimes treat mutation as unsafe or platform-constrained once a program is multithreaded. That makes `put` materially different from `get`: reads are ordinary runtime observation, while writes mutate ambient state that libraries, child processes, tests, and platform calls may observe.

The initial `std.environ` surface is therefore read-only. Code that needs to pass environment variables to a child process should use the child-process environment controls from RFC 063 `std.process`. Tests that need temporary environment changes should use test fixtures or a dedicated testing surface with scoped restoration. A future RFC may add current-process mutation, but it should specify lifecycle restrictions, thread-safety rules, test isolation, and platform behavior explicitly.

### Compatibility / migration

This feature is additive. Existing code using Rust interop, project lifecycle environment injection, or future `ctx` declarations remains valid.

## Alternatives considered

1. **Use only `ctx` for environment variables** — Rejected because `ctx` is a structured configuration feature. Programs and libraries still need a small runtime primitive for direct environment reads.
2. **Use only `std.ci.env`** — Rejected because environment variables are not CI-specific. CI should be a specialized wrapper, not the only environment namespace.
3. **Name the module `std.env`** — Rejected because `env` already appears heavily in project lifecycle terminology and RFC 033 axis examples. `std.environ` is more explicit and Python-familiar.
4. **Single `get(key, default=...)` function** — Rejected because it blurs required, optional, defaulted, and typed access into one overloaded call shape. Separate helpers keep call sites obvious.
5. **Return empty string for missing variables** — Rejected because it hides configuration mistakes and collapses absence into a valid value.
6. **Default hides parse errors** — Rejected because an explicitly configured invalid value should fail loudly. Defaults only handle absence.
7. **Expose the whole environment as a mutable mapping first** — Rejected for the initial surface because mutation semantics, platform behavior, secret exposure, and concurrency deserve separate design.
8. **Add `put(key, value)` immediately** — Rejected for the initial surface because current-process environment mutation is global ambient state. Child-process environment construction belongs in RFC 063 `std.process`; scoped test mutation belongs in testing support; general mutation needs a separate safety policy.

## Drawbacks

- The surface adds another configuration-adjacent API next to `ctx`, project lifecycle environments, `std.ci.env`, and `std.process` env builders.
- `get_as[T]` introduces protocol complexity around parsing, newtypes, validation errors, and default handling.
- A read-only first version may frustrate users who expect Python-like mutation through `os.environ`.
- Platform differences around key casing, encoding, and process environment mutation remain visible at the boundary.

## Implementation architecture

*(Non-normative.)* A minimal implementation can start with current-process string reads plus `Result` and `Option` wrappers, then route `get_as[T]` through the same parse and checked-construction machinery used by ordinary conversions and validated newtypes. CI-specific helpers can then call into the same runtime primitive while preserving their CI-oriented error messages.

## Layers affected

- **Stdlib / runtime (`incan_stdlib`)**: must expose `std.environ`, provide current-process environment reads, define `EnvironError`, and keep diagnostics conservative around secret values.
- **Typechecker / symbol resolution**: must resolve the generic `get_as[T]` surface, reject unsupported target types where possible, and reject `std.environ` calls in const-only contexts.
- **Lowering / emission**: must lower environment reads to the target runtime's process-environment API and preserve `Result` / `Option` behavior.
- **Validated newtypes / conversions**: must allow `get_as[T]` to compose with supported parse paths and `from_underlying` checked construction for newtype targets.
- **Docs / examples**: must teach `std.environ` as runtime state, distinguish it from `ctx`, and avoid showing environment reads in `const` examples except as rejected code.
- **LSP / tooling**: should provide hover and completion for `std.environ` functions and should surface precise diagnostics for unsupported `get_as[T]` targets.

## Unresolved questions

- Should `std.environ` ever include mutation helpers such as `put`, `set`, `unset`, and `clear`, or should current-process environment mutation remain outside the general runtime API?
- Should `get_as[T]` use a general parse trait once one is standardized, or should this RFC define a narrow environment parsing protocol directly?
- Should bytes-oriented environment access exist on Unix-like platforms, or should `std.environ` intentionally expose only Unicode `str` values?
- Should `get_as[T](key, default)` accept an underlying value for a newtype target through implicit checked construction, or require callers to pass an already constructed `T`?
- Should `std.ci.env` re-export `std.environ` names directly, or keep CI-specific wrappers with different error text?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
