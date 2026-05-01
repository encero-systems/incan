# RFC 087: Reusable field contracts and structural model composition

- **Status:** Planned
- **Created:** 2026-04-29
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 017 (validated newtypes with implicit coercion)
    - RFC 021 (model field metadata and schema-safe aliases)
    - RFC 048 (checked contract metadata, Incan emit, and interrogation tooling)
    - RFC 085 (field metadata and type-shaped constraints)
    - RFC 086 (schema descriptors and adapters)
- **Issue:** #474
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC introduces reusable field contracts and structural model composition. A module-scope `field` declaration defines a named field contract once, including its canonical field name, metadata, type, defaulting, and extraction anchor. Inside a `model`, every model field is semantically a `field`; the `field` keyword is optional where the grammar can distinguish a local field declaration from a reusable field reference. Models may also compose larger row shapes by spreading existing model-shaped declarations. The result is a way to define common identifiers, audit fields, and joined row shapes without retyping rich field metadata while preserving checked provenance for metadata extraction and blast-radius analysis.

## Core model

1. **Every model field is semantically a field:** `email: EmailAddress` inside a model is sugar for an explicit local field declaration.
2. **Reusable field contracts are explicit at module scope:** top-level reusable fields must use the `field` keyword.
3. **Model-body `field` is optional where unambiguous:** `field user_id` and `user_id` can both reference a reusable field contract inside a model when `user_id` resolves unambiguously to a reusable field.
4. **Composition is structural, not inheritance:** spreading a model-shaped declaration copies field contracts into a target model; it does not create subtyping, method inheritance, or runtime hierarchy.
5. **Conflicts are explicit:** duplicate fields introduced by local declarations, field references, or model spreads must either be identical under this RFC's compatibility rules or be rejected until a future RFC defines reconciliation syntax.
6. **Provenance is preserved:** metadata extraction must record both the original source field anchor and the composed target model-field anchor.
7. **Schema overlays stay separate:** RFC 086 can attach projection metadata to composed models, but schema metadata cannot add, remove, or retype fields.

## Motivation

Large row-shaped systems repeat the same fields everywhere: `tenant_id`, `user_id`, `account_id`, `created_at`, `updated_at`, ingestion timestamps, external IDs, partition keys, and business identifiers. In realistic models those fields often carry metadata, descriptions, aliases, defaults, classifications, schema-adapter mappings, and stable extraction anchors. Repeating that contract by hand across many models is both noisy and dangerous.

The problem becomes sharper when one row shape is derived from another. A query join, warehouse projection, API response model, or contract-backed row may absorb fields from multiple existing row definitions. Users should not have to retype `user_id [description="...", classification="...", ...]: UserId` in every derived shape. At the same time, the language must avoid hidden inheritance behavior and silent field conflicts.

RFC 085 defines what individual fields can carry. RFC 086 defines how schema descriptors and overlays consume model metadata. This RFC defines how checked field contracts and row shapes can be reused before schema overlays or adapters enter the picture.

## Goals

- Add a module-scope `field` declaration for reusable single-field contracts.
- Make `field` an explicit semantic concept for all model fields.
- Allow `field name` inside a model to import a reusable field contract.
- Allow bare `name` inside a model as sugar for `field name` when `name` resolves unambiguously to a reusable field contract.
- Allow `field name: Type` inside a model as an explicit local field declaration equivalent to `name: Type`.
- Reserve spread syntax for model-shaped composition, such as `...AuditFields`.
- Preserve field metadata, defaults, aliases, type constraints, and source anchors when reusable fields are imported into models.
- Preserve provenance from source field contracts to target model fields in checked metadata extraction.
- Define strict duplicate-field and conflict behavior.

## Non-Goals

- Defining schema overlays, schema imports, adapter interpretation, or descriptor projection. Those belong to RFC 086.
- Expanding the field metadata key set. RFC 085 owns field metadata keys and type-shaped constraints.
- Introducing class inheritance, model inheritance, method inheritance, or subtype relationships.
- Defining query syntax or query planner behavior.
- Defining field override or conflict reconciliation syntax beyond rejection.
- Allowing top-level implicit reusable field declarations without the `field` keyword.
- Allowing bare identifiers in model bodies to become arbitrary expressions.

## Guide-level explanation

A reusable field contract is declared once at module scope:

```incan
type UserId = newtype str[pattern="^usr_[a-zA-Z0-9]+$"]
type TenantId = newtype str
type AccountId = newtype str

field user_id [description="Stable user identifier", classification="restricted"]: UserId
field tenant_id [description="Stable tenant identifier", classification="restricted"]: TenantId
field account_id [description="Stable account identifier", classification="restricted"]: AccountId
field created_at [description="Creation timestamp"]: DateTime
field updated_at [description="Last update timestamp"]: DateTime | None = None
```

Inside a model, those reusable fields can be referenced explicitly:

```incan
model User:
    field tenant_id
    field user_id
    email [classification="personal"]: EmailAddress
    lifecycle_status: LifecycleStatus
    field created_at
    field updated_at
```

Inside a model, the `field` keyword may be omitted when the line is a reusable field reference:

```incan
model User:
    tenant_id
    user_id
    email [classification="personal"]: EmailAddress
    lifecycle_status: LifecycleStatus
    created_at
    updated_at
```

The same keyword can be used to make local field declarations explicit, though it is not required:

```incan
model Account:
    tenant_id
    account_id
    field display_name [description="Human-readable account name"]: str
    field archived: bool = false
    created_at
    updated_at
```

Reusable fields remove the need to repeat large field contracts in derived row shapes:

```incan
model UserAccountRow:
    tenant_id
    user_id
    account_id
    email [classification="personal"]: EmailAddress
    account_name: str
    account_role: AccountRole
```

For larger reusable groups, spread syntax remains model-shaped:

```incan
model AuditFields:
    created_at
    updated_at

model User:
    tenant_id
    user_id
    email [classification="personal"]: EmailAddress
    lifecycle_status: LifecycleStatus
    ...AuditFields
```

Composition preserves provenance. A metadata-layer tool should be able to report that `User.user_id`, `UserAccountRow.user_id`, and `SomeApiUser.user_id` all came from the reusable `user_id` field contract, while also treating each target model field as its own checked field anchor.

## Reference-level explanation

### Syntax

This RFC adds reusable field declarations at module scope:

```text
module_field_decl = "field" IDENT field_meta? alias_sugar? ":" type_expr default? ;
```

Inside a model body, this RFC treats ordinary model fields as explicit `field` declarations semantically:

```text
model_field_decl = "field"? IDENT field_meta? alias_sugar? ":" type_expr default? ;
```

Inside a model body, a reusable field contract can be imported by reference:

```text
model_field_ref = "field"? IDENT ;
```

Inside a model body, a model-shaped declaration can be spread:

```text
model_spread = "..." type_path ;
```

The grammar must distinguish `IDENT ":"` as a local model field declaration and bare `IDENT` as a reusable field reference. A bare identifier in a model body must not be parsed as an arbitrary expression statement.

### Module-scope reusable fields

A module-scope `field` declaration defines a reusable field contract. The contract includes:

- canonical field name;
- field metadata from RFC 085;
- alias sugar from RFC 021 where allowed;
- type expression;
- default or default factory where allowed by RFC 085;
- source location and stable reusable-field anchor where available.

Module-scope reusable fields are declarations. They do not allocate storage, emit runtime values, create callable constructors, or introduce top-level execution.

Top-level field declarations must use the `field` keyword. A top-level declaration such as `user_id: UserId` must not be accepted as an implicit reusable field declaration under this RFC.

### Model-local fields

Every declared field inside a model is semantically a `field`, whether the keyword is present or omitted.

These declarations are equivalent:

```incan
model User:
    field email [classification="personal"]: EmailAddress
```

```incan
model User:
    email [classification="personal"]: EmailAddress
```

The explicit `field` keyword may be used for clarity, generated source, or mixed models where users want to visually distinguish field declarations from other model members.

### Reusable field references inside models

Inside a model, `field name` imports the module-scope reusable field contract named `name`.

Inside a model, bare `name` is sugar for `field name` only when `name` resolves unambiguously to a reusable field contract visible in that scope.

This is valid:

```incan
field user_id [description="Stable user identifier"]: UserId

model User:
    user_id
```

This is rejected:

```incan
model User:
    user_id
```

unless a visible reusable field contract named `user_id` exists.

Importing a reusable field contract into a model creates a model field with the reusable field's canonical name, metadata, alias, type, and defaulting behavior. The target model field has its own model-field anchor, and checked metadata must preserve a provenance edge back to the reusable field contract.

### Model spreads

Inside a model, `...ModelName` imports fields from a model-shaped declaration into the target model.

The spread source must resolve to a model-shaped declaration whose fields are statically known. This RFC permits spreading ordinary models and model-shaped field groups written as models. It does not require subtyping or inheritance.

Spreading a model-shaped declaration imports its fields as if each source field contract were referenced individually, while preserving source provenance for each imported field.

Methods, associated functions, derives, implementations, constructors, runtime behavior, and schema overlays are not imported by a model spread under this RFC.

### Duplicate fields and conflicts

A model must not contain two fields with the same canonical field name unless they are identical field contracts under this RFC's compatibility rules.

Two field contracts are identical only if they have the same:

- canonical field name;
- resolved type;
- nullability and optionality;
- alias metadata;
- default/default-factory behavior;
- compiler-semantic metadata;
- standard descriptive metadata;
- namespaced metadata that participates in the base model descriptor.

If duplicate fields differ in any of those facts, the compiler must report a conflict. This RFC does not define local override or reconciliation syntax.

If duplicate fields are identical, the compiler may accept them but must preserve provenance for all source contracts that contributed to the accepted target field.

### Metadata extraction

Checked metadata extraction must represent reusable field contracts and composed model fields without losing provenance.

The extracted representation must preserve at least:

- reusable field identity and stable anchor where available;
- target model-field identity and stable anchor where available;
- provenance edge from target model field to reusable field or spread source field;
- whether the target field was declared locally, imported by `field name`, imported by bare `name`, or imported through `...Model`;
- normalized field metadata after composition;
- duplicate-field merge provenance when identical contracts are accepted;
- rejected conflict diagnostics where a tooling mode exposes failed extraction.

Blast-radius tools can then answer questions such as which models would be affected by changing the reusable `tenant_id` contract, which query-derived row shapes absorbed `user_id`, or which schema overlays depend on a composed field anchor.

### Interaction with RFC 085

Reusable field contracts use the field metadata, defaulting, safe value, and type-shaped constraint rules from RFC 085. This RFC does not add new metadata keys.

When a reusable field contract is imported into a model, its RFC 085 metadata is preserved as checked field metadata on the target model field. The target field also records that the metadata originated from a reusable field contract.

### Interaction with RFC 086

RFC 086 schema blocks and overlays may attach schema metadata to models built with reusable fields and model spreads. Schema metadata still cannot add, remove, reorder, or retype fields.

Schema overlays target the composed model descriptor after field composition. Adapter projection should be able to trace from an overlay field back to the target model field and, where applicable, back to the reusable field contract or spread source field.

## Design details

### Why top-level `field` is required

Top-level declarations should remain explicit. Allowing `user_id: UserId` at module scope to declare a reusable field would make the language look like it accepts arbitrary top-level field-like code. Requiring `field user_id: UserId` keeps module scope declaration-only and makes reusable contracts visible in search, review, and docs.

### Why `field` is optional inside models

Inside a model body, field declarations are already the dominant syntax. Requiring `field` before every local field would make ordinary models noisy:

```incan
model User:
    field tenant_id: TenantId
    field user_id: UserId
    field email: EmailAddress
```

The keyword is therefore optional inside models. The explicit form remains available where clarity matters:

```incan
model User:
    field tenant_id
    field user_id
    field email: EmailAddress
```

### Why bare identifiers are limited

A bare identifier in a model body is only valid as reusable field sugar. It must not become an expression statement or an implicit local declaration. This keeps model bodies declarative and prevents arbitrary top-level-like execution from entering model declarations.

### Why model composition is not inheritance

Rows often need structural reuse without a runtime object hierarchy. A derived row shape from a join or projection wants fields and metadata, not inherited methods, overridden constructors, implicit subtyping, or parent model identity. Treating composition as structural field import keeps the feature narrow and keeps model identity explicit.

### Compatibility / migration

This RFC is additive. Existing model field declarations remain valid because `field` is optional inside model bodies.

Projects can introduce reusable field contracts incrementally by extracting repeated fields into module-scope `field` declarations and replacing repeated local declarations with `field name` or bare `name` references inside models.

## Alternatives considered

1. **Use `...field_name` for reusable single fields**
   - Rejected because spread syntax visually suggests a larger structure. It is too loud for common scalar fields such as IDs and timestamps.

2. **Allow top-level implicit field declarations**
   - Rejected because module scope should remain explicit and declaration-only. `field user_id: UserId` is clearer than `user_id: UserId` at top level.

3. **Use only model spreads and no reusable field declarations**
   - Rejected because many reuse cases are one-field contracts. Requiring users to create tiny one-field models for every shared ID or timestamp is awkward and obscures intent.

4. **Use inheritance**
   - Rejected because the target use case is row-shape reuse and metadata provenance, not subtype polymorphism or runtime behavior inheritance.

5. **Allow local overrides during composition immediately**
   - Rejected for this RFC because override semantics need careful interaction with RFC 085 metadata lanes, RFC 086 schema overrides, stable anchors, and blast-radius provenance.

## Drawbacks

This RFC adds another declaration form and introduces a bare-identifier form inside model bodies. The grammar and diagnostics must be strict so users do not confuse reusable field references with arbitrary statements.

Composition can hide field origins if tooling does not surface provenance well. The metadata extraction requirements are therefore not optional polish; they are part of the feature's correctness story.

Rejecting local override syntax makes the first version simpler but may force users to define a new reusable field contract when they need a small variation.

## Implementation architecture (non-normative)

A useful implementation shape is to normalize model bodies into explicit field entries before downstream checking. Local declarations, explicit reusable field references, implicit bare references, and model spreads can all become checked model-field entries with a source kind and provenance edge.

Reusable field contracts should participate in the same checked metadata extraction family as models. A composed model descriptor should expose both the final field contract and the source contract ancestry.

## Layers affected

- **Parser / AST**: must support module-scope `field` declarations, optional `field` in model-local field declarations, reusable field references in model bodies, and model spread entries.
- **Typechecker / Symbol resolution**: must resolve reusable field references, validate model spread sources, reject unresolved bare field references, detect duplicate-field conflicts, and preserve source provenance.
- **Checked metadata extractor**: must preserve reusable field anchors, target model-field anchors, composition provenance, duplicate identical-source provenance, and conflict diagnostics where available.
- **IR Lowering / Emission**: must lower composed models as ordinary models with explicit fields after composition, without introducing runtime inheritance or extra storage semantics.
- **Stdlib / Runtime**: should expose composed model field metadata through the same reflection surfaces used for ordinary model fields.
- **Formatter**: must format module-scope field declarations, explicit model-body `field` usage, bare field references, and model spreads deterministically.
- **LSP / Tooling**: should provide completion for reusable fields inside models, go-to-definition from model fields to reusable source contracts, hover provenance, duplicate conflict diagnostics, and rename support that respects stable anchors.
- **Docs**: should render reusable field contracts and composed fields without hiding where a field contract originated.
- **Build / Packaging**: should preserve reusable field contracts and composition provenance in artifacts that claim checked metadata support.

## Design Decisions

- Accept duplicate canonical fields introduced by multiple references or spreads only when the contracts are identical under this RFC's compatibility rules. Preserve provenance for every contributing source. Reject non-identical duplicates.
- Allow model spreads to target visible ordinary model-shaped declarations. A dedicated field-group declaration is not part of this RFC.
- Treat visibility as a property of the target model declaration, not the reusable field contract. Importing a reusable field contract into a model does not import standalone visibility.
- Do not define local override syntax in this RFC. Users who need a different contract should define a different reusable field or local field declaration. A future RFC may introduce explicit reconciliation syntax.
- Treat reusable field anchors and target model-field anchors as source identities. Renaming a reusable field contract creates a new identity unless a future migration or rename-marker feature explicitly preserves continuity. External aliases do not preserve the Incan anchor by themselves.
- Formatter behavior must preserve explicit `field name` references when written by the user and must not rewrite them to bare references. Generated source should prefer explicit `field name` for reusable field references in mixed model bodies.
