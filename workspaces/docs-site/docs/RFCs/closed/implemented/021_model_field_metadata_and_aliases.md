# RFC 021: Model field metadata and schema-safe aliases

## Status

**Status:** Implemented  
**Author(s):** Danny Meijer (@danny-meijer)  
**Issue:** `https://github.com/dannys-code-corner/incan/issues/94`  
**Related:** RFC 005 (Rust interop), RFC 017 (constrained types)

## Summary

Expands `model` field declarations to support **field metadata** (pydantic-style).

Specifically, this RFC introduces:

- **Field metadata keys**: `alias="..."`, `description="..."`
- **Alias sugar**: `name as "wire": T`
- **Alias-aware name resolution**: member access (`obj.type`) and constructor arguments (`Type(type=...)`)
- **Alias-aware destructuring**: pattern field names resolve via aliases (same rules as constructor arguments)
- **Keyword compatibility**: keywords (e.g. `type`, `class`, `from`) are permitted after `.`, in named arguments, and in
  destructuring pattern field names; elsewhere they remain reserved.

Syntax summary:

```incan
model PseudoModel:
   # Canonical + alias
   field_name [alias="wire"]: Type
   field_name [alias="wire"]: Type = default
   
   # Sugar
   field_name as "wire": Type
   
   # With description
   field_name [description="..."]: Type
   field_name [alias="wire", description="..."]: Type
   field_name [description="..."] as "wire": Type
```

Example:

```incan hl_lines="3"
@derive(Deserialize)
model Account:
    type_ [description="Account tier"] as "type": str
```

## Motivation

External schemas often use names that are reserved keywords in Incan (e.g. `type`, `class`, `from`). This makes it awkward
to model wire formats without either renaming fields (sacrificing fidelity) or adopting non-idiomatic workarounds.

We want to preserve **schema fidelity** (the wire name stays `type`), keep **safe internal identifiers** (`type_`), and
support **idiomatic access** in Incan (`obj.type`, `Type(type=...)`) even when the wire name is a keyword.

Beyond aliases, production models benefit from lightweight, self-describing **field metadata**. Following Python’s
`pydantic.Field(...)` and `dataclasses.field(...)`, this RFC standardizes `description="..."` as a first-class metadata
key so tools can surface documentation (IDE hover, generated docs) without changing compilation semantics.

To keep the surface coherent and extensible, this RFC defines the **syntax and storage** for field metadata and
standardizes only two keys in this RFC: `alias` and `description`.

## Non-goals

- This RFC does not introduce a general-purpose `schema:` block for bulk mappings (out of scope for this RFC). This RFC
  is limited to standardizing per-field metadata on field declarations.

  Example of a possible future `schema:` block (non-normative; exact syntax/semantics are out of scope for this RFC):

  ```incan
  model Account:
      type_: str
      class_: str
      from_: str

      # Example only: bulk field mapping/metadata for readability at scale.
      schema:
          type_:  alias="type",  description="Account tier"
          class_: alias="class", description="Customer segment"
          from_:  alias="from",  description="Source system"
  ```

- This RFC does not define a full pydantic-like “Field(...)" feature surface (constraints, validation semantics, examples,
  etc.). It does create a **slot** for field metadata and standardizes `alias` and `description` with the possibility to
  expand this in the future.
- This RFC does not introduce **arbitrary** field metadata keys for adapter ecosystems (e.g. `spark.*`, `sql.*`,
  proprietary namespaces). Only `alias="..."` and `description="..."` are standardized here; broader, namespaced metadata
  is not part of the scope of this RFC.
- This RFC does not move type constraints into field metadata. Type-level constraints belong to constrained types/newtypes
  (see RFC 017).
- This RFC does not apply field aliases/metadata to `class` declarations. Extending the feature to `class` (especially with
  inheritance) is out-of-scope for this RFC and can be revisited if user demand emerges.
- This RFC does not define protobuf schema/tag rules. Protobuf as a long-term primary interface is captured as a future
  direction below, but the details are out of scope for this RFC.

## Future direction (out of scope for this RFC): protobuf-first model schema (proposal)

Long-term, Incan intends to treat **protobuf** as a primary interface for typed APIs and service boundaries.

This RFC’s goal is to ensure field aliasing/metadata is compatible with a proto-first future without committing to the full
protobuf surface yet.

Non-normative requirements (out of scope for this RFC):

- **Stable field identity**: protobuf field numbers (“tags”) are the wire identity; they must be explicit and stable.
  This avoids accidental breaking changes from field reordering.
- **Nested models**: nested `model` types should map to nested `message` types.
- **Presence semantics**: define how `Option[T]` maps to proto3 `optional` / wrapper semantics.
- **Naming + JSON mapping**: define the relationship between canonical names, aliases, proto field names, and ProtoJSON
  field names (including casing defaults and migration knobs like “accept by name”).
- **Compatibility rules**: reserved tag ranges, rename-vs-retag rules, and diagnostics for collisions.

This RFC does not define whether `alias` is treated as a universal “wire name” by default across formats
(JSON/YAML/TOON/TOML/proto JSON), nor does it define format-scoped overrides.

## Guide-level explanation (how users think about it)

When modeling external data, you typically define a `model` and derive `Deserialize`:

```incan
@derive(Deserialize)
model Account:
    type_ [alias="type"]: str
    class_ [alias="class"]: str
    from_ [alias="from"]: str
```

You can then construct the type using the *wire names* (requires the parser changes described in "Keyword compatibility"
below):

```incan
def f() -> Account:
    return Account(type="premium", class="gold", from="billing-service")
```

And access values using the *wire names* (also requires parser changes):

```incan
def g(a: Account) -> str:
    return a.type
```

Non-identifier wire names are still representable as aliases, but must be accessed using the canonical field name in code:

```incan
@derive(Deserialize)
model WeirdKeys:
    one_ [alias="1"]: int

def demo(w: WeirdKeys) -> int:
    # `w.1` is not legal syntax; use the canonical identifier.
    return w.one_
```

Aliases also participate in destructuring patterns:

```incan
def h(a: Account) -> str:
    match a:
        Account(type="premium") => return "premium"
        Account(type="basic") => return "basic"
```

For the common “just alias this one field” case, you can use sugar:

```incan
@derive(Deserialize)
model Account:
    type_ as "type": str
```

You can also add descriptions for documentation purposes:

```incan
@derive(Deserialize)
model Account:
    type_ [alias="type", description="Account tier (e.g. premium, basic)"]: str
    balance [description="Current balance in cents"]: int
```

You can also combine description metadata with `as` sugar (only `alias="..."` conflicts with `as "..."`):

```incan
@derive(Deserialize)
model Account:
    type_ [description="Account tier (e.g. premium, basic)"] as "type": str
```

## Reference-level explanation (precise rules)

### Syntax

#### Syntax position rationale (why metadata is on the name)

This RFC intentionally attaches both the metadata block and `as` sugar to the **field name**, not to the type, so it
remains compatible with type-level bracket syntax (e.g. constrained primitives / generics) and stays unambiguous as the
type system evolves (see RFC 017).

Preferred forms:

```incan
model Limits:
    max_retries [alias="maxRetries"]: int[ge=0, le=10]
    kind as "type": str
```

Avoid attaching field metadata after the type, because it visually collides with type-level brackets:

```incan hl_lines="3"
model Limits:
    # Hard to visually parse: which brackets are the type vs field metadata?
    max_retries: int[ge=0, le=10] [alias="maxRetries"]
```

#### Field metadata (canonical)

Allow optional **field metadata** immediately after the field name:

```text
field_decl = IDENT field_meta? alias_sugar? ":" type_expr default? ;
field_meta = "[" field_meta_args? "]" ;
```

Metadata arguments are a comma-separated list of **named** key/value pairs:

```text
field_meta_args = field_meta_arg { "," field_meta_arg } ;
field_meta_arg  = IDENT "=" field_meta_value ;
field_meta_value = STRING | INT | FLOAT | BOOL ;
```

For this RFC, the following metadata keys are specified:

- `alias="..."` (string literal) — the wire/schema name for serialization
- `description="..."` (string literal) — human-readable documentation for the field

Rules (this RFC):

- Duplicate metadata keys are compile-time errors (e.g. `name [alias="a", alias="b"]: T` is invalid).
- For the standardized keys in this RFC (`alias`, `description`), values must be **string literals**; non-string values
  are compile-time errors.
- Any other keys are compile-time errors in this RFC.

#### Alias sugar (`as "..."`)

Allow an optional alias sugar immediately after the **field name** (before `:`):

```text
alias_sugar = "as" STRING ;
```

Meaning: `name as "wire": T` is equivalent to `name [alias="wire"]: T`.

- The `as "..."` sugar is syntactic for `alias="..."`
- Other metadata keys (e.g. `description="..."`) may be combined with the `as "..."` sugar.
- If both `alias="..."` and `as "..."` are provided, it is a compile-time error.

#### Defaults

Grammar:

```text
default = "=" expr ;
```

> **Note**: This RFC does not change or specify default-expression semantics; defaults follow existing `model` behavior.

### Semantics

#### Canonical field name vs alias

- Each field has a **canonical name** (the identifier written before metadata).
- A field may additionally declare an **alias** via `alias="..."` (or `as "..."` sugar).
- Within a single type, aliases must be unique and must not collide with **any** canonical field name in the model
  (not just the same field). For example, if a model has fields `type_` and `foo`, an alias of `"foo"` on `type_` is
  an error because it collides with the canonical name of another field.
- Aliases must not collide with any **method name** declared in the same `model`. This avoids ambiguous member access
  (e.g. `obj.type` should never have to choose between a field and a method).
- Collision checks apply to the full **visible member surface** of the model type (fields + methods) to ensure unambiguous
  access.
    - At minimum, this includes all user-declared methods plus any compiler-provided/builtin members and any
      derive-introduced members that are part of Incan’s curated derive contract (RFC 005). Implementations must perform
      this check at typecheck time based on the members that are known to exist in the current compilation configuration.
- If both `alias="..."` and `as "..."` are specified for the same field, it is a compile-time error.
- Aliases must be non-empty string literals that contain at least one non-whitespace code point.
- Aliases are **wire/schema keys**, not identifiers. Aliases may contain characters that are not legal in Incan identifiers
  (e.g. `alias="1"`). Such aliases still work for wire mapping (serde/codegen) and reflection, but they cannot be used in
  member access (`obj.<name>`), constructor named args (`Type(<name>=...)`), or destructuring pattern keys, because those
  syntactic positions accept only `IDENT | KEYWORD` tokens. In those cases, users must use the canonical field name.
- Alias matching uses exact string equality; no Unicode normalization or case-folding is performed. This keeps wire names
  as byte-level identifiers and avoids silent behavior changes.

#### Alias-aware field key resolution (`obj.x`, `Type(x=...)`, `Type(x=pat)`)

The same alias-aware field key resolution is used in three places:

- member access (`obj.x`)
- constructor calls (`Type(x=...)`)
- destructuring patterns (`Type(x=pat)`)

When typechecking a field key `x` in any of these positions:

1. If `x` matches a declared **canonical field name**, resolve/bind to that field.
2. Otherwise, if `x` matches a declared **alias**, resolve/bind to that field.
3. Otherwise:
   - for member access (`obj.x`): continue with normal member lookup (e.g. methods); if no member matches, report
     “unknown member”.
   - for constructor calls / patterns: report “unknown field”.

Diagnostics note:

- When reporting “unknown field” for constructor calls / patterns, implementations should include the set of valid keys
  (canonical field names + aliases that are usable as `IDENT | KEYWORD`) and, where feasible, a “did you mean …” suggestion
  for close matches.

Within a single constructor call or destructuring pattern, it is a compile-time error to provide both the canonical name
and its alias for the same field (duplicate field assignment).

Example:

```incan
Account(type="x", type_="y")  # error: same field provided twice
```

Because alias/canonical collisions are rejected at definition time, this resolution is deterministic (a name cannot validly
refer to multiple fields).

Example (member access fallback):

```incan
model Example:
    size [alias="len"]: int
    
    def len(self) -> int: 
      return 123

# error at definition time: alias "len" collides with method name "len"
```

Without such a collision, `obj.<name>` resolves to a field (canonical/alias) first; only if no field matches does it
fall back to normal member lookup (e.g. methods).

#### Keyword compatibility

Because aliases may be keywords (e.g. `type`), the parser must allow **all** keyword tokens in three specific positions:

- after `.` in member access (`obj.type`, `obj.if`, `obj.def`, etc.)
- as the name of a named argument (`Type(type=...)`, `Foo(if=...)`, etc.)
- as the name of a named field in destructuring patterns (see “Alias-aware field key resolution” above)

This is the simpler grammar approach: rather than enumerating a subset of "safe" keywords, we allow any keyword in these
positions. Resolution then follows the normal alias lookup rules—if there's no alias matching the keyword, it's an error.

EBNF (Extended Backus–Naur Form) helper productions:

```text
member_name       = IDENT | KEYWORD ;
named_arg_key     = IDENT | KEYWORD ;
pattern_field_key = IDENT | KEYWORD ;
```

Outside these three positions, keywords remain reserved.

This RFC does **not** propose making keywords generally usable as identifiers elsewhere (e.g. `let type = 1` remains illegal).

This RFC also does **not** add a "string key" syntax for model field access or constructor arguments. As a result, aliases
like `alias="1"` are valid wire names, but cannot be used in `obj.1` / `Type(1=...)` syntax (they are not valid tokens in
those positions).

#### Serde integration

If a `model` derives `Serialize` and/or `Deserialize` (RFC 005), codegen must emit:

- Rust field name = canonical Incan name (e.g. `type_`)
- `#[serde(rename = "wire")]` for fields with `alias="wire"`

If neither `Serialize` nor `Deserialize` is derived, aliases still participate in frontend name resolution (member access
and named arguments), but do not affect Rust emission.

This RFC does not define any model-level casing/rename policy (e.g. `rename_all`). Where serde attributes are emitted,
field-level `alias` is the source of truth for the wire name.

### Interaction with existing features

- **Keywords**: keywords remain reserved in general; aliases can reuse them via the limited parsing + resolution rules above.
- **Type constraints**: numeric/string constraints should use type-level constrained types / newtypes (RFC 017), not field
  metadata keys like `gt=`. This keeps constraints in the “type” family and keeps field metadata focused on schema/wire
  mapping and other future non-type metadata.
- **Descriptions**: `description="..."` is inert (no semantic effect) and is exposed via reflection (`FieldInfo.description`).
  IDEs and doc tooling may surface it (hover, completion details, generated docs), but compilation must not depend on it.
  This RFC does not define schema-generation derives (e.g. JSON Schema / OpenAPI / proto docs).
- **Pattern matching**: aliases participate in destructuring patterns, using the same name resolution rules as constructor
  calls (canonical first, then alias).
- **Spread/rest patterns**: if a feature exists that captures remaining fields into a dict (e.g. `**kwargs`), the captured
  keys are canonical field names (not aliases).
- **Reflection**:
    - `__fields__()` returns `List[FieldInfo]` (rich field info), instead of `List[str]` (just names). This is a
      **breaking change**.
    - Migration for the previous behavior: `[f.name for f in Model.__fields__()]`

#### `FieldInfo`

`FieldInfo` is a frozen `class` in the **language core** (not a `model`) that describes a single model field.

Implementation notes (Rust):

**Things that were changed while implementing**: the generated Rust `__fields__()` returns a `FrozenList[FieldInfo]` backed
by static data, the runtime `FieldInfo` value type uses frozen containers (`FrozenStr`, `FrozenDict`) for zero-allocation
backing, and the reflection trait was renamed to `HasFieldInfo` to avoid a name collision with the value type.
These implementation details were captured in docs and PRs.

- Today, generated Rust uses a reflection trait named `incan_stdlib::reflection::FieldInfo` and derives it via
  `incan_derive::FieldInfo`.
- This RFC introduces a **value type** named `FieldInfo` (returned by `__fields__()`), which is distinct from the existing
  Rust reflection trait. The implementation must avoid name collisions on the Rust side by either:
    - renaming the existing Rust trait/derive (e.g. `FieldInfo` → `FieldInfoTrait` / `DeriveFieldInfo`), or
    - introducing the new value type under a different Rust name/module path and mapping it to the Incan `FieldInfo` class.
- Regardless of the Rust layout, the Incan surface must treat `FieldInfo` as always-available (no imports required).

```incan
# core-provided, frozen record type
class FieldInfo:
    name: str                 # canonical Incan identifier
    alias: Option[str]        # wire name, if present
    description: Option[str]  # human-readable documentation, if present
    wire_name: str            # alias if present, otherwise name
    type_name: str            # canonical display name (stable format), e.g. "str", "int[ge=0]", "Option[str]"
    has_default: bool         # whether the field has a default value
    extra: dict[str, str]     # reserved; always empty in this RFC
```

Notes:

- `FieldInfo` is a core built-in and is immutable (frozen) by definition; user code cannot mutate its fields.
- `wire_name` is derived as `alias` if present, otherwise `name`, and is stored as a field populated by the compiler/runtime.
- `type_name` is the canonical display string produced by the typechecker (fully-resolved and stable); it is not required
  to preserve the source spelling. Type aliases are expanded, and constraints are included where applicable
  (e.g. `int[ge=0]`).
- In this RFC, `extra` is always populated as an empty dict by the compiler/runtime.
- `extra` is reserved for future adapter-specific annotations; its keys/meaning are not specified here, but the field name
  and type are stable. Consumers should treat it as opaque until a future RFC defines concrete keys.
- Supporting richer metadata value types (e.g. lists/objects) is not part of the scope of this RFC.

### Errors / diagnostics

The compiler should produce targeted errors for:

- non-string `alias=` values
- non-string `description=` values
- duplicate field metadata keys (e.g. two `alias=` entries)
- duplicate aliases
- alias colliding with canonical field name
- unknown metadata keys
- empty string alias (`alias=""`) or whitespace-only alias
- alias colliding with a method name on the model
- same field provided via canonical name and alias in a single constructor call or pattern

**Example error messages:**

```text
error: alias "type" collides with canonical field name
  --> models.incn:5:5
   |
 4 |     type: int
 5 |     kind [alias="type"]: str
   |           ^^^^^^^^^^^^ alias "type" already used as canonical name on line 4

error: duplicate alias "wire_name"
  --> models.incn:4:5
   |
 3 |     foo [alias="wire_name"]: str
 4 |     bar [alias="wire_name"]: int
   |           ^^^^^^^^^^^^^^^^ alias "wire_name" already declared on line 3

error: alias must be a non-empty string literal
  --> models.incn:3:11
   |
 3 |     foo [alias=""]: str
   |           ^^^^^^^^ empty string not allowed
```

Diagnostics expectations:

- The typechecker should surface these as normal type errors with precise spans pointing at the offending metadata key/value
  (or alias sugar) so editors can underline them immediately.
- The LSP should report these diagnostics without requiring a full build/codegen step (same behavior as other syntax/type
  errors today).

LSP completion expectations:

- Member completion after `.` should include both canonical field names and aliases.
- If a field has an alias, the alias should be ranked above the canonical name (preferred insertion) to keep access
  ergonomic (e.g. `obj.type` instead of `obj.type_`).
- Completion item details should clarify the mapping (e.g. show `type → type_`) so users can discover the canonical name
  when needed.
- Completion items should be de-duplicated and linked so selecting one suggestion inserts a single identifier (not both
  the alias and the canonical name).

## Impact / compatibility

- **Additive syntax**: Field metadata (`name [alias="..."]: T`, `name [description="..."]: T`) and alias sugar
  (`name as "wire": T`) are new syntax forms. Existing valid programs are unaffected; some previously-invalid programs
  will now parse (notably keyword member access, named args, and destructuring keys).
- **Keyword compatibility is deliberately narrow**: keywords are permitted only
    - after `.`
    - as constructor named-argument keys
    - as destructuring pattern field keys
  Elsewhere, keywords remain reserved.
- **Behavioral**: Name resolution for model fields becomes alias-aware:
    - Member access consults canonical field names first, then aliases; if no field matches, it falls back to normal member
      lookup (methods).
    - Constructor named args and destructuring keys consult canonical first, then alias; if no field matches, it is an error.
- **Breaking change**: `Model.__fields__()` changes from `List[str]` to `List[FieldInfo]`.
    - Migration (canonical names): `[f.name for f in Model.__fields__()]`
    - Migration (wire names): `[f.wire_name for f in Model.__fields__()]`

## Alternatives considered

- **Allow keywords as identifiers globally** (raw identifiers / quoted identifiers): broad surface area; impacts many
  language contexts and does not directly address schema mapping needs (aliases, multiple names, etc.).
- **`schema:` block**: useful for very large mappings, but disjoint from the declaration site; not part of the scope of
  this RFC.
- **Decorator-based field attributes**: not Pythonic for fields; creates noisy inline decorator stacks.
- **Pydantic-like metadata wrapper values** (e.g. `name: T = Meta(alias="wire")`): expressive, but overloads the meaning
  of default values and risks turning “field metadata” into a runtime value story rather than a static schema/type story.
- **Separate `__field_info__()` method**: keep `__fields__() -> List[str]` and add a new `__field_info__() -> List[FieldInfo]`
  for rich metadata. Rejected because it fragments the API; since we're pre-1.0, enriching `__fields__()` directly is cleaner
  and more future-proof.

## Drawbacks

- Adds syntax surface (field metadata brackets + `as` sugar).
- Requires consistent behavior across parsing, typechecking, and codegen, especially around keywords in member/named-arg
  positions.

## Implementation plan

- **Syntax / AST (`crates/incan_syntax`)**:
    - Parse field metadata on `model` fields: `name [alias="..."]` and `name [description="..."]`.
    - Parse `as "..."` alias sugar and lower it to `alias="..."` in the AST (or an equivalent canonical representation).
    - Allow `IDENT | KEYWORD` in:
        - member access name (`obj.<name>`)
        - named-arg keys (`Type(<name>=...)`)
        - destructuring pattern field keys
    - Ensure non-identifier aliases (e.g. `alias="1"`) remain valid as wire names but do not become usable in those syntax
      positions (no `obj.1` / `Type(1=...)`).
- **Typechecker / name resolution (`src/frontend/typechecker`)**:
    - Extend model field typing to store `alias` and `description` metadata.
    - Build an alias map per model and validate:
        - duplicate aliases
        - alias colliding with any canonical field name in the model
        - alias colliding with the visible member surface (fields + methods), including derive/trait-introduced members
        - empty/whitespace-only alias
    - Apply alias-aware resolution in:
        - member access
        - constructor named args
        - destructuring patterns
      including the “canonical wins over alias” rule and the “alias + canonical in the same call/pattern is an error” rule.
- **Backend / IR / codegen (`src/backend`)**:
    - Carry alias/description metadata through IR as needed by codegen and reflection.
    - Rust emission:
        - Keep Rust field identifiers as canonical names.
        - Emit `#[serde(rename = "...")]` when `Serialize`/`Deserialize` derive is present.
    - Reflection emission:
        - Update `__fields__()` codegen to return `List[FieldInfo]` instead of `List[str]`.
        - Populate `FieldInfo` fields (`name`, `alias`, `description`, `wire_name`, `type_name`, `has_default`, `extra={}`).
- **Core/runtime crates**:
    - `crates/incan_core`: keep canonical names/IDs for the built-in `FieldInfo` surface (language vocabulary only).
    - `crates/incan_stdlib`:
        - introduce a Rust runtime **value type** backing the Incan `FieldInfo` class (the elements returned by
          `Model.__fields__()`).
        - reconcile naming with the existing Rust reflection trait named `FieldInfo` (rename/namespace to avoid conflict).
    - `crates/incan_derive`:
        - update derives/imports as needed if the reflection mechanism changes (today codegen imports
          `incan_derive::{FieldInfo, IncanClass}`).
- **Tooling**:
    - Formatter (`src/format`): preserve a stable layout for `name [alias="wire"]: T` and `name as "wire": T`.
    - LSP (`src/lsp`): diagnostics + completion behavior per RFC (include aliases, rank aliases higher, de-duplicate).
- **Docs**:
    - Add a reference section describing schema-safe naming (`type_`) + aliasing and examples.
    - Document migration from `__fields__() -> List[str]` to `List[FieldInfo]`.
- **Tests**:
    - `crates/incan_syntax`: parser tests for:
        - `name [alias="..."]: T`
        - `name [description="..."]: T`
        - `name as "...": T`
      plus keyword-in-position parsing behavior.
    - `src/frontend/typechecker`: tests for alias-aware resolution + collision/duplicate diagnostics.
    - Snapshot tests verifying:
        - serde rename attributes are emitted correctly
        - `__fields__()` returns `FieldInfo` with correct values
    - Edge case tests:
        - Alias that is a special identifier (`"self"`, `"super"`, `"_"`)
        - Empty string alias (error)
        - Whitespace-only alias (error)
        - Non-identifier alias (e.g. `alias="1"`) remains valid as a wire name, but is not usable in
          member/named-arg/pattern syntax
        - Unicode aliases (`alias="日本語"`)
        - Unicode normalization variants (NFC vs NFD) are treated as distinct (exact string equality)
        - Case variants are treated as distinct (no case-folding)
        - Alias matching a method name on the model (error)
        - Alias collision with another field's canonical name (error)

**IDE rename behavior:**

When an IDE performs a “rename symbol” on the **canonical field identifier**, it should rename only the identifier, not
any alias string literals. The alias represents the external wire/schema name and usually must stay stable for compatibility.

Example:

```incan
# before (canonical name = type_, wire name = "type")
type_ [alias="type"]: str

# after rename (canonical name changes, wire name stays the same)
kind [alias="type"]: str
```

## Checklist (comprehensive)

This RFC can be considered "implemented" when the following are complete.

### Spec / semantics

- [ ] Lock down the canonical metadata keys and their meaning:
    - [ ] `alias="..."` is the wire/schema name (must be a non-empty, non-whitespace string literal)
    - [ ] `description="..."` is inert documentation (no semantic effect)
    - [ ] `alias` and `description` values must be string literals (non-string values are errors)
    - [ ] duplicate metadata keys are errors
- [ ] Define and enforce deterministic resolution rules:
    - [ ] canonical name wins over alias
    - [ ] alias collisions with canonical names are errors
    - [ ] duplicate aliases are errors
    - [ ] alias collisions with model methods and other known members (builtin/derive-introduced) are errors
- [ ] Confirm keyword handling is limited to the specified positions (not "raw identifiers" generally).
- [ ] Confirm string matching rules for aliases (exact string equality; no NFC/NFD normalization; no case-folding).

### Syntax / AST / formatting

- [ ] Parser (`crates/incan_syntax`): support field metadata brackets on model fields.
- [ ] Parser (`crates/incan_syntax`): support alias sugar `name as "wire": T` as sugar for `alias="wire"`.
- [ ] Parser: error if both `alias="..."` and `as "..."` are present for the same field.
- [ ] Parser (`crates/incan_syntax`): allow keyword tokens after `.`, as named-arg keys, and as destructuring field keys.
- [ ] AST (`crates/incan_syntax`): represent field metadata (alias/description) explicitly on model field declarations
  (not as ad-hoc strings).
- [ ] Formatter (`src/format`): produce and preserve stable formatting for both bracket metadata and `as` sugar.

### Frontend (typechecker)

- [ ] Build an alias map per model and validate all collision/error cases with precise spans (`src/frontend/typechecker`).
- [ ] Member access:
    - [ ] resolve `obj.<name>` via canonical then alias before falling back to method/member lookup
    - [ ] produce a clear error when the name is neither a field (canonical/alias) nor a member
- [ ] Constructor named arguments:
    - [ ] resolve keys via canonical then alias
    - [ ] error on unknown keys (with "did you mean ..." where feasible)
    - [ ] error if both canonical name and alias are provided for the same field in one call
- [ ] Destructuring patterns:
    - [ ] resolve keys via canonical then alias
    - [ ] preserve any existing semantics around rest/spread (captured keys are canonical)
    - [ ] error if both canonical name and alias are provided for the same field in one pattern
- [ ] Diagnostics:
    - [ ] unknown metadata keys are errors with spans on the key
    - [ ] duplicate metadata keys are errors with spans on the duplicate key
    - [ ] bad metadata values are errors with spans on the value
    - [ ] empty or whitespace-only alias is rejected
    - [ ] duplicate field via alias + canonical is rejected

### Backend (IR / codegen)

- [ ] Carry alias/description metadata through IR as needed by codegen/reflection (`src/backend`).
- [ ] Rust emission:
    - [ ] emit `#[serde(rename = "...")]` for aliased fields when `Serialize`/`Deserialize` is derived
    - [ ] do not emit serde rename attributes when no serde derive is present

### Runtime / reflection

- [ ] Define the Incan `FieldInfo` as a frozen core `class` (always available; no imports).
- [ ] Rust runtime: introduce a `FieldInfo` **value type** backing the class (`crates/incan_stdlib`).
- [ ] Rust runtime: resolve naming conflict with the existing `incan_stdlib::reflection::FieldInfo` trait (rename or
  namespace) and update `incan_derive`/codegen imports accordingly.
- [ ] Update `__fields__()` to return `List[FieldInfo]` with:
    - [ ] `name`, `alias`, `description`, `wire_name`, `type_name`, `has_default`
    - [ ] `wire_name` derived as alias-or-name
    - [ ] `extra` populated as `{}` (empty dict) per this RFC

### Tooling / IDE

- [ ] LSP diagnostics reported without requiring backend/codegen (`src/lsp`).
- [ ] Completion:
    - [ ] include canonical names and aliases
    - [ ] rank aliases above canonical
    - [ ] de-duplicate suggestions and insert a single identifier
    - [ ] show mapping detail (e.g. `type → type_`)
- [ ] Rename behavior: renaming canonical field identifier does not modify alias string literals.

### Tests

- [ ] Parser tests (`crates/incan_syntax`):
    - [ ] bracket metadata parsing
    - [ ] `as` sugar parsing
    - [ ] keyword member/named-arg/pattern positions
- [ ] Typechecker tests:
    - [ ] alias-aware member access (`obj.type`)
    - [ ] alias-aware constructor keys (`Type(type=...)`)
    - [ ] alias-aware destructuring keys
    - [ ] duplicate field via alias + canonical in a single call/pattern
    - [ ] all collision/duplicate/empty-alias errors
- [ ] Codegen snapshot tests:
    - [ ] serde rename attributes emitted correctly
- [ ] Reflection tests:
    - [ ] `__fields__()` returns `FieldInfo` objects with correct `wire_name`/`alias`/`description`/`has_default`
    - [ ] Rust snapshots updated to reflect any rename/namespace changes for the existing Rust `FieldInfo` derive/trait
      (if changed as part of implementing this RFC)
- [ ] Edge cases:
    - [ ] `alias="self"` resolves correctly via `obj.self` (no collision with receiver-only `self` keyword)
    - [ ] `alias="super"` resolves correctly via `obj.super` (no collision with import-path `super` keyword)
    - [ ] non-identifier alias (e.g. `alias="1"`) is allowed as a wire name but not usable in member/named-arg/pattern syntax
    - [ ] Unicode aliases (including NFC vs NFD treated as distinct)
    - [ ] case variants treated as distinct
    - [ ] alias matching a method name (error)
    - [ ] alias colliding with another field's canonical name (error)

## Out of scope for this RFC

- Bulk schema blocks (`schema:`) and external schema sources/imports.
- Namespaced metadata keys (e.g. `spark.*`, `sql.*`) and adapter-boundary validation rules.
- Field inclusion/flattening mechanisms (“field bundles” / model composition).
- Model-level bulk casing policies (e.g. `@serde(rename_all="camelCase")`).
