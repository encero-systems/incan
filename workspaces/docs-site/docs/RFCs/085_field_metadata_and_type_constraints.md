# RFC 085: Field metadata and type-shaped constraints

- **Status:** Planned
- **Created:** 2026-04-29
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 017 (validated newtypes with implicit coercion)
    - RFC 021 (model field metadata and schema-safe aliases)
    - RFC 048 (checked contract metadata, Incan emit, and interrogation tooling)
    - RFC 082 (checked API documentation generation)
- **Issue:** #472
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC strengthens Incan model fields as readable row-contract declarations. It expands RFC 021 field metadata beyond `alias` and `description`, adds `default_factory` as a field construction feature, defines a standard set of high-signal field contract keys, extends metadata values beyond scalar literals, and draws a hard boundary between field metadata and type-shaped validation. Field metadata describes the field as a field; type-specific constraints belong in constrained types, validated newtypes, or type expressions. The result is a model body that remains readable while carrying enough checked metadata for documentation, data contracts, reflection, downstream schema adapters, metadata-layer extraction, and blast-radius analysis.

## Core model

1. **The model field is the row-contract unit:** field name, type, nullability, defaulting, aliasing, description, and high-signal field facts are checked source facts.
2. **Readability is part of the contract:** inline metadata should be limited to facts a reader wants while scanning the data shape.
3. **Metadata is LHS-owned:** field configuration belongs next to the field name, before the type. The RHS remains the direct default-value lane.
4. **Defaulting has one source:** a field may use either an RHS default value or `default_factory`, never both.
5. **Constraints are type-shaped:** numeric bounds, string patterns, length constraints, item constraints, enum membership, and semantic validation belong in the type lane when the type system can express them.
6. **Metadata has lanes:** compiler-semantic keys affect language behavior, standard descriptive keys are checked and preserved for tools, and namespaced keys are inert metadata for explicit consumers.
7. **Evidence is not inline truth:** confidence, evidence trails, timestamps, review state, and authority records should attach to stable field anchors in metadata artifacts, not clutter handwritten model declarations.
8. **Extraction is a first-class consumer:** the checked metadata extractor must preserve field facts in a stable, machine-readable form suitable for docs, adapters, artifact inspection, dependency indexing, compatibility checks, and blast-radius analysis.

## Motivation

RFC 021 intentionally created a field metadata slot but standardized only `alias` and `description`. That was enough for schema-safe names and basic docs, but not enough for model-driven contracts, schema projection, default factories, classification, primary keys, examples, and field tags.

At the same time, copying a Pydantic-style `Field(...)` helper wholesale would make field semantics less readable in Incan. Incan already has a dedicated LHS field metadata position. Putting field configuration there keeps the declaration shape stable:

```incan
field_name [metadata...]: Type = default
```

The model object sits at the center of checked metadata, docs, adapters, compatibility analysis, and blast-radius workflows. That does not mean every operational fact belongs inline. It means the model must carry stable row-contract facts clearly, while downstream metadata systems attach evidence, confidence, lineage, and review state to stable anchors.

## Goals

- Extend RFC 021 field metadata while preserving the existing syntax.
- Define standard compiler-known field metadata keys beyond `alias` and `description`.
- Add `default_factory` for fresh per-instance defaults.
- Make `default_factory` mutually exclusive with RHS defaults.
- Support richer safe metadata values such as lists and dictionaries where appropriate.
- Support namespaced metadata keys as inert preserved metadata.
- Keep type-specific validation in type syntax, constrained types, or validated newtypes.
- Preserve field metadata in reflection, checked API metadata, generated docs, and schema descriptor consumers.
- Preserve field metadata with stable anchors and safe serialized values so metadata-layer tools can diff, index, and attach downstream evidence without source scraping.
- Keep field declarations readable enough to remain the primary source people review.

## Non-Goals

- Defining schema blocks, schema imports, schema descriptor APIs, or adapter interpretation. Those belong to RFC 086.
- Encoding operational SLAs, teams, support channels, infrastructure, pricing, runtime evidence, or promotion approvals in field metadata.
- Replacing RFC 017 constrained types or validated newtypes.
- Making namespaced metadata affect baseline compilation behavior.
- Validating every adapter-specific metadata key in the compiler.
- Adding fields, removing fields, changing field order, or changing field types through metadata.
- Introducing a `Field(...)` RHS helper.
- Making field metadata an unrestricted object language.

## Guide-level explanation

Use inline metadata for high-signal facts:

```incan
model Customer:
    id [primary_key=true, description="Stable customer identifier"]: CustomerId
    email [alias="email_address", description="Primary customer email", classification="restricted"]: EmailAddress
    tags [default_factory=list[str], description="Search and segmentation tags"]: list[str]
```

Direct defaults stay on the RHS:

```incan
model RetryPolicy:
    attempts: int = 3
    labels [default_factory=dict[str, str]]: dict[str, str]
```

`default_factory` and an RHS default cannot both appear:

```incan
model Invalid:
    labels [default_factory=dict[str, str]]: dict[str, str] = {}  # rejected
```

Type-specific validation stays in the type:

```incan
type PositiveCents = newtype int[gt=0]
type CurrencyCode = newtype str[pattern="^[A-Z]{3}$"]

model SalesEvent:
    amount: PositiveCents
    currency: CurrencyCode
```

Namespaced metadata may be written inline when it is still readable:

```incan
model Event:
    created_at [postgres.name="created_at", spark.partition=true]: DateTime
```

For large adapter mappings, use the schema block surface from RFC 086 rather than turning every field into a wall of brackets.

## Reference-level explanation

### Field metadata syntax

RFC 021 field metadata remains valid:

```text
field_decl = IDENT field_meta? alias_sugar? ":" type_expr default? ;
field_meta = "[" field_meta_args? "]" ;
field_meta_args = field_meta_arg { "," field_meta_arg } ;
field_meta_arg = metadata_key "=" metadata_value ;
metadata_key = IDENT ("." IDENT)* ;
```

This RFC extends `metadata_key` to allow dotted namespaced keys. Unqualified keys are reserved for compiler-known or language-standard metadata. Dotted keys are namespaced metadata.

The `as "wire"` alias sugar from RFC 021 remains valid and continues to be equivalent to `alias="wire"` where RFC 021 says it is allowed.

### Safe metadata values

Metadata values must be safe metadata values. A safe metadata value must be representable without executing user code. This RFC requires support for:

- string literals;
- integer literals;
- float literals;
- boolean literals;
- enum variant paths;
- lists of safe metadata values;
- dictionaries with string keys and safe metadata values;
- callable symbol paths only for keys that explicitly accept callables, such as `default_factory`.

Implementations must reject metadata expressions that require function calls, constructors, closures, comprehensions, mutation, I/O, async evaluation, or runtime module initialization.

### Metadata lanes

Compiler-semantic keys are interpreted by the language and can affect construction, name resolution, serialization defaults, reflection, or diagnostics. This RFC defines:

- `alias: str`
- `default_factory: Callable[(), T]`

Standard descriptive keys are checked and preserved by the compiler but do not change ordinary execution by themselves. This RFC defines:

- `description: str`
- `title: str`
- `examples: list[T]` where each example is a safe metadata value assignable to the field type, or a string example when type-checked examples are not available;
- `deprecated: bool | str`
- `classification: str`
- `tags: list[str]`
- `primary_key: bool`
- `primary_key_position: int`
- `business_name: str`
- `read_only: bool`
- `write_only: bool`

Namespaced keys are inert metadata preserved for explicit consumers. A namespaced key has at least one dot, such as `postgres.name`, `spark.partition`, `proto.tag`, `json.name`, or `vendor.rule`. Baseline compilation must validate syntax and safe value shape, store the value, and expose it through checked metadata. Baseline compilation must not interpret ecosystem-specific semantics for unknown namespaces.

### Defaults and default factories

A field may define at most one defaulting mechanism.

An RHS default value defines a direct default:

```incan
count: int = 0
```

`default_factory` defines a zero-argument callable used to produce a default when the field is omitted:

```incan
tags [default_factory=list[str]]: list[str]
```

The `default_factory` callable must be a statically resolvable callable symbol or constructor surface. It must take no arguments and return a value assignable to the field type.

It is a compile-time error to specify both `default_factory` and an RHS default value on the same field.

### Type-shaped constraints

Type-specific constraints should be expressed in type syntax, constrained primitives, validated newtypes, or named semantic types:

```incan
age: int[ge=0]
email: EmailAddress
amount: PositiveCents
```

Field metadata must not become the primary location for numeric bounds, string patterns, length constraints, item constraints, or enum membership when the type system can express those constraints.

Adapters may project type constraints into external schemas. For example, a JSON Schema adapter may map `str[pattern="..."]` into a JSON Schema `pattern`. That is an adapter projection of checked type facts, not a separate field metadata truth.

Field metadata must not define target-system, storage, wire, or adapter-specific field types. An adapter must map from the checked Incan type descriptor to its own type system. For example, a database adapter maps `UserId` or `str[pattern="..."]` to the appropriate database type through adapter-owned rules, not through `postgres.type` metadata on the field.

### Metadata preservation

Checked metadata extraction must preserve all compiler-semantic keys, standard descriptive keys, and namespaced keys. Generated docs should render standard descriptive keys when useful. Namespaced keys may be displayed in advanced views or consumed by adapters, but they should not dominate the ordinary field documentation view.

Reflection must expose enough metadata for schema adapters and documentation tooling to recover the checked field contract without parsing source text.

### Extraction and blast-radius metadata

Field metadata introduced by this RFC must flow through the same checked metadata extraction family as RFC 048 public API and model metadata. The extracted representation must be stable enough for downstream metadata-layer tools to answer questions such as:

- which public models contain fields classified as restricted;
- which fields participate in primary-key contracts;
- which aliases, default factories, and type constraints changed between two artifacts;
- which schema adapters or external projections depend on a field anchor;
- which downstream evidence or review records attach to the affected model and field anchors.

The extraction contract must preserve at least:

- package, module, model, and field identity;
- stable model and field anchors where available;
- source location and declaration visibility where available;
- canonical field name and alias metadata;
- field type as a checked type reference or structured descriptor hook;
- default/default-factory facts;
- metadata lane for each key;
- safe serialized metadata value;
- enough provenance to distinguish inline field metadata from metadata merged later by RFC 086.

Blast-radius and metadata-layer tools may enrich these anchors with evidence, confidence, ownership, lineage, freshness, review status, and compatibility decisions. Those enrichments are not field metadata under this RFC unless another RFC explicitly promotes a key into the checked field metadata surface.

### Diagnostics

The compiler must report diagnostics for at least:

- duplicate inline metadata keys;
- unknown unqualified compiler-semantic keys;
- unsafe metadata values;
- invalid metadata value type for a compiler-known key;
- both RHS default and `default_factory` on the same field;
- `default_factory` target is not a zero-argument callable returning the field type;
- invalid namespaced key syntax;
- namespaced metadata appears where the grammar does not allow metadata;
- field metadata attempts to define target-system, storage, wire, or adapter type mappings;
- field metadata attempts to express a type-specific constraint that must be written in type syntax, when the compiler can identify that misuse.

Diagnostics should point at the metadata key that caused the error and should include the field name when applicable.

## Design details

### Why no `Field(...)` helper

Python uses `dataclasses.field(...)` and Pydantic uses `Field(...)` because ordinary assignment syntax cannot represent all field configuration cleanly. Incan already has an LHS field metadata position. Reusing that position keeps field configuration attached to the field declaration rather than hiding field semantics in a runtime-looking RHS helper.

This RFC therefore prefers:

```incan
tags [default_factory=list[str], description="Labels"]: list[str]
```

over:

```incan
tags: list[str] = Field(default_factory=list[str], description="Labels")
```

The RHS remains the direct default-value lane.

### Why namespaced metadata is inert

The compiler cannot own every ecosystem. It should provide a stable metadata carrier. Libraries should own interpretation for their namespace. This lets the ecosystem grow without turning the compiler into a registry of SQL, Protobuf, JSON Schema, data-contract, and vendor-specific semantics.

### Why constraints stay in types

Constraints such as `gt=0`, `pattern="..."`, and `max_length=320` describe which values inhabit a type. Putting them in field metadata makes the same value type mean different things depending on where it appears. Named constrained types and validated newtypes let those semantics be reused, imported, documented, and checked consistently.

### Why evidence stays outside fields

Evidence records need source, confidence, timestamp, authority, review status, and supporting facts. Those are essential for governance and blast-radius workflows, but they are not the field's row schema. A handwritten model should carry accepted contract facts. Assessment tools and promotion systems may attach evidence to the same field anchors in separate metadata artifacts.

### Relationship to RFC 021

RFC 021 remains the foundation. `alias`, `description`, `as` sugar, alias-aware constructor keys, alias-aware member access, and reflection behavior continue to apply. This RFC expands the metadata key space and safe value model.

### Relationship to RFC 017

RFC 017 owns validated newtypes and type-level validation. This RFC does not move type constraints into field metadata. It clarifies that field metadata and type constraints are separate lanes of the model contract.

### Relationship to RFC 048 and RFC 082

RFC 048 owns checked metadata extraction and stable metadata artifacts. RFC 082 owns documentation generation from checked metadata. This RFC defines additional field metadata facts that those systems must be able to preserve and project.

For metadata-layer and blast-radius ambitions, the important contract is that these field facts are extracted as checked facts with stable anchors, not rediscovered by parsing source text or generated Rust. Downstream systems can attach mutable governance and evidence records to those anchors while the source model remains the readable row-contract declaration.

### Compatibility / migration

This RFC is additive. Existing RFC 021 metadata remains valid.

Code using only `alias` and `description` can continue unchanged.

## Alternatives considered

1. **Keep only `alias` and `description`**
   - Rejected because model fields need default factories, classifications, tags, examples, primary-key hints, and namespaced metadata for schema consumers.

2. **Use a `Field(...)` RHS helper**
   - Rejected because Incan already has LHS field metadata, and RHS helpers blur the distinction between direct defaults and field configuration.

3. **Put constraints in field metadata**
   - Rejected as the primary model because reusable constrained types and validated newtypes are clearer, more composable, and less location-dependent.

4. **Make namespaced metadata compiler-semantic**
   - Rejected because adapter semantics should evolve independently from the language.

5. **Put governance evidence inline**
   - Rejected because confidence, evidence, authority, timestamps, and review state are operational metadata about how facts were derived or approved. They should attach to stable anchors in metadata artifacts, not clutter source models.

## Drawbacks

This RFC expands the field metadata language and therefore increases parser, formatter, metadata extraction, reflection, and tooling complexity.

The lane model requires clear docs. Users need to understand why `classification` can be inline, adapter-specific names can be stored but not interpreted by the compiler, target type mappings belong to adapters, and runtime evidence belongs elsewhere.

Supporting richer safe metadata values increases the burden on serialization and checked metadata stability.

## Implementation architecture (non-normative)

A useful implementation shape is to normalize inline metadata into a checked field metadata map per model field. Each normalized entry should retain source location so diagnostics, generated docs, metadata-layer tools, and blast-radius reports can explain where a value came from.

Checked metadata extraction should preserve the lane of each key: compiler-semantic, standard descriptive, or namespaced.

Artifact metadata should preserve field metadata losslessly for packages that claim RFC 048 metadata support. Tools should report missing metadata support rather than reconstructing field metadata from emitted code or rendered docs.

## Layers affected

- **Parser / AST**: must support dotted metadata keys and richer safe metadata values.
- **Typechecker / Symbol resolution**: must validate compiler-known metadata keys, default factories, safe metadata values, and type-shaped constraint boundaries.
- **Checked metadata extractor**: must preserve field metadata lanes, stable field anchors, default factories, namespaced metadata, safe value representations, and enough provenance for metadata-layer and blast-radius consumers.
- **IR Lowering / Emission**: must preserve construction semantics for defaults and default factories while making field metadata available to reflection and artifacts.
- **Stdlib / Runtime**: must expose field metadata consistently through reflection.
- **Formatter**: must format inline metadata and multiline metadata lists deterministically.
- **LSP / Tooling**: should provide hover, completion, diagnostics, and go-to-definition behavior for standard field metadata.
- **Docs**: should render high-signal field metadata from checked metadata without letting adapter-specific keys dominate ordinary docs.

## Design Decisions

- Stabilize the standard descriptive key set in this RFC as `description`, `title`, `examples`, `deprecated`, `classification`, `tags`, `primary_key`, `primary_key_position`, `business_name`, `read_only`, and `write_only`.
- Keep `classification` and `tags` as unqualified standard descriptive keys. They are intentionally lightweight checked facts, not a full governance policy model.
- Include lists and dictionaries in the first safe metadata value set. Descriptor serialization must support them rather than deferring them.
- Allow `examples` to contain safe metadata values assignable to the field type, with string examples allowed when typed examples are not available.
- Keep `primary_key` and `primary_key_position` as field metadata. Composite key order is represented by field-level positions; richer model-level identity constraints can be defined by a future RFC if needed.
- Treat stable field anchors as source-field and model-field metadata identities, not as external alias identities. A canonical field rename is a new source identity unless a future migration or rename-marker feature explicitly preserves continuity. External aliases and adapter mappings do not by themselves preserve the Incan field anchor.
