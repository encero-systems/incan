# RFC 086: Schema descriptors and adapters

- **Status:** Planned
- **Created:** 2026-04-29
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 021 (model field metadata and schema-safe aliases)
    - RFC 048 (checked contract metadata, Incan emit, and interrogation tooling)
    - RFC 082 (checked API documentation generation)
    - RFC 085 (field metadata and type-shaped constraints)
    - RFC 087 (reusable fields and complete model contract composition)
    - RFC 103 (secret values and non-disclosure)
    - RFC 137 (named enum payload fields)
    - RFC 141 (structured type reflection and matching)
- **Issue:** #473
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC defines the schema adapter layer that sits on top of checked model metadata. It introduces structured schema descriptors for model types, `schema:` blocks for readable large metadata sections, named schema overlays for expanding the schema metadata of existing models, declarative `schema from ...` imports for external metadata sources and metadata-only reuse, deterministic merge rules, and an explicit adapter consumption model. The compiler owns schema descriptor shape, metadata normalization, stable anchors, overlays, reproducible imports, and extraction into metadata artifacts. Libraries own interpretation for formats such as Arrow, SQL, JSON Schema, Protobuf, Open Data Contract Standard, and proprietary ecosystems.

## Core model

1. **Models are schema authorities:** field existence, order, type, nullability, defaults, aliases, and metadata originate from checked model declarations and merged schema metadata.
2. **Descriptors are structured:** adapters consume structured model, field, type, default, and metadata descriptors, not parsed source or stringified type expressions.
3. **Heavy metadata moves out of field brackets:** `schema:` blocks, named schema overlays, and `schema from ...` imports keep large adapter/domain mappings readable.
4. **Existing models can be expanded without reopening them:** a named schema overlay can attach additional metadata to an existing model without changing the model declaration.
5. **Metadata merge is explicit:** imports, schema blocks, overlays, and inline metadata merge deterministically with conflict and override rules.
6. **Adapters are explicit:** metadata such as `postgres.name="user_id"` or `proto.tag=12` is inert until an explicit adapter consumes the descriptor.
7. **Schema sources are declarative:** schema imports must not execute user code or fetch network resources.
8. **External schemas cannot redefine the model:** schema imports, schema blocks, and overlays may attach metadata only to existing fields.
9. **Projections are not truth:** generated SQL, JSON Schema, Protobuf, documentation, and data contract files are projections of checked descriptors.
10. **Descriptors feed metadata-layer tools:** normalized schema descriptors and overlays must be extractable as checked metadata so blast-radius, compatibility, catalog, and governance systems can reason from stable anchors rather than source scraping.
11. **Source types remain authoritative:** field types and constraints come from checked Incan type descriptors. Namespaced metadata may request an explicit target mapping, but the owning adapter must validate compatibility before using it; a request cannot redefine the source field contract.

## Motivation

RFC 085 makes individual model fields rich enough to carry row-contract facts. That is not sufficient for large adapter ecosystems. A single field may need SQL, JSON, Protobuf, warehouse, governance, and UI metadata. Inline brackets become unreadable when they carry all of that.

The language needs a second layer: a readable way to attach larger metadata maps to model fields, a way to extend existing models with named schema profiles, and a structured descriptor API that lets libraries consume those maps without scraping source or reverse-engineering generated Rust.

This is the foundation for model-driven schema adapters. Users should be able to define a model once, then derive Arrow schemas, SQL DDL, JSON Schema, Protobuf messages, Open Data Contract Standard YAML, catalog records, and custom mappings from the same checked descriptor.

## Goals

- Define a structured schema descriptor API for model types.
- Expose field types as structured type descriptors, not authoritative strings.
- Add `schema:` blocks inside model bodies for readable field metadata sections.
- Add named `schema Name for Model:` overlays for expanding existing model schemas without editing or reopening the model.
- Add `schema from "./file"` imports for declarative external metadata.
- Add `schema from OtherModel` imports for metadata-only reuse by field name.
- Define deterministic merge and precedence rules across schema imports, schema blocks, named overlays, and inline field metadata from RFC 085.
- Keep adapters explicit and library-owned.
- Track schema import files as build inputs.
- Preserve schema descriptors in checked metadata artifacts where supported.
- Preserve descriptor provenance, overlay identity, and merge decisions so metadata-layer and blast-radius consumers can explain which source changed a downstream projection.
- Define `schema` as projection context: external names, wire identity, inclusion, layout hints, documentation grouping, catalog terms, governance labels, adapter-owned options, and explicitly requested compatible target mappings.

## Non-Goals

- Defining the field metadata key set. RFC 085 owns field metadata keys and type-shaped constraints.
- Validating every adapter-specific metadata namespace in the compiler.
- Executing user code while importing schema metadata.
- Fetching remote schema sources.
- Allowing schema metadata to add, remove, reorder, or retype model fields.
- Defining target-system type semantics in the compiler or changing checked Incan source types through schema metadata.
- Reopening models or adding inheritance semantics through schema overlays.
- Defining batch-level quality, lineage, dependency indexing, blast-radius analysis, promotion gates, or evidence storage.
- Defining one universal data contract manifest format.
- Making generated adapter outputs a source of truth.

## Guide-level explanation

Small metadata stays inline:

```incan
model User:
    email_id [description="Primary email address"]: EmailAddress
    created_at: DateTime
```

Heavy adapter metadata moves to a `schema:` block:

```incan
model User:
    email_id [description="Primary email address"]: EmailAddress
    created_at: DateTime

    schema:
        postgres:
            email_id.name = "email_address"
            created_at.name = "created_at"
            created_at.index = "events_created_at_idx"

        proto:
            email_id.tag = 12
            created_at.tag = 13
```

Shared metadata can be imported:

```incan
model User:
    email_id: EmailAddress
    created_at: DateTime

    schema from "./user.schema.yaml"

    schema:
        email_id:
            description := "Primary customer-facing email address"
```

Existing models can be expanded with named schema overlays:

```incan
model User:
    id: UserId
    email: EmailAddress
    created_at: DateTime

schema UserWarehouse for User:
    postgres:
        id.name = "user_id"
        email.name = "email_address"
        created_at.name = "created_at"
        created_at.index = "users_created_at_idx"

schema UserContract for User:
    odcs:
        owner = "growth"
        terms = ["customer", "identity"]
```

The model remains the authority for fields and types. The overlays are named metadata profiles for that model.

Adapters consume checked descriptors explicitly:

The `schema_of` import below is the proposed standard reflection API. The `sql_adapter` package and its `ddl_from_schema` function represent an application-selected adapter library, not a built-in Incan API.

```incan
from std.reflection import schema_of
from sql_adapter import ddl_from_schema

def emit() -> str:
    return ddl_from_schema(schema_of[User, UserWarehouse]())
```

The compiler stores and exposes namespaced metadata such as `postgres.*`, but it does not define target-system types through schema metadata. The adapter maps checked Incan type descriptors to its target representation.

## Reference-level explanation

### Schema descriptors

The compiler and standard reflection surface must expose a structured schema descriptor for model types.

The canonical request API is `schema_of[Model]()`. The structured descriptor must represent at least:

- model name;
- model stable anchor where available;
- ordered fields;
- field stable anchors where available;
- canonical field names;
- field aliases;
- field types as structured type descriptors;
- nullability and optionality;
- default/default-factory metadata;
- declared field and whole-model validation identities and retained model contract sources;
- compiler-semantic metadata;
- standard descriptive metadata;
- namespaced metadata;
- source/provenance sufficient for diagnostics and documentation.
- extraction provenance sufficient for metadata-layer, compatibility, and blast-radius tooling.

Type descriptors must be structured. A string spelling may be exposed for display, but it must not be the authoritative type representation.

Schema descriptors may be requested for the base model with `schema_of[Model]()` or with selected overlays using `schema_of[Model, OverlayA, OverlayB]()`. The descriptor identity must include the base model and selected overlays in order so tools can distinguish `User`, `User` with warehouse metadata, and `User` with contract metadata.

### Type mapping boundary

The checked Incan type descriptor remains the authority for a source field's type and declared constraints. Namespaced schema metadata may request a target-system, storage, wire, or adapter-specific mapping, including a key such as `postgres.type`. The compiler validates metadata syntax, safe values, field references, and merge conflicts; the owning adapter defines target type semantics and must validate the selected request against the checked source descriptor before generating or using the projection. Retaining an unconsumed request in metadata does not certify its compatibility.

Target-mapping requests must use checked adapter-owned enum values rather than strings encoding SQL or another target type language. The adapter package declares the enum; the compiler checks case identity and payload names and types, while the adapter validates target semantics and source compatibility. For example, a proposed PostgreSQL adapter package could supply:

```incan
# Proposed declaration supplied by the postgres_adapter package.
pub enum PostgresType:
    Text
    VarChar(length: int)
```

The following model and schema overlay are application declarations using proposed RFC syntax. The imported PostgresType is supplied by that adapter package, not by the compiler or standard library:

```incan
from postgres_adapter import PostgresType

model Customer:
    name: str

schema CustomerWarehouse for Customer:
    postgres:
        name.type = PostgresType.Text
```

An adapter may accept that request only under its checked mapping contract. Changing the request to `PostgresType.VarChar(length=200)` must not silently impose a 200-character domain limit on the unconstrained source str. PostgreSQL documents [varchar(n) as length-limited and text as having no declared length limit](https://www.postgresql.org/docs/current/datatype-character.html). An incompatible or unestablished mapping must receive a diagnostic; where a source descriptor already carries sufficient compatible constraints, an adapter may validate a corresponding target mapping. The compiler must not inspect arbitrary validator bodies to manufacture a compatibility proof.

RFC 085 safe metadata rules permit structural enum-case values with recursively safe payloads, so the VarChar request does not execute a constructor or arbitrary user code. Descriptors must retain its checked enum identity, case identity, and named length payload. Adapters must consume that structure directly and render SQL only at the output boundary; they must not recover type choices by parsing display strings. A payload having the declared int type does not establish a valid target length: adapter validation must also reject unsupported or invalid parameter values.

Compatibility checking must account for the source type, declared constraints, nullability, and the adapter's representation contract. A mapping request must not silently narrow the checked source contract, discard information, introduce unchecked coercion, or weaken RFC 103 secret protection. Unsupported mappings must not silently fall back to another target type. An explicit metadata override chooses a request; it cannot waive compatibility checking or alter Incan construction, field access, validation, or source type identity.

Other projection facts, such as column names, JSON property names, Protobuf tags, partition hints, index names, and documentation grouping, retain their normal adapter-owned interpretation. Target mapping requests remain namespaced projection metadata rather than a second source type declaration.

### Projection context

`schema` models projection context. Projection context is adapter-facing metadata that cannot be derived from the checked Incan model descriptor and should not live in adapter-wide configuration.

Schema metadata may describe:

- external names, such as database column names, JSON property names, CSV headers, or partner-facing field names;
- wire identity, such as Protobuf field tags or other stable non-type identifiers required for compatibility;
- projection inclusion or exclusion, such as whether a field appears in a public API projection;
- projection layout hints, such as partition keys, clustering keys, sort keys, index names, or grouping hints;
- documentation presentation, such as generated-doc titles, grouping, display order, or projection-specific descriptions;
- catalog and governance terms that are projection-specific rather than intrinsic field facts;
- adapter-owned options and explicit target-mapping requests that affect the projection without redefining the field's checked type and must pass the owning adapter's compatibility checks.

Schema metadata should not repeat facts an adapter can derive from the checked descriptor plus its own configuration. If the adapter can infer a fact from field name, field type, field order, nullability, defaulting, alias metadata, or adapter configuration, that fact should not be written in `schema` unless it is intentionally overriding projection context.

### Descriptor extraction and metadata-layer use

Schema descriptors are part of the checked metadata surface, not only a runtime reflection convenience. For packages or artifacts that claim RFC 048 metadata support, descriptor extraction must preserve enough information for downstream tools to index, diff, and explain schema-affecting changes without parsing source text.

The extracted descriptor representation must preserve at least:

- package, module, model, and overlay identity;
- stable model and field anchors where available;
- selected overlay list and overlay merge order;
- schema import sources as build inputs;
- canonical field names, aliases, field order, nullability, defaults, and structured type descriptors;
- normalized field metadata after base merge;
- overlay metadata maps before and after selected overlay merge;
- source/provenance for inline metadata, schema block statements, imports, and overlay statements;
- explicit override records from `:=`;
- conflict diagnostics or rejected metadata where a tooling mode exposes failed extraction.

An extracted target-mapping request must remain distinguishable from a mapping validated by a selected adapter. A validated projection must retain the requested mapping, the checked source field identity and constraints, adapter identity and version where available, and its compatibility outcome. Merely extracting or merging metadata must not manufacture a successful adapter validation.

Blast-radius and metadata-layer systems can use this extracted descriptor to answer questions such as which warehouse columns, API properties, ODCS fields, docs pages, generated Protobuf tags, or partner projections are affected by a model field change. Those systems may attach mutable evidence, ownership, review, confidence, lineage, and rollout state to descriptor anchors, but those records remain downstream metadata unless a future RFC promotes them into the checked descriptor.

### Schema blocks

A `schema:` block may appear inside a `model` body after field declarations.

```text
schema_block = "schema" ":" NEWLINE INDENT schema_stmt* DEDENT ;
```

Schema blocks attach metadata to fields declared in the same model. They must not add fields, remove fields, change field order, or change field types.

This RFC permits two schema block styles.

Per-field style:

```incan
model User:
    email_id: EmailAddress

    schema:
        email_id:
            alias = "email"
            description = "Primary email address"
            postgres.name = "email_address"
            proto.tag = 12
```

Grouped namespace style:

```incan
model User:
    email_id: EmailAddress
    created_at: DateTime

    schema:
        postgres:
            email_id.name = "email_address"
            created_at.name = "created_at"

        proto:
            email_id.tag = 12
            created_at.tag = 13
```

The two styles produce the same normalized metadata map. Implementations must reject ambiguous statements that cannot be normalized to a known field plus metadata key.

### Schema imports

A model body may include declarative schema imports:

```incan
model User:
    email_id: EmailAddress

    schema from "./user.schema.yaml"
    schema from AuditFieldMetadata
```

`schema from "./file"` imports metadata from a local compile-time source. It must not fetch network resources and must not execute user code. The source file is a build input and must participate in reproducible builds and incremental invalidation. Imported target-mapping values must preserve and resolve the same enum type, case identity, and typed payload structure as inline metadata. A plain string such as "varchar(200)" must not be interpreted as a typed mapping request. External decoding must not execute YAML object constructors or arbitrary user code.

`schema from OtherModel` imports metadata by matching canonical field names. It does not import fields, types, methods, defaults, derives, or inheritance behavior. It is metadata-only reuse.

A schema import must not introduce metadata for unknown fields unless the import is explicitly marked as partial-tolerant by a future RFC. Under this RFC, unknown imported fields are compile-time errors.

### Named schema overlays

A named schema overlay attaches metadata to an existing model without editing the model declaration:

```text
schema_overlay = "schema" IDENT "for" type_path ":" NEWLINE INDENT schema_overlay_item* DEDENT ;
schema_overlay_item = schema_import | schema_stmt ;
```

```incan
schema UserWarehouse for User:
    schema from "./user_warehouse.schema.yaml"

    postgres:
        id.name = "user_id"
        email.name = "email_address"
```

The target of a schema overlay must resolve to a model type. A schema overlay must not add fields, remove fields, reorder fields, retype fields, add methods, add derives, or change construction behavior. It may only attach schema metadata to fields that exist on the target model.

An overlay is a named schema metadata profile. It is not inheritance, not a model alias, and not a reopened model body. Ordinary construction, type checking, field access, and method resolution for the target model are unaffected.

An overlay must not declare or override compiler-semantic field metadata, including alias, `default_factory`, validate, and normalize, whether directly or through a schema import. Such entries must receive a compiler diagnostic identifying the offending key, field, overlay, and source declaration or import. The same restriction applies to explicit := overrides: precedence does not authorize changing the model contract. An overlay may supply adapter-owned names such as json.name or `postgres.name`; these names affect only a projection whose explicitly selected adapter documents and consumes them. They must not change canonical field names, ordinary constructor argument names, or the model's compiler-semantic alias. This restriction concerns overlays of an existing model; it does not remove compiler-semantic metadata from the owning model's declarations under RFC 085.

Multiple overlays may target the same model:

```incan
schema UserWarehouse for User:
    postgres:
        id.name = "user_id"

schema UserPublicApi for User:
    json:
        id.name = "id"
```

Adapters and reflection APIs should require an explicit overlay selection when overlay metadata is desired. A plain base descriptor should not silently include every overlay in scope.

### Merge and precedence

Base model metadata merges in this order, from lowest to highest precedence:

1. `schema from ...` imports in source order;
2. `schema:` blocks in source order;
3. inline field metadata from RFC 085.

Within schema blocks and schema imports, `key = value` is set-if-unset. If the key already exists with a different value, the compiler must report a conflict. `key := value` is an explicit override and must record that an override occurred in checked metadata.

Inline field metadata has highest precedence because it is closest to the field declaration. Duplicate inline keys are errors under RFC 085.

For RFC 087 models composed through a from header, inherited descriptive and namespaced field metadata forms a baseline before these owning-model merge steps. Metadata authored inline on a source field remains inherited metadata after composition; it must not acquire target-inline precedence. Structurally compatible source fields may contribute distinct keys or equal checked values. Contradictory values require an explicit := reconciliation in the owning model's schema block, retaining all incoming values and their origins. Owning-model inline precedence must not silently settle such a conflict. Retained validation obligations are compiler-semantic contract facts and must not be replaced or suppressed by projection metadata. Structural disagreements are errors before metadata reconciliation, and unresolved composition conflicts must be rejected before any named overlay is selected.

When an overlay is selected, overlay metadata is merged on top of the base model descriptor. Inside an overlay, imports and local schema statements merge in source order using the same `=` and `:=` rules. An overlay may explicitly override base metadata with `:=`, but `=` conflicts if the base descriptor already has a different value for the same field metadata key.

When multiple overlays are selected together, they merge in the order selected by the descriptor API. Conflicting `=` assignments across overlays are errors. Explicit `:=` overrides must record the overlay that performed the override.

These rules also apply to target-mapping requests. An explicit `:=` selects the effective request and records its provenance; it must not override the source type or suppress a later adapter incompatibility diagnostic. Two different mapping requests do not become compatible merely because precedence selects one.

Merge results must be deterministic for a fixed source tree, selected overlay list, and schema import set.

### Adapter interpretation

Adapters consume schema descriptors explicitly.

Baseline compilation must not interpret unknown namespaced metadata semantics. It must preserve explicit target-mapping requests and their provenance without treating a key ending in `type` as a source type declaration or rejecting it solely by that spelling. Schema blocks, imports, and overlays must not replace the checked source field type. An adapter consuming a target-mapping request must validate it before generating or using the projection and must reject incompatible, unsupported, or unestablished mappings with a clear diagnostic. Adapter-owned validation may reject additional namespace-specific keys when invoked.

Adapters must treat generated outputs as projections of checked metadata. They must not silently invent field existence, field order, source type, or default facts that contradict the model schema descriptor. Target-specific type choices must be derived from checked Incan type descriptors and adapter-owned mapping rules.

### Diagnostics

The compiler must report diagnostics for at least:

- schema block references an unknown field;
- schema import references an unknown field;
- schema overlay target is not a model type;
- schema overlay references an unknown field on the target model;
- schema overlay directly or indirectly declares or overrides compiler-semantic metadata such as alias, `default_factory`, validate, or normalize;
- schema merge conflict from `key = value`;
- invalid explicit override syntax;
- unsupported schema import source;
- external schema file cannot be read as a build input;
- external schema file contains unsafe metadata values;
- schema block statement cannot be normalized to field plus metadata key;
- schema metadata attempts to replace a checked Incan source field type or constraint;
- schema descriptor cannot represent a field type structurally.

Diagnostics should point at the schema statement or import that caused the error and should include the field name when applicable.

An adapter consuming a target-mapping request must diagnose incompatible, unsupported, or unestablished mappings before generating or using the projection. The diagnostic must identify the canonical field, its checked source type and relevant constraints, the requested target mapping, the adapter, the originating metadata statement or import when available, and the incompatibility or missing compatibility support. For the Customer example above, requesting `PostgresType.VarChar(length=200)` must report that the unconstrained source str has no declared 200-character bound and the requested target would narrow it. The adapter must not truncate values, silently change the source contract, or claim that an explicit metadata override resolved that incompatibility.

## Design details

### Why schema blocks exist

Inline metadata is readable only while it is sparse. Adapter ecosystems can require many keys per field. `schema:` blocks let the model body remain readable as a shape while still keeping metadata in checked source.

### Why schema models projection context

The model descriptor already carries the row shape. Adapter configuration can supply broad target-specific defaults. `schema` exists for the remaining layer: projection context that is specific to a model, overlay, field occurrence, or external compatibility contract. External names, stable wire tags, inclusion decisions, layout hints, generated-document grouping, and catalog terms are examples of facts that may differ per projection even when the checked model type is unchanged.

### Why named overlays exist

Many useful schema profiles are not owned by the module that defines the model. A warehouse adapter, partner API, catalog export, or ODCS contract may need to attach metadata to a stable model without editing that model. Named overlays make that expansion explicit and checked while preserving the model as the authority for fields and types.

### Why schema imports are declarative

External metadata sources are useful for generated or shared schema facts, but they must not turn compilation into arbitrary code execution. Schema imports are local, declarative build inputs.

### Why adapters are explicit

The compiler cannot own every ecosystem. It should provide a stable schema descriptor API. Libraries should own interpretation for their namespace. This lets the ecosystem grow without turning the compiler into a registry of SQL, Protobuf, JSON Schema, data-contract, and vendor-specific semantics.

### Why source types and target mappings have separate authority

The checked Incan model carries the source type descriptor for every field. An explicit target-mapping request can sit beside other projection metadata without becoming a competing source declaration: the owning adapter must interpret and validate it against that descriptor. Allowing a request such as `postgres.type` makes per-field projection customization readable while preserving one source of truth for Incan types and constraints. Compatibility checks and provenance are required; neither a metadata value nor an explicit merge override proves that a target representation is valid.

### Relationship to RFC 085

RFC 085 owns field metadata keys, safe metadata values, default factories, and the type-constraint boundary. This RFC owns metadata aggregation, descriptor exposure, schema block readability, named overlays, external schema sources, and adapter consumption.

### Relationship to RFC 048 and RFC 082

RFC 048 owns checked metadata extraction and stable metadata artifacts. RFC 082 owns documentation generation from checked metadata. This RFC defines the schema descriptor facts and aggregation rules those systems must preserve when model schema metadata is present.

For blast-radius and metadata-layer ambitions, RFC 086 descriptors are the bridge between checked model facts and downstream projections. A schema adapter should be able to declare or expose which descriptor and overlay it consumed, so later tooling can relate a model-field change to generated SQL, JSON Schema, Protobuf, ODCS, catalog, or documentation outputs.

### Compatibility / migration

This RFC is additive.

Projects can start with inline field metadata only. As metadata grows, they can migrate large adapter mappings into `schema:` blocks without changing the normalized schema descriptor.

When metadata belongs to a downstream projection rather than the model owner, projects can move that metadata into named overlays. Existing model construction and base descriptors remain unchanged unless a consumer explicitly selects the overlay.

## Alternatives considered

1. **Keep only inline field metadata**
   - Rejected because multi-adapter metadata turns model bodies into unreadable walls of brackets.

2. **Move all rich metadata to external files**
   - Rejected because high-signal row contract facts should stay visible beside the field they describe.

3. **Make the compiler understand every adapter namespace**
   - Rejected because adapter semantics should evolve independently from the language.

4. **Allow schema imports to add fields**
   - Rejected because the model body must remain the authority for field existence, type, and order.

5. **Store field types as strings in schema descriptors**
   - Rejected as the authoritative representation because strings are display artifacts, not checked semantic types.

6. **Require all schema expansion to happen inside the model body**
   - Rejected because many adapter and contract profiles are downstream concerns owned outside the model's source module.

7. **Treat target-mapping metadata as an independent type authority**
   - Rejected because a projection request must not override checked Incan types or constraints. Explicit namespaced requests are permitted only with owning-adapter compatibility checks; unsupported or conflicting requests require diagnostics.

## Drawbacks

Schema blocks add a second place to write field metadata. Deterministic precedence and conflict diagnostics are mandatory to avoid silent drift.

External schema imports add build-input and reproducibility concerns. The feature must stay declarative and local-only unless a future RFC defines stronger supply-chain rules.

The descriptor API becomes a stable contract consumed by adapters, docs, package tooling, and possibly registries. Schema evolution will require versioning discipline.

## Implementation architecture (non-normative)

A useful implementation shape is to normalize inline metadata from RFC 085, schema blocks, and schema imports into one checked base field metadata map per model field before downstream metadata extraction. Named overlays can then normalize into separate overlay metadata maps keyed by target model and overlay name. Each normalized entry should retain source location and merge provenance so diagnostics, generated docs, metadata-layer tools, and blast-radius reports can explain where a value came from.

Schema descriptors should be generated from checked model declarations after metadata merge. When overlays are selected, descriptor construction should merge the requested overlay maps over the base descriptor in a deterministic order. Adapters should consume descriptors rather than parsing source files or generated Rust.

Adapter outputs should be able to carry or reference the descriptor identity they were generated from: base model anchor, selected overlay anchors, descriptor schema version, adapter name/version where available, and source artifact identity. This does not make adapter outputs authoritative, but it gives blast-radius tooling a reliable edge from source contract to projection.

External schema import support can be staged after schema descriptors, schema blocks, and overlays, but the RFC-level contract treats them as one normalized metadata model.

## Layers affected

- **Parser / AST**: must support `schema:` blocks, `schema Name for Model:` overlays, `schema from ...`, and explicit override syntax in schema blocks and overlays.
- **Typechecker / Symbol resolution**: must validate schema block field references, schema overlay targets, schema overlay field references, schema import references, merge conflicts, safe metadata values from imports, and structural descriptor representability.
- **Checked metadata extractor**: must preserve normalized base schema metadata, overlay schema metadata, merge provenance, stable anchors, default factories, namespaced metadata, structured type descriptors, descriptor identity, and enough extraction provenance for metadata-layer and blast-radius consumers.
- **IR Lowering / Emission**: must make schema metadata available to reflection and artifacts without changing runtime behavior unless a consumer explicitly uses it.
- **Stdlib / Runtime**: must expose structured schema descriptor types and reflection functions without requiring row instances.
- **Formatter**: must format schema blocks, grouped namespace style, and explicit overrides deterministically.
- **LSP / Tooling**: should provide hover, completion, diagnostics, schema import awareness, and adapter-provided validation hooks where available.
- **Docs**: should render schema metadata from checked descriptors and avoid treating adapter projections as source truth.
- **Build / Packaging**: should track schema import files as build inputs and preserve schema descriptors in artifacts that claim checked metadata support, including overlay identity and descriptor/projection edges where available.

## Design Decisions

- Use `schema_of[Model]()` as the canonical user-facing descriptor API for base model descriptors.
- Use `schema_of[Model, Overlay]()` for descriptor requests with one overlay, and `schema_of[Model, OverlayA, OverlayB]()` for multiple overlays. Overlay order is semantically significant and defines merge order.
- Keep overlays strictly projection-only. Reject compiler-semantic keys, including alias, `default_factory`, and validate, in overlays and their imports, even with explicit overrides. Adapter-owned projection names remain permitted and must not alter the source model contract.
- Allow multiple overlays to be selected at once. Conflicts across selected overlays are diagnosed by the descriptor construction path according to the RFC merge rules.
- Support both per-field and grouped namespace schema block styles. They normalize to the same descriptor metadata map.
- Support local YAML schema import files first. JSON and an Incan-native metadata syntax can be added later without changing the core merge model.
- Resolve schema import paths relative to the source file that declares the `schema from` statement.
- Make `schema from OtherModel` match canonical field names only. Alias-based matching is excluded from this RFC because aliases are projection metadata, not source field identity.
- Register adapter-provided LSP validation through explicit tooling hooks supplied by adapter packages or build configuration. The compiler owns syntax and safe-value checks; adapters own namespace-specific validation.
- Represent descriptor/projection edges with base model anchor, selected overlay anchors in order, descriptor schema version, adapter identifier and version where available, source artifact identity where available, and field-level projection anchors.
- Permit explicit namespaced target-mapping requests in schema blocks, imports, overlays, and RFC 085 field metadata. Represent these requests as adapter-owned checked enum values with safe constant payloads, retaining their type, case, and payload structure; do not use target-type strings for dispatch. The owning adapter must validate compatibility with the checked source type, constraints, nullability, and representation contract before using the projection. Reject incompatible, unsupported, or unestablished mappings; no silent narrowing, truncation, coercion, fallback, or weakening of secret protection is permitted. Existing metadata conflict and explicit override rules still apply, and an override cannot waive compatibility checking.
