# RFC 096: Declaration metadata blocks

- **Status:** Draft
- **Created:** 2026-05-12
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 017 (validated newtypes with implicit coercion)
    - RFC 021 (model field metadata and schema-safe aliases)
    - RFC 048 (checked contract metadata, Incan emit, and interrogation tooling)
    - RFC 053 (formatter vertical spacing buckets)
    - RFC 082 (checked API documentation generation)
    - RFC 085 (field metadata and type-shaped constraints)
    - RFC 086 (schema descriptors and adapters)
    - RFC 091 (constrained integer newtype storage carriers)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC adds optional braced metadata blocks to declaration surfaces that already carry checked static metadata, starting with model fields and constrained primitive newtype underlyings. The goal is to preserve the compact `name as "wire": Type = default` field line while moving growing metadata and dense type options into nearby structured blocks that normalize to the same checked descriptors defined by RFC 021, RFC 085, RFC 086, and RFC 091.

## Core model

1. **The declaration line owns identity and type:** a model field line should remain readable as field name, optional wire alias, type, and default.
2. **Metadata is structured, not prose:** metadata blocks contain compile-time metadata entries that are exposed through checked descriptors, not comments or runtime objects.
3. **Blocks are an alternate spelling, not a second semantic layer:** a field metadata block normalizes to the same field metadata map as existing inline field metadata.
4. **Inline metadata remains valid for sparse cases:** short metadata may stay in brackets or a single-line metadata block when it does not harm readability.
5. **Large metadata moves close, not away:** declaration-local blocks keep metadata physically near the declaration without requiring a separate model-wide `schema:` section.
6. **Type options may use the same visual escape hatch:** constrained primitive options on newtype underlyings may be written in a block form when bracket syntax becomes too dense.
7. **Schema descriptors remain authoritative:** adapters consume normalized descriptors; they do not scrape source spelling or depend on whether metadata came from brackets, blocks, or schema declarations.

## Motivation

RFC 021 introduced inline field metadata, and RFC 085 extended the model with additional safe metadata and default factories. That works for sparse fields, but real contract-heavy code can quickly become a wall of punctuation. A field may carry a public/private marker, a source identifier, a wire alias, a long type expression, a default, a description, classification labels, adapter hints, and governance tags. Keeping all of that in one line makes the declaration hard to scan precisely when the model is most important.

RFC 086 addresses large adapter mappings with `schema:` blocks and overlays. That remains useful when metadata belongs to a projection or downstream profile, but it is not always the right answer for metadata owned by the model itself. A short field description, classification label, or local adapter hint often belongs beside the field declaration. Moving it into a separate `schema:` section can feel indirect, while keeping it inline can overload the field line.

The same pressure appears in constrained primitive syntax. RFC 017 and RFC 091 use compact bracket syntax such as `int[ge=1, le=12, storage=u8]`. That is acceptable for small examples, but it scales poorly as type-shaped options grow. The syntax needs a block form for cases where the type contract is still local but no longer readable as a bracket list.

## Goals

- Add declaration-local braced metadata blocks for model fields.
- Preserve existing inline field metadata syntax from RFC 021 and RFC 085.
- Normalize bracket metadata and braced metadata into the same checked field metadata descriptor.
- Keep field declarations readable when aliases, defaults, complex types, and multiple metadata entries appear together.
- Add a braced constrained primitive option form for validated newtype underlyings.
- Preserve RFC 086 schema blocks and overlays for model-wide, imported, and downstream projection metadata.
- Define formatter rules for single-line and multiline declaration metadata blocks.
- Define duplicate-key, precedence, and diagnostic behavior across inline metadata, metadata blocks, and schema metadata.

## Non-Goals

- This RFC does not remove existing `[key=value]` field metadata syntax.
- This RFC does not remove `schema:` blocks, named schema overlays, or schema imports from RFC 086.
- This RFC does not allow metadata to add, remove, reorder, or retype fields.
- This RFC does not make declaration metadata a runtime object literal.
- This RFC does not introduce arbitrary runtime expressions as metadata values.
- This RFC does not define every adapter namespace or validate every adapter-specific key.
- This RFC does not introduce field-local executable code blocks.
- This RFC does not change the semantic boundary that type constraints belong to types and field metadata belongs to fields.
- This RFC does not require immediate migration of existing code that uses inline metadata.

## Guide-level explanation

Simple field declarations remain compact. A field with only an alias, type, and default should not need any metadata block:

```incan
model Event:
    score as "Score": CustomerScore = 50
```

When the field owns structured metadata, attach a braced block directly to the field declaration:

```incan
model Event:
    score as "Score": CustomerScore = 50 {
        description = "Normalized customer score"
        classification = "restricted"
    }
```

Very small metadata may stay on one line when it remains readable:

```incan
model LogRecord:
    severity_text as "SeverityText": str { description = "OpenTelemetry severity text." }
```

Larger fields expand without changing the field declaration lane:

```incan
type SeverityNumber = newtype int {
    ge = 1
    le = 24
    storage = u8
}

model LogRecord with Serialize:
    timestamp as "Timestamp": Timestamp {
        description = "Time when the event occurred."
    }

    observed_timestamp as "ObservedTimestamp": Option[Timestamp] = None {
        description = "Time when telemetry observed the event."
    }

    trace_id as "TraceId": Option[TraceId] = None {
        description = "Request trace identifier when the event is span-correlated."
    }

    span_id as "SpanId": Option[SpanId] = None {
        description = "Span identifier when the event is span-correlated."
    }

    trace_flags as "TraceFlags": Option[TraceFlags] = None {
        description = "W3C trace flags for the correlated span."
    }

    severity_text as "SeverityText": str {
        description = "OpenTelemetry severity text, such as INFO or WARN."
    }

    severity_number as "SeverityNumber": SeverityNumber {
        description = "OpenTelemetry normalized severity number."
    }

    body as "Body": TelemetryValue {
        description = "Human or structured event body."
    }

    resource as "Resource": Resource {
        description = "Entity that produced this telemetry."
    }

    instrumentation_scope as "InstrumentationScope": InstrumentationScope {
        description = "Logical scope that emitted this record."
    }

    attributes as "Attributes": Attributes = Attributes({}) {
        description = "Additional structured attributes for this event."
    }

    event_name as "EventName": Option[str] = None {
        description = "Optional event class or type name."
    }
```

Recursive and nested types also stay readable because the metadata is no longer competing with the type expression:

```incan
model TelemetryValue with Serialize:
    kind as "Type": TelemetryValueKind {
        description = "Telemetry value kind: none, string, bool, int, float, bytes, array, or map."
    }

    string_value as "StringValue": Option[str] {
        description = "String value when kind is string."
    }

    bool_value as "BoolValue": Option[bool] {
        description = "Boolean value when kind is bool."
    }

    int_value as "IntValue": Option[int] {
        description = "Integer value when kind is int."
    }

    float_value as "FloatValue": Option[float] {
        description = "Floating-point value when kind is float."
    }

    bytes_value as "BytesValue": Option[str] {
        description = "Encoded byte value when kind is bytes."
    }

    array_value as "ArrayValue": list[TelemetryValue] {
        description = "Nested array values when kind is array."
    }

    map_value as "MapValue": Dict[str, TelemetryValue] {
        description = "Nested map values when kind is map."
    }
```

Adapter and governance metadata may live in a field-local block when it is owned by the model itself:

```incan
model CustomerEvent:
    customer_id as "CustomerId": CustomerId {
        description = "Stable customer identifier."
        classification = "restricted"
        catalog.term = "customer.identity"
        json_schema.examples = ["cus_123"]
    }
```

When metadata belongs to a downstream projection or reusable profile, users should still use RFC 086 schema blocks or named overlays:

```incan
model CustomerEvent:
    customer_id as "CustomerId": CustomerId {
        description = "Stable customer identifier."
        classification = "restricted"
    }

schema CustomerWarehouse for CustomerEvent:
    postgres:
        customer_id.name = "customer_id"
        customer_id.index = "events_customer_id_idx"
```

## Reference-level explanation

### Field metadata blocks

A model field may be followed by a declaration metadata block:

```text
field_decl = visibility? IDENT field_meta? alias_sugar? ":" type_expr default? decl_metadata_block? ;
decl_metadata_block = "{" metadata_entry* "}" ;
metadata_entry = metadata_key metadata_assign metadata_value ;
metadata_assign = "=" ;
```

The metadata block must attach to the field declaration immediately before it. It is part of the field declaration, not a nested executable block. The parser must not allow statements, function definitions, control flow, or local variable declarations inside a field metadata block.

The field metadata block must normalize to the same field metadata map defined by RFC 021 and RFC 085. Tooling and checked metadata consumers must not need to know whether a key was written as `[description="..."]` or in a braced block.

The following declarations are semantically equivalent:

```incan
model Account:
    type_ [description="Account tier"] as "type": str
```

```incan
model Account:
    type_ as "type": str {
        description = "Account tier"
    }
```

Inline bracket metadata and braced metadata may appear on the same field only when their keys are disjoint:

```incan
model Account:
    type_ [classification="restricted"] as "type": str {
        description = "Account tier"
    }
```

A field must not define the same metadata key more than once across bracket metadata, alias sugar, and its braced metadata block:

```incan
model Invalid:
    type_ [description="Account tier"] as "type": str {
        description = "Customer segment"
    }
```

The compiler must report duplicate declaration-local metadata keys at the later source location and must include the field name and the earlier key location in the diagnostic.

Alias sugar remains the preferred spelling for wire names. A braced metadata block may contain `alias = "wire"` for migration and generated-source compatibility, but it must conflict with any different alias provided by `as "wire"` or `[alias="wire"]` on the same field.

### Metadata values

Metadata block values must use the safe metadata value set defined by RFC 085 and RFC 086. Values must be compile-time metadata values, not runtime expressions. A metadata block must not evaluate user code.

Namespaced metadata keys are written as dotted paths:

```incan
model Event:
    created_at as "CreatedAt": DateTime {
        description = "Time when the event occurred."
        postgres.name = "created_at"
        postgres.index = "events_created_at_idx"
        json_schema.format = "date-time"
    }
```

The compiler must preserve each metadata entry's source location and provenance in checked metadata where the surrounding descriptor already records provenance.

### Merge and precedence

Declaration-local field metadata includes bracket metadata, alias sugar, and braced field metadata blocks. Declaration-local metadata has the same precedence over RFC 086 schema imports, schema blocks, and named overlays that inline field metadata already has.

Within declaration-local metadata, duplicate keys are errors rather than ordered overrides. This keeps the field declaration honest and avoids requiring local readers to understand merge order inside one declaration.

Schema blocks and named overlays keep the `=` and `:=` merge behavior defined by RFC 086. Field metadata blocks introduced by this RFC use only `=` because they are declaration-local and do not need explicit override syntax.

### Formatter behavior

The formatter should preserve a single-line metadata block only when the field declaration plus block has one metadata entry and fits within the formatter's normal line budget:

```incan
severity_text as "SeverityText": str { description = "OpenTelemetry severity text." }
```

The formatter must expand metadata blocks with multiple entries, long values, comments, or nested safe metadata values:

```incan
customer_id as "CustomerId": CustomerId {
    description = "Stable customer identifier."
    classification = "restricted"
}
```

The formatter should prefer alias sugar over `alias = "..."` in declaration metadata blocks when formatting source it is allowed to rewrite semantically. It must not rewrite generated source, macro output, or user source when preserving exact spelling is required by a tool mode.

### Constrained primitive option blocks

A constrained primitive underlying in a validated newtype may use a braced option block instead of bracket syntax:

```incan
type CustomerScore = newtype int {
    gt = 0
    lt = 100
    storage = i8
}
```

This is semantically equivalent to:

```incan
type CustomerScore = newtype int[gt=0, lt=100, storage=i8]
```

The option block is part of the constrained primitive type expression. It is not a newtype body and it does not allow methods, docstrings, or runtime statements. A newtype that needs a body must still use the existing body-bearing declaration form:

```incan
type CustomerScore = newtype int {
    gt = 0
    lt = 100
    storage = i8
}:
    def display(self) -> str:
        return f"{self.0}%"
```

Only primitive constraint and storage options accepted by RFC 017 and RFC 091 are valid in this block form. Duplicate option keys must be compile-time errors. The compiler must normalize bracket syntax and block syntax into the same constrained primitive descriptor.

## Design details

### Why use `=` inside metadata blocks

This RFC uses `key = value` inside metadata blocks rather than `key: value` to avoid making declaration metadata look like a general object literal. Existing inline metadata already uses `key=value`, and RFC 086 schema statements already use assignment forms for metadata. Keeping assignment syntax reinforces that metadata blocks are static descriptor assignments, not runtime dictionaries.

### Why keep alias on the field line

Wire identity is high-signal enough to belong in the field declaration lane. The preferred form is:

```incan
score as "Score": CustomerScore = 50
```

not:

```incan
score: CustomerScore = 50 {
    alias = "Score"
}
```

The block form still accepts `alias` for compatibility, but style guidance and formatter fixups should prefer `as "WireName"` for ordinary source.

### Relationship to schema blocks

Declaration metadata blocks and RFC 086 `schema:` blocks solve different locality problems. Declaration blocks are for metadata owned by the field's declaration. Schema blocks and overlays are for model-wide mappings, imported metadata, downstream projection profiles, and metadata that should be selectable without editing the base model.

The same normalized field descriptor may include metadata from both sources. Provenance must make that visible to diagnostics and tooling.

### Relationship to type constraints

Field metadata blocks must not make field metadata keys such as `gt`, `lt`, `pattern`, or `storage` affect the field's type. Type-shaped constraints remain part of the type expression. If a user wants a constrained integer field, they should declare or reuse a constrained newtype:

```incan
type RetryAttempts = newtype int {
    ge = 0
    le = 10
    storage = u8
}

model RetryPolicy:
    attempts as "maxRetries": RetryAttempts {
        description = "Maximum retry attempts."
    }
```

This keeps value-domain semantics reusable and prevents two fields with the same apparent primitive type from having different hidden validation rules.

### Comments inside blocks

Comments may appear inside declaration metadata blocks and constrained primitive option blocks. Comments have no semantic effect and must not appear in checked metadata descriptors.

```incan
field as "Field": str {
    # Catalog classification used by governance adapters.
    classification = "restricted"
}
```

### Diagnostics

Diagnostics for invalid metadata blocks should point at the offending key or value. Diagnostics should include the declaration kind and name, such as `field customer_id`, `newtype CustomerScore`, or `field severity_number`.

The compiler should provide targeted messages for these cases:

- duplicate metadata keys across bracket and block metadata;
- alias conflicts across `as`, `[alias=...]`, and `alias = ...`;
- runtime expressions in metadata values;
- unsupported constrained primitive options;
- field metadata keys that look like type constraints when written in a field metadata block;
- declarations with an opening `{` metadata block and no closing `}`.

## Alternatives considered

1. **Keep only inline brackets** — Rejected because real model fields with aliases, descriptions, classifications, adapter keys, complex types, and defaults become unreadable.
2. **Move all rich metadata into `schema:` blocks** — Rejected as the only local answer because some metadata is declaration-owned and should remain beside the field.
3. **Use hanging indentation without braces** — Rejected because it makes alignment carry semantic weight and introduces a field continuation form without an explicit block opener.
4. **Use decorators before fields** — Rejected because decorators create vertical noise, split the field's metadata from its type, and make repeated field metadata look like executable modifiers.
5. **Use `key: value` inside metadata blocks** — Rejected for this draft because it visually collides with object literals and with the `name: Type` field annotation pattern.
6. **Make constrained primitive blocks the only spelling** — Rejected because bracket syntax remains useful for short constraints and already exists in implemented and drafted RFCs.
7. **Allow metadata blocks on every declaration immediately** — Rejected for this RFC because the semantics should be proven first on fields and constrained newtype underlyings before expanding the surface.

## Drawbacks

This RFC adds braces to a Python-shaped language surface. That is a real cost. The benefit is a readable local escape hatch for structured metadata and type options, but the language must keep the brace use narrow and formatter-backed or it risks drifting toward mixed visual grammars.

The RFC also introduces another spelling for field metadata. Normalization keeps semantics unified, but readers and tools must still understand both the bracket and block forms. Formatter guidance and style documentation are necessary to prevent gratuitous churn between spellings.

Declaration metadata blocks can make models taller. Single-entry blocks should remain inline when readable, and model-wide or projection-owned metadata should still move to RFC 086 schema blocks or overlays.

## Implementation architecture

The implementation should normalize all declaration-local field metadata into the existing field metadata descriptor before schema descriptor extraction. The parser should retain source provenance for each metadata entry so duplicate-key diagnostics and checked metadata tools can explain whether a value came from brackets, alias sugar, or a block.

The constrained primitive block form should normalize into the same constrained primitive representation as bracket syntax. Later compiler stages should not distinguish `int[ge=0, le=10]` from `int { ge = 0; le = 10 }` except for source spans and formatting.

## Layers affected

- **Parser / AST**: must parse braced metadata blocks after model field declarations and braced option blocks after constrained primitive underlyings in validated newtype declarations.
- **Typechecker / Symbol resolution**: must validate metadata keys and values according to RFC 085, detect duplicate declaration-local keys, detect alias conflicts, and preserve type-constraint semantics on the type side rather than the field metadata side.
- **IR Lowering / Emission**: should consume normalized metadata and constrained primitive descriptors without depending on source spelling.
- **Checked metadata extraction**: must expose normalized field metadata and constrained primitive descriptors with source provenance where supported.
- **Formatter**: must format single-line and multiline metadata blocks deterministically, preserve readable spacing, and avoid turning short fields into unnecessary vertical blocks.
- **LSP / Tooling**: should offer hover, completion, diagnostics, and quick fixes for metadata keys in braced blocks, including migration suggestions from dense inline metadata to block form.
- **Documentation generation**: should render field metadata identically regardless of whether it was written in brackets or a declaration metadata block.

## Unresolved questions

- Should `alias = "WireName"` inside a declaration metadata block remain accepted long term, or should alias be allowed only through `as "WireName"` and legacy brackets?
- Should declaration metadata blocks eventually be allowed on enum variants, function parameters, properties, type aliases, and trait requirements?
- Should the constrained primitive block form use raw `int { ... }` as drafted here, or should it require a disambiguating marker such as `int where { ... }`?
- Should the formatter automatically migrate dense bracket metadata to braced metadata blocks, or should that remain an explicit lint/fix mode?
- Should metadata block keys use the same namespace validation policy as RFC 086 schema keys, or should declaration-local metadata remain limited to RFC 085 keys plus explicitly enabled namespaces?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
