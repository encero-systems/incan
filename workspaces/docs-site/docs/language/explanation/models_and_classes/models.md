# Models

A `model` is Incan's **data-first** type: you declare typed fields, and you get a predictable data shape that works well
for configs, DTOs, and serialization.

If you're deciding between `model` and `class`, start with the [Models & classes overview](index.md). This page focuses
on how models behave once you've chosen them.

## Quick start

```incan
@derive(Serialize, Deserialize)
model Account:
    type_ [description="Account tier"] as "type": str  # (1)
    is_admin: bool = false  # (2)

def main() -> None:
    a = Account(type="premium")  # (3)
    println(a.type)              # (4)
    println(json_stringify(a))   # (5)
```

1. Per-field schema metadata/aliases are model-only. See [Field metadata and schema-safe aliases](#field-metadata-and-schema-safe-aliases).
2. Defaults make a field optional at construction sites. See [Defining fields and defaults](#defining-fields-and-defaults).
3. Construction is keyword-only, and alias keys may be used. See [Constructing models (keyword-only)](#constructing-models-keyword-only).
4. Member access may use aliases when they are identifier/keyword-shaped. See [Field access](#field-access).
5. Serialization uses aliases as JSON keys. See [Serialization](#serialization).

**The key ideas**:

- **A `model` is field-defined**: the fields are the API surface.
- **Construction is keyword-only**: model constructors use named arguments.
- **Defaults are part of the type**: a default makes a field optional at construction sites.
- **Schema mapping is model-only**: models support field aliases/metadata for wire formats.
- **No inheritance**: use composition instead of `extends`.

!!! info "Glossary"
    - **DTO (Data Transfer Object)**: a type whose job is to carry data between layers or systems (e.g., API payloads)
    - **wire format**: how data looks when sent over the network (e.g., JSON keys)
    - **schema**: the expected shape/structure of data (field names, types)
    - **alias**: an alternate name for a field used in wire formats (e.g., `"type"` instead of `type_`)
    - **metadata**: additional information about a field (e.g., a description or an alias)

??? info "Coming from Rust?"
    Both `model` and `class` compile to Rust `struct`s + `impl`s. A `model` is semantically "this is data";
    a `class` is "this is an object with behavior".

    - `def m(self)` corresponds to an immutable receiver (roughly like `&self`)
    - `def m(mut self)` corresponds to a mutable receiver (roughly like `&mut self`)
    - Models can have methods, but they're typically pure helpers (no mutation)

??? info "Coming from Python (pydantic / dataclasses)?"
    - A `model` is like a pydantic `BaseModel` or a `@dataclass`: you declare fields, and construction is keyword-based.
    - You don't write `__init__`; the declared fields define the constructor keys.
    - Field aliases (`as "type"`) work like pydantic's `Field(alias="type")`.
    - `@derive(Validate)` is the Incan equivalent of pydantic validators / `model_validator`.

??? info "Coming from TypeScript / JavaScript?"
    - A `model` is like a TypeScript `interface` or `type`, but with runtime presence (it compiles to a real struct).
    - Use models for API request/response payloads (DTOs).
    - Field aliases let you keep code-safe names (`type_`) while matching JSON keys (`"type"`).

## Defining fields and defaults

A model is a list of fields with types, optionally with defaults:

```incan
model Config:
    host: str = "localhost"
    port: int = 8080
```

Rules:

- **Required vs optional**:
    - A field **without** a default is required at construction time.
    - A field **with** a default may be omitted (the default is used).
- **Type checking**: default expressions must be compatible with the field type.

## Field metadata and schema-safe aliases

Models support per-field metadata for schema/wire formats:

- `alias="..."`: wire/schema name
- `description="..."`: documentation string
- **sugar**: `name as "wire"` is equivalent to `name [alias="wire"]`

```incan
@derive(Serialize, Deserialize)
model Account:
    type_ [description="Account tier"] as "type": str
```

??? tip "When would I use an alias?"
    Use an alias when:

    - The wire format uses a **keyword** (like `"type"`, `"from"`, `"class"`) that you can't use as an identifier.
    - The wire format uses a **non-identifier** (like `"1"`, `"my-field"`, `"@id"`).
    - You want a **shorter/clearer name in code** while matching an external schema (like "created_at" instead of "createdAt").

### Using aliases in code

When a field has an alias, you may use that alias in three places:

- member access: `a.type`
- constructor keys: `Account(type="premium")`
- destructuring patterns: `Account(type="premium") => ...`

This lets you keep a safe canonical identifier (`type_`) while still matching external schemas (`"type"`). Only aliases
that are identifier/keyword-shaped can be used in code.

### Aliases are strings, not identifiers

Aliases are **wire keys**, so they can be any string (including non-identifier values like `"1"`). Aliases participate in
wire mapping (e.g. JSON) and reflection.

In **code**, only identifier/keyword tokens are accepted in these three positions:

- member access: `obj.<name>`
- constructor named arguments: `Type(<name>=...)`
- destructuring pattern keys: `Type(<name>=pat)`

So non-identifier aliases can't be written in code. In those cases, use the **canonical field name** in code and keep the
alias for wire mapping and reflection.

```incan
@derive(Deserialize, Serialize)
model WeirdWireKeys:
    one [alias="1"]: int

def demo(w: WeirdWireKeys) -> int:
    return w.one  # canonical name in code

# Wire I/O (conceptually):
#   Deserialize: {"1": 7} -> WeirdWireKeys(one=7)
#   Serialize: WeirdWireKeys(one=7) -> {"1": 7}
```

??? info "What counts as a code-spellable alias?"
    A code-spellable alias is one you can write in the three code positions above because it *looks like* an identifier
    (e.g. `"todays_date"`) or a keyword that Incan allows in those positions (e.g. `"type"`).

    Other aliases are valid wire keys but not valid code names (e.g. `"1"`, `"created at"`). Use the canonical field name
    in code; the alias is used for wire mapping and reflection.

### Alias constraints (to avoid ambiguity)

Within a model:

- aliases must be **non-empty** and contain at least one non-whitespace character
- aliases must be **unique**
- aliases must not collide with any canonical field name
- aliases must not collide with any **visible member name** on the model (fields/methods, including built-in helpers and
  members introduced by derives/traits)
- aliases are matched by **exact string equality** (no Unicode normalization, no case-folding)
- leading/trailing whitespace is allowed and **significant** (so `"id"` and `" id "` are different wire keys)

## Constructing models (keyword-only)

Model construction uses **named arguments**:

```incan
model Point:
    x: int
    y: int

def main() -> None:
    p = Point(x=10, y=20)
```

Rules:

- **No positional args**: `Point(10, 20)` is not supported.
- **Unknown fields are errors**: `Point(z=1)` is a type error.
- **Duplicates are errors**: `Point(x=1, x=2)` is a type error.
- **Missing required fields are errors**: if a field has no default, you must pass it.

## Field access

Access a field with dot syntax:

```incan
def area(p: Point) -> int:
    return p.x * p.y
```

If a field has an alias, you may use either the canonical name or the alias for member access (when the alias is
identifier/keyword-shaped). The canonical name always works.

If you need behavior, you can still define methods on a model (often pure helpers):

```incan
model Point:
    x: int
    y: int

    def area(self) -> int:
        return self.x * self.y
```

Models also support `@staticmethod` for methods that belong to the type rather than an instance:

```incan
model Point:
    x: int
    y: int

    @staticmethod
    def origin() -> Point:
        return Point(x=0, y=0)
```

See: [Classes: Static methods](classes.md#static-methods-staticmethod) for full details.

## Serialization

With `@derive(Serialize)` / `@derive(Deserialize)`, a model serializes/deserializes as a JSON object.

If a field has an alias, that alias is used as the JSON key (wire name). This lets you keep schema-safe canonical field
names in code while still matching external payloads.

See: [Derives: Serialization (Reference)](../../reference/derives/serialization.md).

## Validation (`@derive(Validate)`)

If you derive `Validate` on a model, you opt into validated construction:

- you must implement `validate(self) -> Result[Self, E]`
- you construct via `TypeName.new(...) -> Result[TypeName, E]`
- raw construction via `TypeName(...)` is a compile-time error

This is the "data shape + invariant" pattern: the type stays easy to pass around, but construction becomes explicit and
fallible so invariants can't be bypassed by accident.

See: [Derives: Validation (Reference)](../../reference/derives/validation.md).

## Pattern matching and destructuring

Models can be destructured by field name in `match` patterns:

```incan
def describe(a: Account) -> str:
    match a:
        Account(type="premium") => return "Premium"
        Account(type="basic") => return "Basic"
        _ => return "Other"
```

You can use canonical field names or aliases in destructuring keys (when aliases are identifier/keyword-shaped).

## Reflection helpers

Models (and classes) provide:

- `__class_name__() -> str`
- `__fields__() -> FrozenList[FieldInfo]`

For models, `FieldInfo` includes schema-relevant information like `alias`, `description`, and `wire_name`.

See: [Reflection (Reference)](../../reference/reflection.md)

## Common pitfalls

- **Expecting positional construction**: model constructors are keyword-only.
- **Using `class` for schema mapping**: field metadata/aliases are model-only.
- **Expecting aliases to rename fields everywhere**: aliases are for specific key positions and wire mapping; the
  canonical name remains the stable identifier in code.
