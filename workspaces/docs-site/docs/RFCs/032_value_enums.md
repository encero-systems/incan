# RFC 032: value enums — `StrEnum` and `IntEnum`

- **Status:** Blocked (by RFC 033)
- **Created:** 2026-03-06
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 050 (Enum Methods & Trait Adoption), RFC 033 (`ctx` Keyword)
- **Issue:** [#166](https://github.com/dannys-code-corner/incan/issues/166)
- **RFC PR:**
    - [#411](https://github.com/dannys-code-corner/incan/pull/411)
    - <!-- follow-up PR: LSP metadata and blocked-by-RFC-033 status -->
- **Written against:** v0.2
- **Shipped in:**
    - v0.3 (core value-enum compiler, backend, serialization, manifest, docs, and LSP metadata surface; environment/config resolution remains blocked by RFC 033)

## Summary

Introduce **value enums** — enums whose variants carry an associated primitive value (`str` or `int`). This gives Incan a Python `StrEnum`/`IntEnum`-equivalent: enums that are more than labels but less than full ADTs. Value enums auto-generate `value()`, `from_value()`, string display, and parsing support, enabling clean string/integer lookups, serialization round-tripping, and future environment-variable resolution hooks.

## Goals

- Allow enum declarations to bind each variant to an explicit primitive `str` or `int` value.
- Provide a standard lookup surface for converting value-domain inputs into typed enum variants.
- Preserve existing regular enum and ADT behavior for declarations without a value type specifier.
- Make value enums usable by serialization, parsing, display, and future `ctx` axis resolution without requiring hand-written match helpers.
- Keep the feature explicit and predictable: no inferred values, no enum-as-primitive subtyping, and no custom value types.

## Non-Goals

- General algebraic data types with per-variant values and payload fields in the same variant.
- Custom-valued enums over `float`, `bool`, model types, or arbitrary user-defined types.
- Implicit string derivation from variant names.
- Auto-incrementing integer values.
- Changing regular enum pattern matching semantics.
- Making value enum variants subtype or compare equal to their underlying `str` or `int` values.

## Motivation

### Labels vs values

Today, Incan enums are Rust-style ADTs — powerful for pattern matching with data, but lacking a way to associate a simple value with each variant:

```incan
# Current: pure labels — no associated string value
enum Env:
    Dev
    QA
    Prod

# How do I go from "production" (a config string) to Env.Prod?
# There is no way to do this today.
```

In Python, `StrEnum` solves this:

```python
class Env(StrEnum):
    Dev = "development"
    QA = "qa"
    Prod = "production"

Env("production")  # => Env.Prod
str(Env.Dev)        # => "development"
```

This pattern is everywhere:

- **Configuration**: Environment names, log levels, feature flags — values that arrive as strings from env vars, CLI args, config files, or API responses
- **Serialization**: JSON/YAML field values that map to typed variants (`"pending"` → `Status.Pending`)
- **Database columns**: String or integer codes that map to domain concepts (`1` → `Priority.Low`)
- **API contracts**: Wire values that differ from internal naming (`"prod"` vs `Env.Prod`)

Without value enums, users must write manual match blocks for every conversion, duplicating logic and inviting bugs.

### Prerequisite for `ctx` axis resolution (RFC 033)

RFC 033 introduces the `ctx` keyword with multi-axis match blocks:

```incan
ctx AppConfig:
    match Env:
        case Dev:
            database_url = "sqlite://dev.db"
        case Prod:
            database_url = "postgres://prod/app"
```

When `Env` is resolved from an environment variable (`APP_ENV=production`), the runtime needs to map the string `"production"` to `Env.Prod`. With value enums, that future resolver has a standard lookup table:

```incan
enum Env(str):
    Dev = "development"
    QA = "qa"
    Prod = "production"
```

Without value enums, `ctx` axis resolution can only match on variant *names* (case-insensitive: `"Prod"`, `"prod"`, `"PROD"`), which limits expressiveness and forces users to name variants to match wire values.

### Python familiarity

Python developers expect `StrEnum`/`IntEnum` as core tools. Incan should offer the same convenience with compile-time safety.

## Guide-level explanation (how users think about it)

### Basic `StrEnum`

```incan
enum LogLevel(str):
    Debug = "debug"
    Info = "info"
    Warning = "warning"
    Error = "error"
    Critical = "critical"
```

This declares an enum where each variant has an associated string value. The compiler auto-generates:

- `LogLevel.Debug.value()` → `"debug"`
- `LogLevel.from_value("warning")` → `Some(LogLevel.Warning)`
- `str(LogLevel.Info)` → `"info"` (string display uses the value)
- Serialization and deserialization use the value string

### Basic `IntEnum`

```incan
enum HttpStatus(int):
    Ok = 200
    NotFound = 404
    InternalServerError = 500
```

Same pattern, but with integer values:

- `HttpStatus.Ok.value()` → `200`
- `HttpStatus.from_value(404)` → `Some(HttpStatus.NotFound)`

### Using value enums

```incan
# Parse from string (env var, config file, API response)
let level = LogLevel.from_value(env("LOG_LEVEL"))
match level:
    case Some(l):
        configure_logging(l)
    case None:
        configure_logging(LogLevel.Info)  # default

# Use in match
def describe(status: HttpStatus) -> str:
    match status:
        case HttpStatus.Ok:
            return "Success"
        case HttpStatus.NotFound:
            return "Not found"
        case HttpStatus.InternalServerError:
            return "Server error"

# Access the underlying value when needed
print(f"Status code: {status.value()}")
```

### Value enums with `ctx` (RFC 033)

```incan
enum Env(str):
    Dev = "development"
    QA = "qa"
    Prod = "production"

enum RunMode(str):
    Batch = "batch"
    Streaming = "streaming"

ctx PipelineConfig(env_prefix="PIPELINE_"):
    database_url: str = "sqlite://local.db"

    match Env:
        case Dev:
            database_url = "sqlite://dev.db"
        case Prod:
            database_url = "postgres://prod/app"

    match RunMode:
        case Streaming:
            buffer_size = 0
```

When run with `PIPELINE_ENV=production`, the runtime calls `Env.from_value("production")` to resolve the axis. Without value enums, it would only try case-insensitive variant name matching (`"production"` ≠ `"Prod"`, `"Dev"`, or `"QA"` — no match).

### Interaction with `message()`

Current Incan enums already generate a `message()` method that returns the variant name as a string (e.g., `Color.Red.message()` → `"Red"`). Value enums add a separate `value()` method that returns the associated value. These are distinct:

```incan
enum Env(str):
    Dev = "development"

Env.Dev.message()  # → "Dev" (variant name — existing behavior)
Env.Dev.value()    # → "development" (associated value — new)
str(Env.Dev)       # → "development" (string display uses value, not name)
```

## Reference-level explanation (precise rules)

### Syntax

```text
enum <Name>(<value_type>):
    <Variant1> = <value_literal>
    <Variant2> = <value_literal>
    ...
```

Where `<value_type>` is either `str` or `int`.

**Rules:**

1. The parenthesized value type after the enum name is the **value type specifier**. Only `str` and `int` are allowed.
2. Every variant must have a `= <literal>` assignment. Omitting a value is a compile error.
3. Values must be unique within the enum. Duplicate values are a compile error.
4. Value literals must match the declared value type: string literals for `str`, integer literals for `int`.
5. Value enum variants must not carry tuple or struct data; they are simple value variants only. Combining `(str)` value type with `Variant(int, int)` data fields is a compile error.

### Type checking rules

- A value enum is a distinct type (not a subtype of `str` or `int`). `Env` is not `str`.
- `value()` returns the value type: `self.value() -> str` for `StrEnum`, `self.value() -> int` for `IntEnum`.
- `from_value()` is a static method: `Env.from_value(s: str) -> Option[Env]` / `HttpStatus.from_value(n: int) -> Option[HttpStatus]`.
- Value enums participate in pattern matching exactly like regular unit-variant enums.
- Value enums can have methods (per RFC 050, once implemented).
- Value enums can adopt traits (per RFC 050, once implemented).

### Auto-generated surface

For `enum Foo(str)` with variants `A = "alpha"`, `B = "beta"`:

| Surface        | Incan-facing contract                       | Behavior                                                                            |
| -------------- | ------------------------------------------- | ----------------------------------------------------------------------------------- |
| `value()`      | `self.value() -> str`                       | Returns the associated string value                                                 |
| `from_value()` | `Foo.from_value(value: str) -> Option[Foo]` | Matches input against all variant values                                            |
| String display | string conversion / interpolation           | Outputs the associated value, not the variant name                                  |
| Parsing        | parse from `str` where a `Foo` is expected  | Same lookup semantics as `from_value()` but reported through the target parsing API |
| `message()`    | `self.message() -> str`                     | Returns the variant name; existing behavior is unchanged                            |

For `enum Foo(int)`, `value()` returns `int` and `from_value()` takes `int`.

### External representation

The associated value is the enum's canonical external representation anywhere a value enum crosses a data boundary:

- String display emits the **value**, not the variant name (`"production"` not `"Prod"`).
- Serialization emits the **value**, not the variant name.
- Deserialization matches on the **value** (`"production"` → `Env.Prod`).
- Future configuration and environment resolution hooks should use the same value table when resolving value enum inputs.
- Language-level parsing surfaces use the same value table and failure semantics as `from_value()`, adapted to the parsing API's result type.

Backends may realize this through generated display/parsing helpers, per-variant serialization metadata, or equivalent hooks. The emitted code shape is implementation detail; the language-level contract is that all external representations of a value enum converge on the associated value.

### Lowering model

Backends should lower value enums to an ordinary closed enum representation plus generated helpers for value lookup, reverse lookup, display behavior, parsing, and any serialization metadata required by the chosen backend. The exact emitted code shape is implementation detail; the language-level contract is the generated method surface and external representation described above.

For `IntEnum`, the same model applies with integer-valued lookup and reverse lookup rather than string parsing.

## Design details

### Proposed Syntax

The value type specifier `(str)` or `(int)` appears after the enum name, before the colon. This mirrors Python's `class Env(StrEnum):` parenthesized base class syntax while remaining consistent with Incan's existing `enum Name:` declaration pattern.

```text
enum Name(str):     # StrEnum
enum Name(int):     # IntEnum
enum Name:          # Regular ADT enum (unchanged)
```

If RFC 050 enum trait adoption is available, the `with` clause follows the value type specifier:

```incan
enum Env(str) with Display:
    Dev = "development"
    Prod = "production"
```

### Semantics

- Value enums are **not subtypes** of their value type. `Env` is not `str`. Use `.value()` to extract.
- `from_value()` returns `Option` — invalid values are not errors, they're `None`. This lets callers decide how to handle unknown values (error, default, etc.).
- String display uses the **value**, not the variant name. This is intentional: when you `print()` or interpolate a value enum, you get the wire format. Use `.message()` for the variant name.
- Language-level parsing follows the same matching as `from_value()` but reports failure through the target parsing API rather than returning `Option`.

### Interaction with existing features

**Pattern matching**: Value enums match by variant, not by value. `case Env.Dev:` matches the variant, regardless of the associated value. To match on the raw value, use `match env_string: case "production": ...`.

**Traits (RFC 050)**: Once enum methods and trait adoption land, value enums can have additional methods. The auto-generated `value()`, `from_value()`, and `message()` methods are reserved by value enums and cannot be redefined by user code.

**Serialization**: Value enums serialize and deserialize using their associated values rather than their variant names.

**`ctx` (RFC 033)**: Axis resolution gains a two-step lookup: (1) try `from_value()` for exact value match, (2) fall back to case-insensitive variant name match. This means `PIPELINE_ENV=production` resolves via value, and `PIPELINE_ENV=Prod` resolves via name.

**Generics**: Value enums cannot have type parameters. `enum Foo[T](str):` is a compile error — value enums are inherently concrete.

### Compatibility / migration

This is strictly additive — no existing syntax changes. Regular `enum Name:` declarations continue to work exactly as before. The `(str)` / `(int)` value type specifier is new syntax that doesn't conflict with any existing construct.

## Alternatives considered

### String-valued variants via decorators

```incan
enum Env:
    @value("development")
    Dev
    @value("qa")
    QA
```

Rejected: more verbose, requires decorator infrastructure on enum variants (which doesn't exist), and doesn't clearly signal that this is a _value enum_ vs a regular enum with metadata.

### Implicit string values (auto-lowercase)

```incan
enum Env(str):
    Dev        # implicitly "dev"
    QA         # implicitly "qa"
    Prod       # implicitly "prod"
```

Rejected: too magical. The whole point of value enums is that the wire value can differ from the variant name (`"development"` ≠ `"Dev"`). Explicit values keep the value table obvious and auditable.

### `StrEnum` / `IntEnum` as separate keywords

```incan
strenum Env:
    Dev = "development"
```

Rejected: proliferates keywords. The `enum Name(type):` syntax is more composable and extensible (e.g., future `enum Foo(float):` if needed).

### Make enums subtypes of their value type

In Python, `StrEnum` variants ARE strings — `Env.Dev == "development"` is `True`. We could do the same.

Rejected: breaks Incan's type safety philosophy. A `str` and an `Env` should not be interchangeable. Explicit `.value()` is clearer and avoids subtle bugs where string comparisons accidentally match enum values.

## Drawbacks

- **Complexity**: Adds a new enum flavor. Users must understand the difference between `enum Foo:` (ADT) and `enum Foo(str):` (value enum). The distinction is clear in practice, but it's one more concept.
- **Value type restriction**: Only `str` and `int` are supported. Users wanting `float` or custom types must use regular enums with methods. This is intentional (simple values should be simple) but may require explanation.
- **String display uses value, not name**: Printing an `Env.Dev` shows `"development"`, not `"Dev"`. This is the right default for wire-format types but could surprise users who expect the variant name. `message()` exists for that use case.

## Implementation architecture

*(Non-normative.)* A practical implementation preserves enum-level value-type metadata and per-variant literal values as first-class enum information rather than treating them as ad hoc attributes. Later compilation stages can then derive the standardized helper surface (`value()`, `from_value()`, display and parsing support, and serialization-facing value mapping) from that single canonical representation. Tooling should use the same representation so completions, hover text, formatting, and diagnostics remain consistent.

## Layers affected

- **Parser / AST**: enum declarations must preserve the optional value type specifier and per-variant literal assignments as first-class enum metadata.
- **Typechecker**: value enums must validate allowed value types, required values, value literal types, duplicate values, reserved generated method names, and the prohibition on payload-bearing value variants.
- **Lowering / IR emission**: lowered enum representations must carry enough value metadata to generate the standardized `value()` / `from_value()` surface and preserve the canonical external representation.
- **Serialization / runtime interop**: serialization, parsing, configuration, and environment integrations must use the associated value rather than the variant name whenever a value enum crosses a data boundary.
- **Formatter / LSP / docs tooling**: tooling should preserve and surface value enum declarations distinctly from regular enums and ADTs, including completions, hover text, formatting, and diagnostics.

## Related PRs

- [#411](https://github.com/dannys-code-corner/incan/pull/411) — implemented the core RFC 032 value-enum compiler, backend, serialization, manifest, docs, release-note, and verification surface.

## Implementation Plan

### Phase 1: Parser, AST, and formatter

- Extend enum declaration parsing so `enum Name(str):` and `enum Name(int):` are accepted while regular `enum Name:` and payload-bearing ADT variants continue to behave as before.
- Preserve the optional value type specifier and each per-variant literal assignment in the AST.
- Keep invalid ordinary enum assignments rejected, and emit clear diagnostics for value enum syntax mistakes such as missing values, wrong literal kinds, unsupported value types, and payload-bearing value variants.
- Update formatter behavior so value enum declarations round-trip stably without rewriting regular enum declarations.

### Phase 2: Typechecker and generated surface

- Validate value enum declarations after symbols are collected: allowed value type, required values, value literal type compatibility, duplicate raw values, no payload fields, and no user-defined/generated method name conflicts.
- Register the generated `value()` and `from_value()` method surface so normal member lookup and call checking can typecheck value enum usage.
- Preserve existing `message()` behavior as variant-name access and keep value enums distinct from their raw `str` / `int` value types.

### Phase 3: Lowering, emission, and external representation

- Lower value enum metadata into the IR representation used by enum emission.
- Emit generated helpers for value lookup and reverse lookup, including `value()` and `from_value()`.
- Preserve the canonical external representation for display, parsing, serialization, and deserialization hooks supported by the backend. Configuration and environment resolution remain future integration work.
- Keep emitted code shape backend-owned while preserving the language-level contract defined by this RFC.

### Phase 4: Tests, docs, and release integration

- Add parser, formatter, typechecker, lowering/emission, and codegen snapshot coverage for valid and invalid value enums.
- Add end-to-end tests that exercise value lookup in expression positions, not only declarations.
- Update authored user-facing docs for enum declarations and value enum behavior.
- Bump the active dev version to the target implementation version and add a release-note entry for the planned feature work.

## Progress Checklist

### Spec / lifecycle

- [x] RFC 032 moved to Planned with settled design decisions.
- [x] RFC 032 moved to In Progress for implementation pickup.
- [x] Keep RFC progress checklist current as implementation slices land.

### Parser / AST / formatter

- [x] Parser: accept value enum type specifiers `str` and `int`.
- [x] Parser: parse and preserve per-variant literal assignments for value enums.
- [x] Parser diagnostics: reject missing values, unsupported value types, wrong literal kinds, duplicate/conflicting syntax, and payload-bearing value variants with clear errors.
- [x] AST: represent enum value type metadata and per-variant raw values without changing regular enum behavior.
- [x] Formatter: round-trip value enum declarations and preserve ordinary enum formatting.

### Typechecker

- [x] Validate value enum declarations for allowed value types and required values.
- [x] Validate duplicate raw values and literal type compatibility.
- [x] Reject payload-bearing value enum variants.
- [x] Reserve generated method names such as `value()` and `from_value()` for value enums.
- [x] Typecheck `value()` and `from_value()` calls with `str` / `int` and `Option[Enum]` results.
- [x] Preserve enum-vs-primitive type safety for assignment and equality.

### Lowering / IR / emission

- [x] Carry value enum metadata into IR lowering.
- [x] Emit `value()` helpers for string and integer value enums.
- [x] Emit `from_value()` helpers for string and integer value enums.
- [x] Preserve `message()` as variant-name behavior.
- [x] Preserve canonical external representation hooks for display, parsing, serialization, and deserialization surfaces that exist in the backend.
- [ ] Wire value-enum metadata into future environment/config resolution surfaces once those hooks exist. Current `incan env` lifecycle resolution only merges manifest overlays and has no typed program-config resolver hook yet; this remains blocked on RFC 033 / [#167](https://github.com/dannys-code-corner/incan/issues/167).

### Tooling / docs / release

- [x] Surface value-enum backing metadata in LSP/completion/hover details.
- [x] Update authored user-facing docs for value enum syntax and behavior.
- [x] Add release notes for RFC 032 implementation.
- [x] Bump active dev version to `0.3.0-dev.23`.

### Verification

- [x] Parser tests cover valid value enum syntax and invalid declarations.
- [x] Formatter tests cover value enum round-trips.
- [x] Typechecker tests cover generated surface and invalid primitive assignment/equality.
- [x] Codegen snapshot tests cover `value()` and `from_value()` usage in expression positions.
- [x] Integration test covers compile/run behavior for at least one `str` value enum and one `int` value enum.
- [x] Full repo gate passes.

## Design Decisions

1. **`from_value()` is exact-match only.** Value lookup must not perform implicit case folding. The `ctx` axis resolver described by RFC 033 may apply its own case-insensitive fallback on variant names after value lookup fails, but that fallback is not part of the value enum API.

2. **Variant values are explicit.** `enum Env(str): Dev` must not auto-assign `"dev"` or any other derived string value.

3. **Integer values are explicit.** `enum Priority(int): Low = 0; Medium; High` is invalid because every value enum variant must declare its own value.

4. **Value enum variants remain enum values, not raw primitive values.** `Env.Prod` has type `Env`, not `str`, even when its associated value is `"production"`. Assigning `Env.Prod` to a `str` or comparing `Env.Prod == "production"` is invalid; callers must use `Env.Prod.value()` when they need the raw value.

5. **The associated value is the canonical external representation.** Display, parsing, serialization, deserialization, and configuration/environment resolution must use the associated value rather than the variant name whenever a value enum crosses a data boundary.

6. **Value enums do not expose `from_name()`.** Name-to-variant lookup is reflection/introspection behavior, not value-domain lookup. If Incan later gains general enum reflection, name lookup belongs there rather than in value enums.

7. **RFC 050 `with` clauses follow the value type specifier.** The combined spelling is `enum Env(str) with TraitName:` rather than placing `with` before `(str)` or using a separate value-enum declaration form.
