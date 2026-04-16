# RFC 032: value enums — `StrEnum` and `IntEnum`

- **Status:** Draft
- **Created:** 2026-03-06
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 050 (Enum Methods & Trait Adoption), RFC 033 (`ctx` Keyword)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/317
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

Introduce **value enums** — enums whose variants carry an associated primitive value (`str` or `int`). This gives Incan a Python `StrEnum`/`IntEnum`-equivalent: enums that are more than labels but less than full ADTs. Value enums auto-generate `value()`, `from_value()`, `Display`, and `FromStr` implementations, enabling clean string/integer lookups, serde round-tripping, and environment-variable resolution.

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

When `Env` is resolved from an environment variable (`APP_ENV=production`), the runtime needs to map the string `"production"` to `Env.Prod`. With value enums, this is automatic:

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
- `str(LogLevel.Info)` → `"info"` (Display uses the value)
- Serde serializes/deserializes using the value string

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
str(Env.Dev)       # → "development" (Display uses value, not name)
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
2. Every variant MUST have a `= <literal>` assignment. Omitting a value is a compile error.
3. Values must be unique within the enum. Duplicate values are a compile error.
4. Value literals must match the declared value type: string literals for `str`, integer literals for `int`.
5. Value enum variants CANNOT carry tuple or struct data — they are simple value variants only. Combining `(str)` value type with `Variant(int, int)` data fields is a compile error.

### Type checking rules

- A value enum is a distinct type (not a subtype of `str` or `int`). `Env` is not `str`.
- `value()` returns the value type: `self.value() -> str` for `StrEnum`, `self.value() -> int` for `IntEnum`.
- `from_value()` is a static method: `Env.from_value(s: str) -> Option[Env]` / `HttpStatus.from_value(n: int) -> Option[HttpStatus]`.
- Value enums participate in pattern matching exactly like regular unit-variant enums.
- Value enums can have methods (per RFC 050, once implemented).
- Value enums can adopt traits (per RFC 050, once implemented).

### Auto-generated implementations

For `enum Foo(str)` with variants `A = "alpha"`, `B = "beta"`:

| Method / Trait |                 Signature                  |                              Behavior                              |
| -------------- | ------------------------------------------ | ------------------------------------------------------------------ |
| `value()`      | `fn value(&self) -> &str`                  | Returns the associated string value                                |
| `from_value()` | `fn from_value(s: &str) -> Option<Foo>`    | Matches input against all variant values                           |
| `Display`      | `fn fmt(...)`                              | Outputs the associated value (not the variant name)                |
| `FromStr`      | `fn from_str(s: &str) -> Result<Foo, ...>` | Same as `from_value`, but returns `Result` for `std::str::FromStr` |
| `message()`    | `fn message(&self) -> String`              | Returns the variant name (existing behavior, unchanged)            |

For `IntEnum`, `value()` returns `i64` and `from_value()` takes `i64`.

### Serde behavior

When serde is active for a value enum:

- Serialization: emits the **value**, not the variant name (`"production"` not `"Prod"`)
- Deserialization: matches on the **value** (`"production"` → `Env.Prod`)
- Backends may realize this through per-variant rename metadata or an equivalent serialization hook.

### Lowering model

Backends should lower value enums to an ordinary closed enum representation plus generated helpers for value lookup, reverse lookup, display behavior, and any serialization metadata required by the chosen backend. The exact emitted code shape is implementation detail; the language-level contract is the generated method and serialization behavior described above.

For `IntEnum`, the same model applies with integer-valued lookup and reverse lookup rather than string parsing.

## Design details

### Proposed Syntax

The value type specifier `(str)` or `(int)` appears after the enum name, before the colon. This mirrors Python's `class Env(StrEnum):` parenthesized base class syntax while remaining consistent with Incan's existing `enum Name:` declaration pattern.

```text
enum Name(str):     # StrEnum
enum Name(int):     # IntEnum
enum Name:          # Regular ADT enum (unchanged)
```

### Semantics

- Value enums are **not subtypes** of their value type. `Env` is not `str`. Use `.value()` to extract.
- `from_value()` returns `Option` — invalid values are not errors, they're `None`. This lets callers decide how to handle unknown values (error, default, etc.).
- The `Display` impl uses the **value**, not the variant name. This is intentional: when you `print()` or interpolate a value enum, you get the wire format. Use `.message()` for the variant name.
- `FromStr` follows the same matching as `from_value()` but returns `Result` for compatibility with Rust's `str::parse()`.

### Interaction with existing features

**Pattern matching**: Value enums match by variant, not by value. `case Env.Dev:` matches the variant, regardless of the associated value. To match on the raw value, use `match env_string: case "production": ...`.

**Traits (RFC 050)**: Once enum methods and trait adoption land, value enums can have additional methods. The auto-generated `value()`, `from_value()`, and `message()` methods will coexist with user-defined methods.

**Serde**: When serde-style serialization is active, value enums serialize and deserialize using their associated values rather than their variant names.

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

Rejected for the initial version: too magical. The whole point of value enums is that the wire value can differ from the variant name (`"development"` ≠ `"Dev"`). Auto-lowercase could be a future convenience shorthand, but explicit values should be the default.

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
- **Display uses value, not name**: Printing an `Env.Dev` shows `"development"`, not `"Dev"`. This is the right default for wire-format types but could surprise users who expect the variant name. `message()` exists for that use case.

## Implementation architecture

*(Non-normative.)* A practical implementation preserves enum-level value-type metadata and per-variant literal values as first-class enum information rather than treating them as ad hoc attributes. Later compilation stages can then derive the standardized helper surface (`value()`, `from_value()`, display and parsing support, and serde-facing value mapping) from that single canonical representation. Tooling should use the same representation so completions, hover text, formatting, and diagnostics remain consistent.

## Layers affected

- **Language surface**: value-carrying enum declarations must preserve explicit variant values as part of the enum contract.
- **Type system**: allowed value types, uniqueness, and interactions with enum method surfaces must be validated.
- **Execution handoff**: implementations must preserve variant values and generate the standardized helper surface such as `value()` and `from_value()`.
- **Serde / runtime interop**: integrations must preserve the documented value mapping for serialization and parsing behavior.
- **Docs / tooling**: the distinction between value enums, plain enums, and general ADTs must be explained clearly.

## Unresolved questions

1. **Should `from_value()` also try case-insensitive matching for `StrEnum`?** Current design is exact match only. Case-insensitive could be a separate `from_value_ignore_case()` or a parameter. The `ctx` axis resolver (RFC 033) does its own case-insensitive fallback on variant names, so `from_value()` can stay strict.

2. **Should auto-lowercase be a future shorthand?** `enum Env(str): Dev` auto-assigning `"dev"` as the value. This is explicitly deferred — require explicit values in v1. Could revisit if the boilerplate becomes tedious.

3. **Should `IntEnum` support auto-incrementing?** `enum Priority(int): Low = 0; Medium; High` where `Medium` gets `1` and `High` gets `2`. Common in C-style enums. Deferred — explicit for v1.

4. **Should there be a `from_name()` companion to `from_value()`?** `Env.from_name("Dev")` for when you want to match on variant names programmatically (rather than values). Currently `message()` gives you name→string, but there's no string→variant by name. The `ctx` axis resolver handles this internally, but exposing it as API might be useful.

5. **Relationship with RFC 050 enum methods**: Once RFC 050 lands, value enums should be able to have user-defined methods alongside the auto-generated ones. Need to ensure no naming conflicts (e.g., user defines their own `value()` method on a value enum — should this be an error?).

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
