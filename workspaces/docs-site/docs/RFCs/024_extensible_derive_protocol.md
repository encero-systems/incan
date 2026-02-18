# RFC 024: Extensible Derive Protocol

- Status: Planned
- Author(s): Danny Meijer (@dannymeijer)
- Issue: —
- RFC PR: —
- Created: 2026-02-17
- Related:
    - [RFC 012] (JsonValue & enum methods)
    - [RFC 021] (field metadata & aliases)
    - [RFC 023] (compilable stdlib & rust.module binding)
    - [RFC 025] (multi-instantiation trait dispatch)

## Summary

This RFC proposes an extensible derive protocol that lets modules declare themselves as **derivable**. A derivable module
exposes a `__derives__` list that declares which of the module's traits are adoptable via `@derive()`. When a type
derives the module, those traits — and their methods — are adopted onto the type. This replaces the current closed
`DeriveId` registry for format-related derives with a trait-based, module-driven mechanism — enabling user-defined
serialization formats, schema generators, and behavioral adapters without compiler changes.

## Motivation

### The derive system is closed

Today, `@derive(Serialize, Deserialize)` is backed by a hardcoded `DeriveId` enum in
`crates/incan_core/src/lang/derives.rs`. The method injection (`to_json`, `from_json`) is wired into the typechecker via
`inject_json_methods()`. Adding a new serialization format requires changes across multiple compiler stages — there is no
user-facing mechanism to create custom derivable traits.

### Serialization isn't just JSON

A natural extension of Incan's model system is serving multiple wire formats from one definition: JSON, YAML, Protobuf,
Avro, Arrow, and more. Each format needs its own serialization/deserialization methods, and users need the ability to
pick exactly which formats a model supports. For example:

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

This model gets `.to_json()`, `.from_json()`, `.to_yaml()`, `.from_yaml()`, `.proto_schema()` — all statically verified,
all type-safe.

### Users need custom derives

Data engineering workflows (steps, pipelines, Readers/Writers) often use internal formats. Teams need to create their own
derivable modules — an internal binary codec, a company-specific schema format, a custom wire protocol — without forking
the compiler or waiting for stdlib additions.

### Injected methods need trait bounds

The current `inject_json_methods()` approach makes `.to_json()` appear on types that derive `Serialize`, but there's no
trait backing it. This means generic functions cannot express "T must be JSON-serializable":

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
  examples of what the protocol *enables*. It does not propose adding those libraries to the stdlib. Each format would
  be introduced by its own RFC or feature issue (e.g., [RFC 012] for `JsonValue`).
- **Migrating built-in derives** (`Eq`, `Clone`, `Debug`, etc.) to the `__derives__` protocol. These remain compiler
  intrinsics handled by the `DeriveId` registry.
  See [Interaction with existing features](#interaction-with-existing-features) for details.
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

`Config` now has `.to_json()` and `.from_json()` methods. The user can see exactly where they come from — the `json`
module defines the traits. See Phase 4 of this RFC for the `std.serde.json` module migration.

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

Both format modules define their own `Serialize` trait, each carrying `@rust.derive("serde::Serialize")`. The compiler
deduplicates to a single Rust-level derive, while each module injects its own distinct methods (`.to_json()` vs
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

The (hypothetical) `sql` module in this example defines a `SqlSchema` trait whose `sql_ddl()` method is implemented
entirely in Incan by iterating over `__fields__()`. The same pattern works for OpenAPI, GraphQL type definitions, or any
text-based schema format.

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

Here `auditable` might define an `Auditable` trait that provides a `.diff(other)` method, a `.changelog()` method, or
field-level change tracking — whatever the module's traits declare. The mechanism is the same regardless of whether the
derive produces bytes, text, or behavior.

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

A module that defines a `__derives__` attribute at module level is a **derivable module**. The attribute lists which of
the module's traits are adoptable via `@derive()`:

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

The traits listed in `__derives__` are adopted by any type that derives the module. This is equivalent to the type
writing `with TraitName` for each listed trait, but driven by the `@derive()` decorator. Only traits explicitly listed
in `__derives__` are adopted — other traits defined in the module are not automatically included.

### Rust derive binding via `@rust.derive`

A trait in a derivable module may need the compiler to emit a Rust `#[derive(...)]` attribute on any struct that adopts
it. This is distinct from `@rust.extern` (which delegates a *method call* to Rust) — `@rust.derive` declares that the
*type itself* requires a Rust-level derive for the trait's methods to work.

The `@rust.derive("path::to::Derive")` decorator on a trait declaration carries this binding:

```incan
@rust.derive("serde::Serialize")
trait Serialize:
    def to_json(self) -> str:
        return to_string(self)?
```

When a type adopts this trait via `@derive()`, the compiler emits `#[derive(serde::Serialize)]` on the Rust struct.

Traits that don't need a Rust-level derive (pure Incan behavioral traits, schema generators using `__fields__()`
reflection) simply omit `@rust.derive` — their methods compile normally without any struct-level annotation.

### Derive deduplication

Multiple modules may declare the same `@rust.derive` path. For example, both `json.Serialize` and `yaml.Serialize`
carry `@rust.derive("serde::Serialize")`. The compiler collects all `@rust.derive` paths from all adopted traits into a
set before emission, producing one `#[derive(serde::Serialize, serde::Deserialize)]` regardless of how many format
modules are derived.

### Individual trait imports

Traits within a derivable module can be imported individually:

```incan
from std.serde.json import Serialize
```

When used in `@derive(Serialize)`, only that single trait is adopted (and its `@rust.derive` path, if any, is emitted).
This enables fine-grained control — derive only serialization, only deserialization, etc.

### Method resolution

When a type derives a module, the module's traits are adopted. Method calls on instances of the type resolve through
normal trait method lookup. If two derived modules define traits with the same method name, this is a compile-time error
(ambiguous method), following normal trait method resolution rules.

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

Trait method injection follows normal trait adoption rules. Methods with `self` receiver become instance methods on the
adopting type. Methods without a receiver become associated functions (e.g., `Model.from_json(s)`).

### Three categories of derivable modules

#### 1. Serialization formats (data in/out)

These convert instances to/from bytes or strings. They use `rust::` interop to call codec libraries. Multiple serde
formats define similarly-named traits (each module has its own `Serialize` / `Deserialize`) that inject distinct methods:

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

No `rust.module()`, no `@rust.extern` — the `.incn` file is the complete implementation. `@rust.derive` declares the
Rust struct-level derive needed for the `rust::` interop calls to work. The `rust::` interop ([RFC 005]) provides access
to the underlying Rust codec library.

#### 2. Schema generators (type shape out)

These generate schema artifacts from the model's type definition. They operate on field metadata via `__fields__()`
reflection and are typically pure Incan:

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

Some formats are **hybrids** — they need both schema generation AND instance serialization (e.g., Avro needs schema
JSON plus binary encode/decode).

#### 3. Behavioral derives

These attach behavior to models without producing bytes or schemas. For example:

|      Module      | `__derives__` |              What it does               |
| ---------------- | ------------- | --------------------------------------- |
| `std.validation` | `[Validate]`  | Checked construction via `.new()`       |
| `std.governance` | `[Governed]`  | PII masking, field-level access control |
| `std.versioning` | `[Versioned]` | API version-aware response shapes       |

### Interaction with existing features

#### Built-in derives (`Eq`, `Clone`, `Debug`, etc.)

Built-in derives remain compiler intrinsics. They are **not** migrated to the `__derives__` protocol because their
implementations are Rust proc macros that generate `impl` blocks — there is no Incan-expressible body to put in a trait.
The `DeriveId` registry continues to handle these.

The distinction is clear: built-in derives implement *language-level semantics* (equality, ordering, cloning, debug
formatting). Format derives implement *library-level functionality* (serialization, schema generation). The protocol
applies to the latter.

> Note: as the language evolves, this might change. It is hypothetically possible to rewrite the built-in derives as
> traits in the stdlib, but that would be a significant change requiring currently unavailable functionality and syntax
> that is not in scope for this RFC.

#### `rust::` imports (RFC 005)

The `rust::` import mechanism is the primary way derivable modules access Rust codec libraries. A derivable module's
trait methods are pure Incan that call into Rust libraries via `rust::` imports. The two mechanisms are complementary.

#### `with` trait bounds (RFC 023)

Traits from derivable modules work with the existing `with` bound syntax. A function can require specific format
capabilities:

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

The `json` module sees `alias`, `description`, etc. The `protobuf` module reads `proto.tag` for stable field numbering.
Each format consumes the metadata it understands and ignores the rest.

### Compatibility / migration

This RFC is **additive** for the protocol itself — `__derives__`, `@rust.derive`, and module-based `@derive()` are new
capabilities. However, it includes one **deprecation**: bare `@derive(Serialize, Deserialize)` will be removed from the
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

The migration is mechanical: add the format import, replace bare `Serialize`/`Deserialize` with the module name. The
generated Rust output is identical. Built-in derives (`Debug`, `Clone`, `Eq`, etc.) are unaffected.

## Alternatives considered

### 1. Decorator-based method injection (current approach)

The status quo: hardcode method injection in the typechecker per derive. Rejected because it doesn't scale to N formats
and provides no trait for generic bounds.

### 2. `__derive__` as a simple list without traits

A module-level `__derive__` that maps to Rust derives, with methods injected by convention (e.g., `to_<format>` always
exists). Rejected because there's no trait to bind against in generic functions, and the method signatures are invisible
to the user.

### 3. Proc-macro-style user derives

Allow users to write Rust proc macros and register them as Incan derives. Rejected because it requires Rust expertise
and breaks the "Incan all the way down" principle. The trait-based approach keeps everything in Incan.

### 4. Making all built-in derives use this protocol too

Migrate `Eq`, `Clone`, `Debug`, etc. to `__derives__`-based modules. Rejected because these are genuinely compiler
intrinsics — their implementations are Rust proc macros that generate `impl` blocks, not callable functions. The
protocol is for library-level functionality.

## Drawbacks

- **Two derive systems**: built-in derives (`DeriveId` registry) and module-based derives (`__derives__` protocol)
  coexist. This is intentional — they serve different purposes — but adds conceptual surface area.
- **Naming collisions**: if a module defines a `Serialize` trait and the user also imports `Serialize` from another
  module, the compiler must disambiguate. Normal trait resolution rules apply, but the error messages need to be clear.
- **Rust derive deduplication**: the compiler must correctly deduplicate underlying Rust derives across modules. This is
  straightforward (collect into a set) but adds a codegen step.

## Implementation plan

### Phase 1: Parser support for `__derives__` and `@rust.derive`

- [ ] Extend the parser to recognize module-level `__derives__ = [...]` as a special attribute
- [ ] Store the derives list in the AST's module metadata (alongside `rust.module()`)
- [ ] Parse `@rust.derive("path")` as a decorator on trait declarations
- [ ] Store `@rust.derive` paths in `TraitDecl` AST metadata
- [ ] Emit compile error for `__derives__ = []` (empty list)
- [ ] Emit compile error if `__derives__` references a name that isn't a trait in the same module
- Touchpoints: `crates/incan_syntax/src/parser/core.rs`, `crates/incan_syntax/src/ast/decls.rs`

### Phase 2: Derive expansion in the typechecker

- [ ] When `@derive(name)` resolves to a module (not a `DeriveId`), read `__derives__` from the module
- [ ] Adopt the listed traits onto the type; inject their methods into the type's method table
- [ ] Collect `@rust.derive` paths from adopted traits for the emission layer
- [ ] When `@derive(name)` resolves to a single trait (imported from a derivable module), adopt only that trait
- [ ] Replace `inject_json_methods()` with the general trait adoption mechanism
- [ ] Add diagnostic for ambiguous method names when deriving multiple modules with conflicting trait methods
- Touchpoints: `src/frontend/typechecker/collect/decl_helpers.rs`, `collect/stdlib_imports.rs`

### Phase 3: Emission deduplication

- [ ] Collect all `@rust.derive` paths from adopted traits, plus `DeriveId`-mapped derives
- [ ] Deduplicate into a set before emitting `#[derive(...)]`
- [ ] Verify that `@rust.derive` with multiple arguments works: `@rust.derive("a::B", "c::D")`
- Touchpoints: `src/backend/ir/emit/decls/structures.rs`

### Phase 4: Migrate `std.serde.json` to the protocol

- [ ] Rewrite `stdlib/serde/json.incn` with `__derives__`, `@rust.derive`, `Serialize` trait, `Deserialize` trait
- [ ] `Serialize.to_json()` returns `str` (serialization of a valid model cannot fail); `Deserialize.from_json()`
  returns `Result[Self, str]`
- [ ] Remove `inject_json_methods()` hardcoding from `decl_helpers.rs`
- [ ] Remove `Serialize` / `Deserialize` from `DeriveId` registry
- [ ] Verify `@derive(json)` works end-to-end (typechecks, lowers, emits correct Rust)
- [ ] Verify `from std.serde.json import Serialize` + `@derive(Serialize)` works for partial derives
- [ ] Add codegen snapshot tests for single-format and partial derives
- [ ] Update existing tests that use bare `@derive(Serialize, Deserialize)` to use `@derive(json)`

### Phase 5: Add a second serde format and a schema generator

- [ ] Implement `std.serde.yaml` following the same pattern as `std.serde.json`
- [ ] Verify multi-format derives work: `@derive(json, yaml)` — correct deduplication, distinct methods
- [ ] Verify `with` trait bounds work across format modules: `T with (json.Serialize, yaml.Serialize)`
- [ ] Implement one schema generator (e.g., `std.schema.sql`) to validate the pure-Incan `__fields__()` reflection path
- [ ] Verify schema generator derives work: `@derive(sql)` with no `@rust.derive` on the trait
- [ ] Add codegen snapshot tests for multi-format and schema generator derives

### Phase 6: Documentation and migration guide

- [ ] Update user-facing docs to show the `from std.serde import json` pattern
- [ ] Document how to create custom derivable modules (user guide)
- [ ] Add deprecation notice for bare `@derive(Serialize, Deserialize)` in release notes

## Design decisions

The following questions were considered during design and are recorded here for posterity.

1. **Trait naming within modules**: modules use short names (`Serialize`, `Deserialize`). Users who need disambiguation
   use import aliasing: `from std.serde.json import Serialize as JsonSerialize`. This keeps module definitions simple
   and pushes naming concerns to the import site where the user has full context.

2. **`__derives__` syntax**: parsed as an implicit const assignment. The dunder convention already signals
   "compiler-recognized"; an explicit `const` keyword would be redundant. It is semantically immutable — reassigning
   `__derives__` is a compile error.

3. **Missing or empty `__derives__`**: a module without `__derives__` is not derivable. A module with
   `__derives__ = []` is a compile error (or at minimum a warning) — an empty list signals a mistake, since there is
   no reason to declare `__derives__` without listing at least one trait.

4. **Bare `Serialize` / `Deserialize` derives**: bare `@derive(Serialize, Deserialize)` ceases to exist as a `DeriveId`
   shortcut. Users import the format module and derive it: `@derive(json)`. If direct access to the Rust serde traits
   is needed, `rust::` interop remains available. This eliminates ambiguity and makes the format dependency explicit.

5. **`@rust.derive` validation**: treated the same as `@rust.extern` — the path string is passed through to the emitted
   Rust code. Validation happens at Rust compile time, not in the Incan compiler. This keeps the protocol simple and
   works with any Rust derive crate without the Incan compiler needing to know about them.

6. **Multiple `@rust.derive` on one trait**: allowed. A single trait may require multiple Rust-level derives. The
   decorator accepts multiple arguments: `@rust.derive("serde::Serialize", "apache_avro::AvroSchema")`.

## Deferred questions

1. **Derive-time metadata**: some formats may need per-model configuration (e.g., JSON naming conventions, Protobuf
   field numbering strategy). Whether this should be decorator args (`@derive(json, rename_all="camelCase")`), field
   metadata, or a separate mechanism is out of scope for this RFC and deferred to future format-specific RFCs.

2. **Pretty printing**: should `.to_json()` accept formatting options (indent, sort keys), or should pretty printing be
   a separate function (e.g., `json.pretty(value, indent=2)`)? Deferred to the `std.serde.json` implementation.

## References

- [RFC 005] — Rust Interop
- [RFC 012] — `JsonValue` Type and Enum Methods
- [RFC 025] — Multi-Instantiation Trait Dispatch
- [RFC 021] — Model field metadata and schema-safe aliases
- [RFC 023] — Compilable Stdlib & Rust Module Binding
- Rust `serde` crate (format-agnostic serialization)
- Rust `prost` crate (Protobuf code generation)
- Rust `apache-avro` crate (Avro serialization and schema)

--8<-- "_snippets/rfcs_refs.md"
