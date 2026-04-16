# RFC 024: extensible derive protocol

- **Status:** Planned
- **Created:** 2026-02-17
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 050 (enum methods & trait adoption)
    - RFC 051 (`JsonValue`)
    - RFC 021 (field metadata & aliases)
    - RFC 023 (compilable stdlib and rust.module binding)
    - RFC 025 (multi-instantiation trait dispatch)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/148
- **RFC PR:** —
- **Written against:** v0.1
- **Shipped in:** —

## Summary

This RFC proposes an extensible derive protocol that lets modules declare themselves as **derivable**. A derivable module exposes a `__derives__` list that declares which of the module's traits are adoptable via `@derive()`. When a type derives the module, those traits — and their methods — are adopted onto the type. This replaces the current closed built-in model for format-related derives with a trait-based, module-driven mechanism, enabling user-defined serialization formats, schema generators, and behavioral adapters without compiler changes.

## Motivation

### The derive system is closed

Today, format-oriented derives are still effectively compiler-owned. A small built-in derive set controls the behavior, and format-specific method injection is still special-cased rather than driven by an open protocol. Adding a new serialization format therefore requires compiler changes across multiple stages instead of being something library authors can express themselves.

### Serialization isn't just JSON

A natural extension of Incan's model system is serving multiple wire formats from one definition: JSON, YAML, Protobuf, Avro, Arrow, and more. Each format needs its own serialization/deserialization methods, and users need the ability to pick exactly which formats a model supports. For example:

```incan
from std.serde import json, yaml
from std.schema import protobuf

@derive(json, yaml, protobuf)
model CustomerEvent:
    customer_id: str
    email: str
    event_type: str
    amount: int
    timestamp: datetime
```

This model gets `.to_json()`, `.from_json()`, `.to_yaml()`, `.from_yaml()`, `.proto_schema()` — all statically verified, all type-safe.

### Users need custom derives

Data engineering workflows (steps, pipelines, Readers/Writers) often use internal formats. Teams need to create their own derivable modules — an internal binary codec, a company-specific schema format, a custom wire protocol — without forking the compiler or waiting for stdlib additions.

### Injected methods need trait bounds

The current format-specific method injection approach makes `.to_json()` appear on types that derive `Serialize`, but there is no trait-backed contract that generic code can name. This means generic functions cannot express "T must be JSON-serializable":

```incan
# Impossible today — no trait to bind against
def export[T](data: T) -> str:
    return data.to_json()  # Compiler: "T has no method to_json"
```

With trait-based derives, this becomes expressible:

```incan
def export[T with json.Serialize](data: T) -> str:
    return data.to_json()  # Verified: json.Serialize guarantees .to_json()
```

## Non-Goals

- **Implementing specific format libraries.** This RFC uses YAML, Protobuf, Avro, SQL DDL, and others as illustrative
examples of what the protocol *enables*. It does not propose adding those libraries to the stdlib. Each format would be introduced by its own RFC or feature issue (for example RFC 051 for `JsonValue`).
- **Migrating built-in derives** (`Eq`, `Clone`, `Debug`, etc.) to the `__derives__` protocol. These remain compiler
intrinsics handled by the `DeriveId` registry. See [Interaction with existing features](#interaction-with-existing-features) for details.
- **Runtime reflection of field values.** The protocol relies on existing `__fields__()` metadata reflection for schema
generators. Dynamic field *value* access (needed to express `__eq__` or `__repr__` in pure Incan) is out of scope.

## Guide-level explanation (how users think about it)

### Deriving a format

Users import a format module and derive it:

```incan
from std.serde import json

@derive(json)
model Config:
    host: str
    port: int
    debug: bool
```

`Config` now has `.to_json()` and `.from_json()` methods. The user can see exactly where they come from — the `json` module defines the traits.

### Deriving multiple formats

```incan
from std.serde import json, yaml

@derive(json, yaml)
model Config:
    host: str
    port: int

config = Config(host="localhost", port=8080)
json_str = config.to_json()
yaml_str = config.to_yaml()
```

Both format modules define their own `Serialize` trait, each carrying `@rust.derive("serde::Serialize")`. The compiler deduplicates to a single Rust-level derive, while each module injects its own distinct methods (`.to_json()` vs
`.to_yaml()`).

### Partial derives (serialize-only, deserialize-only)

```incan
from std.serde.json import Serialize as json_write

@derive(json_write)
model LogEntry:
    message: str
    level: str
    timestamp: datetime

# LogEntry has .to_json() but NOT .from_json()
```

### Schema generation (pure Incan, no Rust needed)

Not all derives involve serialization. Schema generators produce text artifacts from a model's field metadata using
`__fields__()` reflection — no `rust::` imports required:

```incan
from std.schema import sql

@derive(sql)
model Users:
    id: int
    name: str
    email: str

print(Users.sql_ddl())
# CREATE TABLE Users (
#   id INTEGER,
#   name TEXT,
#   email TEXT
# );
```

The (hypothetical) `sql` module in this example defines a `SqlSchema` trait whose `sql_ddl()` method is implemented entirely in Incan by iterating over `__fields__()`. The same pattern works for OpenAPI, GraphQL type definitions, or any text-based schema format.

### Behavioral derives

Derives aren't limited to formats. A derivable module can attach any behavior to a model:

```incan
from std.schema import sql
from my_company.observability import auditable

@derive(sql, auditable)
model Account:
    id: int
    owner: str
    balance: int
```

Here `auditable` might define an `Auditable` trait that provides a `.diff(other)` method, a `.changelog()` method, or field-level change tracking — whatever the module's traits declare. The mechanism is the same regardless of whether the derive produces bytes, text, or behavior.

### Using trait bounds in generic functions

Because derives are backed by traits, they compose with the `with` bound syntax ([RFC 023]):

```incan
from std.serde import json, yaml

def export[T with (json.Serialize, yaml.Serialize)](
    data: T,
    format: str,
) -> str:
    if format == "json":
        return data.to_json()
    return data.to_yaml()
```

### Writing a custom derivable module

No compiler changes needed. A user writes exactly the same pattern as stdlib:

```incan
# my_company/formats/internal.incn
from rust::my_codec import encode, decode

__derives__ = [Serialize, Deserialize]

# No @rust.derive needed — encode/decode handle serialization directly, without requiring a Rust derive on the struct.
trait Serialize:
    def to_internal(self) -> bytes:
        return encode(self)?

trait Deserialize:
    def from_internal(data: bytes) -> Result[Self, str]:
        return decode(data)?
```

Then anywhere in the codebase:

```incan
from my_company.formats import internal

@derive(internal)
model SensorReading:
    device_id: str
    value: float
```

`SensorReading` now has `.to_internal()` and `.from_internal()`.

## Reference-level explanation (precise rules)

### The `__derives__` module attribute

A module that defines a `__derives__` attribute at module level is a **derivable module**. The attribute lists which of the module's traits are adoptable via `@derive()`:

```incan
__derives__ = [Serialize, Deserialize]
```

Here, `Serialize` and `Deserialize` refer to traits defined in the same module. When a type writes
`@derive(module_name)`, the compiler:

1. Resolves `module_name` to the imported module
2. Reads `module_name.__derives__` to get the list of derivable traits
3. Adopts those traits onto the type — their methods become available on instances of the type
4. Determines the Rust-level `#[derive(...)]` attributes needed (an emission concern, derived from `@rust.derive`
decorators on the adopted traits)

### Trait adoption via derive

The traits listed in `__derives__` are adopted by any type that derives the module. This is equivalent to the type writing `with TraitName` for each listed trait, but driven by the `@derive()` decorator. Only traits explicitly listed in `__derives__` are adopted — other traits defined in the module are not automatically included.

### Rust derive binding via `@rust.derive`

A trait in a derivable module may need the compiler to emit a Rust `#[derive(...)]` attribute on any struct that adopts it. This is distinct from `@rust.extern` (which delegates a *method call* to Rust) — `@rust.derive` declares that the
*type itself* requires a Rust-level derive for the trait's methods to work.

The `@rust.derive("path::to::Derive")` decorator on a trait declaration carries this binding:

```incan
@rust.derive("serde::Serialize")
trait Serialize:
    def to_json(self) -> str:
        return to_string(self)?
```

When a type adopts this trait via `@derive()`, the compiler emits `#[derive(serde::Serialize)]` on the Rust struct.

Traits that don't need a Rust-level derive (pure Incan behavioral traits, schema generators using `__fields__()` reflection) simply omit `@rust.derive` — their methods compile normally without any struct-level annotation.

### Derive deduplication

Multiple modules may declare the same `@rust.derive` path. For example, both `json.Serialize` and `yaml.Serialize` carry `@rust.derive("serde::Serialize")`. The compiler collects all `@rust.derive` paths from all adopted traits into a set before emission, producing one `#[derive(serde::Serialize, serde::Deserialize)]` regardless of how many format modules are derived.

### Individual trait imports

Traits within a derivable module can be imported individually:

```incan
from std.serde.json import Serialize
```

When used in `@derive(Serialize)`, only that single trait is adopted (and its `@rust.derive` path, if any, is emitted). This enables fine-grained control — derive only serialization, only deserialization, etc.

### Method resolution

When a type derives a module, the module's traits are adopted. Method calls on instances of the type resolve through normal trait method lookup. If two derived modules define traits with the same method name, this is a compile-time error (ambiguous method), following normal trait method resolution rules.

## Design details

### Syntax

Three new syntactic elements:

1. **Module-level `__derives__` attribute**: a list of derive names assigned at module scope.

    ```incan
    __derives__ = [Serialize, Deserialize]
    ```

    Parsed as a const assignment where the name is `__derives__` and the value is a list of identifiers. Each identifier
    must resolve to a trait defined in the same module.

2. **`@derive(module)` expansion**: the existing `@derive(...)` syntax is extended to accept module names (not just
derive names). When the argument resolves to a module with a `__derives__` attribute, it is expanded.

    ```incan
    from std.serde import json
    @derive(json)          # Module derive — expands via __derives__
    @derive(Debug, Clone)  # DeriveId derives — unchanged
    model Foo:
        x: int
    ```

3. **`@rust.derive` decorator on traits**: declares the Rust `#[derive(...)]` attribute that must be emitted on any
struct adopting this trait. This is the bridge between an Incan trait and the Rust code generation it requires.

    ```incan
    @rust.derive("serde::Serialize")
    trait Serialize:
        def to_json(self) -> str:
            return to_string(self)?
    ```

    Traits without `@rust.derive` are pure Incan — no Rust-level derive is emitted for them.

No new keywords. `@rust.derive` follows the existing `@rust.extern` decorator pattern.

### Semantics

When the compiler encounters `@derive(name)`:

1. **Resolve `name`**: check if it refers to a `DeriveId` (built-in derive) or an imported symbol.
2. **If `DeriveId`**: existing behavior — emit the corresponding Rust `#[derive(...)]`.
3. **If module with `__derives__`**: adopt the traits listed in `__derives__` onto the type. The compiler determines the
necessary Rust-level derives from the adopted traits during emission.
4. **If trait from a derivable module**: adopt only that single trait onto the type.
5. **Error**: if `name` is neither a known derive, a derivable module, nor a trait from one — emit a diagnostic.

Trait method injection follows normal trait adoption rules. Methods with `self` receiver become instance methods on the adopting type. Methods without a receiver become associated functions (e.g., `Model.from_json(s)`).

### Three categories of derivable modules

#### 1. Serialization formats (data in/out)

These convert instances to/from bytes or strings. They use `rust::` interop to call codec libraries. Multiple serde formats define similarly-named traits (each module has its own `Serialize` / `Deserialize`) that inject distinct methods:

|       Module        |       `__derives__`        |          Traits / methods          |
| ------------------- | -------------------------- | ---------------------------------- |
| `std.serde.json`    | `[Serialize, Deserialize]` | `.to_json()`, `.from_json()`       |
| `std.serde.yaml`    | `[Serialize, Deserialize]` | `.to_yaml()`, `.from_yaml()`       |
| `std.serde.toml`    | `[Serialize, Deserialize]` | `.to_toml()`, `.from_toml()`       |
| `std.serde.msgpack` | `[Serialize, Deserialize]` | `.to_msgpack()`, `.from_msgpack()` |
| `std.serde.csv`     | `[Serialize, Deserialize]` | `.to_csv_row()`, `.from_csv_row()` |

Example implementation:

```incan
# stdlib/serde/json.incn
from rust::serde_json import to_string, from_str

__derives__ = [Serialize, Deserialize]

@rust.derive("serde::Serialize")
trait Serialize:
    def to_json(self) -> str:
        return to_string(self)?

@rust.derive("serde::Deserialize")
trait Deserialize:
    def from_json(json_str: str) -> Result[Self, str]:
        return from_str(json_str)?
```

No `rust.module()`, no `@rust.extern` — the `.incn` file is the complete implementation. `@rust.derive` declares the Rust struct-level derive needed for the `rust::` interop calls to work. The `rust::` interop ([RFC 005]) provides access to the underlying Rust codec library.

#### 2. Schema generators (type shape out)

These generate schema artifacts from the model's type definition. They operate on field metadata via `__fields__()` reflection and are typically pure Incan:

|        Module         |    `__derives__`     |  Traits / methods   |       Artifact        |
| --------------------- | -------------------- | ------------------- | --------------------- |
| `std.schema.protobuf` | `[ProtobufMessage]`  | `.proto_schema()`   | `.proto` definition   |
| `std.schema.avro`     | `[AvroSchemaDerive]` | `.avro_schema()`    | Avro schema JSON      |
| `std.schema.openapi`  | `[OpenApiSchema]`    | `.openapi_schema()` | OpenAPI spec fragment |
| `std.schema.graphql`  | `[GraphqlType]`      | `.graphql_type()`   | GraphQL type def      |
| `std.schema.sql`      | `[SqlSchema]`        | `.sql_ddl()`        | `CREATE TABLE`        |
| `std.schema.arrow`    | `[ArrowSchema]`      | `.arrow_schema()`   | `arrow::Schema`       |

Schema generators with only one trait can still use `__derives__` to make the module derivable. Since the trait has no
`@rust.derive` decorator, no Rust-level derive is emitted — the trait methods are pure Incan reflection:

```incan
# stdlib/schema/sql.incn

__derives__ = [SqlSchema]

trait SqlSchema:
    def sql_ddl(self) -> str:
        lines: list[str] = []
        lines.append(f"CREATE TABLE {self.__class_name__} (")
        for field in self.__fields__():
            sql_type = _incan_type_to_sql(field.type_name)
            lines.append(f"  {field.wire_name} {sql_type},")
        lines.append(");")
        return "\n".join(lines)
```

Some formats are **hybrids** — they need both schema generation AND instance serialization (e.g., Avro needs schema JSON plus binary encode/decode).

#### 3. Behavioral derives

These attach behavior to models without producing bytes or schemas. For example:

|      Module      | `__derives__` |              What it does               |
| ---------------- | ------------- | --------------------------------------- |
| `std.validation` | `[Validate]`  | Checked construction via `.new()`       |
| `std.governance` | `[Governed]`  | PII masking, field-level access control |
| `std.versioning` | `[Versioned]` | API version-aware response shapes       |

### Interaction with existing features

#### Built-in derives (`Eq`, `Clone`, `Debug`, etc.)

Built-in derives remain compiler intrinsics. They are **not** migrated to the `__derives__` protocol because their implementations are Rust proc macros that generate `impl` blocks — there is no Incan-expressible body to put in a trait. The `DeriveId` registry continues to handle these.

The distinction is clear: built-in derives implement *language-level semantics* (equality, ordering, cloning, debug formatting). Format derives implement *library-level functionality* (serialization, schema generation). The protocol applies to the latter.

> Note: as the language evolves, this might change. It is hypothetically possible to rewrite the built-in derives as
> traits in the stdlib, but that would be a significant change requiring currently unavailable functionality and syntax
> that is not in scope for this RFC.

#### `rust::` imports (RFC 005)

The `rust::` import mechanism is the primary way derivable modules access Rust codec libraries. A derivable module's trait methods are pure Incan that call into Rust libraries via `rust::` imports. The two mechanisms are complementary.

#### `with` trait bounds (RFC 023)

Traits from derivable modules work with the existing `with` bound syntax. A function can require specific format capabilities:

```incan
def publish[T with (json.Serialize, avro.Serialize, avro.AvroSchema)](
    events: List[T],
    target: ExportTarget,
) -> Result[int, str]:
    match target:
        ExportTarget.Api =>
            for e in events:
                http_post(e.to_json())
        ExportTarget.Kafka =>
            schema = T.avro_schema()
            for e in events:
                kafka_publish(e.to_avro(), schema)
```

#### Field metadata (RFC 021)

Derivable modules can read field metadata via `__fields__()`. This enables format-specific field annotations:

```incan
from std.serde import json
from std.schema import protobuf

@derive(json, protobuf)
model Event:
    customer_id: str
    email [pii=True, proto.tag=1]: str
    event_type [proto.tag=2, values=["click", "purchase"]]: str
```

The `json` module sees `alias`, `description`, etc. The `protobuf` module reads `proto.tag` for stable field numbering. Each format consumes the metadata it understands and ignores the rest.

### Compatibility / migration

This RFC is **additive** for the protocol itself — `__derives__`, `@rust.derive`, and module-based `@derive()` are new capabilities. However, it includes one **deprecation**: bare `@derive(Serialize, Deserialize)` will be removed from the
`DeriveId` registry once the `std.serde.json` module is available (see design decision #4). Users migrate to the
explicit module form:

```incan
# Before (deprecated — will be removed)
@derive(Serialize, Deserialize)
model Config:
    host: str

config.to_json()

# After
from std.serde import json

@derive(json)
model Config:
    host: str

config.to_json()
```

The migration is mechanical: add the format import, replace bare `Serialize`/`Deserialize` with the module name. The generated Rust output is identical. Built-in derives (`Debug`, `Clone`, `Eq`, etc.) are unaffected.

## Alternatives considered

### 1. Decorator-based method injection (current approach)

The status quo: hardcode method injection in the typechecker per derive. Rejected because it doesn't scale to N formats and provides no trait for generic bounds.

### 2. `__derive__` as a simple list without traits

A module-level `__derive__` that maps to Rust derives, with methods injected by convention (e.g., `to_<format>` always exists). Rejected because there's no trait to bind against in generic functions, and the method signatures are invisible to the user.

### 3. Proc-macro-style user derives

Allow users to write Rust proc macros and register them as Incan derives. Rejected because it requires Rust expertise and breaks the "Incan all the way down" principle. The trait-based approach keeps everything in Incan.

### 4. Making all built-in derives use this protocol too

Migrate `Eq`, `Clone`, `Debug`, etc. to `__derives__`-based modules. Rejected because these are genuinely compiler intrinsics — their implementations are Rust proc macros that generate `impl` blocks, not callable functions. The protocol is for library-level functionality.

## Drawbacks

- **Two derive systems**: built-in compiler derives and module-based derives (`__derives__` protocol)
coexist. This is intentional because they serve different purposes, but it does add conceptual surface area.
- **Naming collisions**: if a module defines a `Serialize` trait and the user also imports `Serialize` from another
module, the compiler must disambiguate. Normal trait resolution rules apply, but the error messages need to be clear.
- **Backend derive deduplication**: the compiler must correctly deduplicate backend-level derive metadata gathered from
multiple adopted traits. This is straightforward, but it does add another lowering step.

## Implementation architecture

*(Non-normative.)* A practical rollout has four broad steps:

1. **Surface recognition**: support module-level `__derives__` declarations and trait-level `@rust.derive(...)` metadata, including validation that declared derive traits are real traits in the same module and that empty `__derives__` lists are rejected.
2. **Trait adoption path**: resolve `@derive(name)` through modules or directly imported derivable traits, adopt the selected traits onto the target type, inject their method surfaces, and report ambiguity clearly when multiple derivable modules collide.
3. **Backend deduplication**: collect backend-facing derive metadata from adopted traits, deduplicate it before emission, and preserve whole-module plus partial-trait derive usage coherently.
4. **Library migration and proof points**: migrate `std.serde.json` to the protocol, then validate the design against at least one additional format module and one non-Serde schema-style derive surface so the mechanism proves it is genuinely extensible rather than JSON-shaped.

The implementation order is not part of the public contract. The important point is that the derive protocol remains module-driven rather than hardcoded around one closed compiler registry.

## Design decisions

The following questions were considered during design and are recorded here for posterity.

1. **Trait naming within modules**: modules use short names (`Serialize`, `Deserialize`). Users who need disambiguation
use import aliasing: `from std.serde.json import Serialize as JsonSerialize`. This keeps module definitions simple and pushes naming concerns to the import site where the user has full context.

2. **`__derives__` syntax**: parsed as an implicit const assignment. The dunder convention already signals
"compiler-recognized"; an explicit `const` keyword would be redundant. It is semantically immutable — reassigning
   `__derives__` is a compile error.

3. **Missing or empty `__derives__`**: a module without `__derives__` is not derivable. A module with
   `__derives__ = []` is a compile error (or at minimum a warning) — an empty list signals a mistake, since there is
no reason to declare `__derives__` without listing at least one trait.

4. **Bare `Serialize` / `Deserialize` derives**: bare `@derive(Serialize, Deserialize)` ceases to exist as a `DeriveId`
shortcut. Users import the format module and derive it: `@derive(json)`. If direct access to the Rust serde traits is needed, `rust::` interop remains available. This eliminates ambiguity and makes the format dependency explicit.

5. **`@rust.derive` validation**: treated the same as `@rust.extern` — the path string is passed through to the emitted
Rust code. Validation happens at Rust compile time, not in the Incan compiler. This keeps the protocol simple and works with any Rust derive crate without the Incan compiler needing to know about them.

6. **Multiple `@rust.derive` on one trait**: allowed. A single trait may require multiple Rust-level derives. The
decorator accepts multiple arguments: `@rust.derive("serde::Serialize", "apache_avro::AvroSchema")`.

## Layers affected

- **Parser / AST**: must recognize the derive protocol surface, including module-level `__derives__` declarations and format-module derive usage.
- **Typechecker / symbol resolution**: must resolve derivable modules, adopted traits, and injected methods according to the protocol rather than a closed derive registry alone.
- **Lowering / emission**: must preserve the selected derive contracts into backend-specific implementations without exposing raw backend mechanics as the public contract.
- **Stdlib / library surfaces**: derivable modules must declare their exported traits and methods coherently enough to participate in generic bounds and method adoption.
- **Docs / tooling**: must explain the difference between compiler intrinsics and extensible derive modules clearly.

## Design Decisions

1. **Derive-time metadata** is out of scope for this RFC and is deferred to future format-specific RFCs.
   - Some formats may need per-model configuration such as JSON naming conventions or Protobuf field numbering strategy.
   - This RFC does not decide whether that metadata belongs in decorator args, field metadata, or a separate mechanism.

2. **Pretty printing** is deferred to the `std.serde.json` implementation rather than settled here.
   - This RFC does not decide whether `.to_json()` should accept formatting options or whether pretty printing should be a separate helper such as `json.pretty(...)`.

## References

- RFC 005 — Rust Interop
- RFC 050 — Enum Methods and Enum Trait Adoption
- RFC 051 — `JsonValue` for `std.json`
- RFC 025 — Multi-Instantiation Trait Dispatch
- RFC 021 — Model field metadata and schema-safe aliases
- RFC 023 — Compilable Stdlib & Rust Module Binding
- Rust `serde` crate (format-agnostic serialization)
- Rust `prost` crate (Protobuf code generation)
- Rust `apache-avro` crate (Avro serialization and schema)
