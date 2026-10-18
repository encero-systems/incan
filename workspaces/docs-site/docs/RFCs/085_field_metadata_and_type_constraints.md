# RFC 085: Field metadata and type-shaped constraints

- **Status:** Planned
- **Created:** 2026-04-29
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 017 (validated newtypes with implicit coercion)
    - RFC 021 (model field metadata and schema-safe aliases)
    - RFC 033 (dependency-resolved configuration defaults)
    - RFC 048 (checked contract metadata, Incan emit, and interrogation tooling)
    - RFC 082 (checked API documentation generation)
    - RFC 086 (schema projections and adapter interpretation)
    - RFC 087 (reusable field contracts and named validation checks)
    - RFC 103 (secret values and non-disclosure)
    - RFC 122 (canonical model construction and fieldmethods)
    - RFC 128 (checked callable signatures)
    - RFC 131 (typed errors and validation umbrellas)
    - RFC 132 (frozen classes)
    - RFC 137 (named enum payload fields)
    - RFC 142 (shared failure-policy vocabulary)
    - RFC 143 (cloning with field overrides)
- **Issue:** [#472](https://github.com/encero-systems/incan/issues/472)
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC strengthens Incan model fields as readable row-contract declarations. It expands RFC 021 field metadata beyond `alias` and `description`, adds `default_factory` and checked validate callable metadata as field construction features, defines a standard set of high-signal field contract keys, extends metadata values beyond scalar literals, and draws a hard boundary between field metadata and type-shaped validation. Field metadata describes the field as a field; type-specific constraints belong in constrained types, validated newtypes, or type expressions. The result is a model body that remains readable while carrying enough checked metadata for documentation, data contracts, reflection, downstream schema adapters, metadata-layer extraction, and blast-radius analysis.

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
- Attach custom field checks with `validate=check` metadata while retaining type-shaped constraints and typed error diagnostics.
- Allow fallible default factories with original failures retained under a ValidationError umbrella and a statically visible Result-returning construction contract.
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

Default factories may also fail. The imports below are existing stdlib APIs, `default_endpoint` is application-defined, and fallible factory integration is proposed behavior:

```incan
from std.environ import get, EnvironError

def default_endpoint() -> Result[str, EnvironError]:
    return get("SERVICE_ENDPOINT")

model ServiceConfig:
    endpoint [default_factory=default_endpoint]: str
```

Canonical construction of ServiceConfig returns `Result[ServiceConfig, ValidationError]`. A missing environment value is captured under RFC 131's ValidationError umbrella, retaining the original EnvironError and its diagnostics. Supplying `endpoint` explicitly skips this factory, but the constructor keeps the same declared Result type. An unsuccessful factory does not expose a partially initialized model or turn its error into text. Factory metadata stores a checked callable reference; its body runs during construction when needed, never during metadata extraction.

Factory failure handling is configurable. Ordinary construction defaults to FailFast: after a needed factory returns Err, later factories are not invoked. A caller may explicitly select CollectAll to attempt the remaining eligible default factories and retain their failures together under ValidationError. Both policies preserve original typed failures and have the same declared constructor result type. CollectAll is an explicit choice to execute additional factory bodies, including their ordinary permitted effects; it does not retry failed factories or roll back effects. A check requiring a field whose factory failed remains blocked, and no partially initialized model is returned. The toggle concerns factory evaluation; it does not replace a decoder's complete input-validation contract or a streaming consumer's independent failure policy.

Factories can also declare typed dependencies on other fields, following the same resolved-default principle as RFC 033 ctx declarations. The following application-defined factory and model illustrate proposed behavior:

```incan
def default_dsn(host: str, port: int) -> str:
    return f"postgresql://{host}:{port}/app"

model DatabaseConfig:
    host: str = "localhost"
    port: int = 5432
    dsn [default_factory=default_dsn]: str

def example() -> None:
    config = DatabaseConfig(host="db.internal", port=55432)
    assert config.dsn == "postgresql://db.internal:55432/app"
```

The declared host and port parameters identify canonical stored fields and receive their effective normalized values. Supplying dsn explicitly skips this factory. Dependencies may refer to later-declared fields; resolution follows the checked dependencies rather than assuming every default is ready in source order. Cycles and conflicts with the model's authored construction-hook order require clear diagnostics. Dependency failure blocks the factory instead of calling it with unavailable values. This is ordinary per-instance model construction; ctx retains its separate shared lazy resolution and publication contract.

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
- enum variant paths and structural enum-case values with recursively safe, type-checked payloads, including RFC 137 named payloads;
- lists of safe metadata values;
- dictionaries with string keys and safe metadata values;
- callable symbol paths only for keys that explicitly accept callables, such as `default_factory` and validate, and checked fieldmethod identity paths within normalize lists under RFC 122.

Structural enum-case values are a narrow exception to call-shaped syntax: the callee must resolve to an enum case, and the compiler must check its payload arity, names, and types without executing user code. A function or constructor with a similar spelling must not qualify. Checked metadata must preserve the enum type identity, case identity, and payload structure rather than flattening the value to its display text.

Implementations must reject metadata expressions that require arbitrary function calls, model or class construction, closures, comprehensions, mutation, I/O, async evaluation, or runtime module initialization. Accepting structural enum cases does not authorize constructor hooks, validators, or other user code to run during extraction.

### Metadata lanes

Compiler-semantic keys are interpreted by the language and can affect construction, name resolution, serialization defaults, reflection, or diagnostics. This RFC defines:

- `alias: str`
- `validate`: a checked synchronous callable taking one explicitly typed field-value parameter and returning `Result[None, E]`, where E provides RFC 131's common Error interface.
- `normalize`: an ordered list of checked source fieldmethod identities resolving composed normalization under RFC 087 and RFC 122. These references are construction operations, not ordinary callable values.
- `default_factory`: a checked callable with zero arguments or explicitly typed field-dependency parameters, returning a field value `T` or a fallible `Result[T, E]`, where `E` provides the common Error interface.

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

### Data-exchange direction

`read_only` and `write_only` describe data-exchange contracts. An adapter interpreting `read_only=true` may expose the field in output but must exclude it from accepted input for that projection. An adapter interpreting `write_only=true` may accept the field as input but must exclude it from output. An adapter must explicitly document whether and how it consumes these checked facts and must enforce the interpretation it advertises. RFC 086 owns projection and adapter integration; the compiler preserves and type-checks the metadata rather than independently changing ordinary execution.

These keys must not independently restrict ordinary Incan construction, field access, or mutation, or change member visibility. Visibility and RFC 132 freezing remain governed by their language contracts. For example, `id [read_only=true]: int` does not by itself prohibit an application from supplying `id` to an ordinary model constructor. Excluding that field from an external input projection must not waive the model's checked construction requirements.

`write_only` is not secret protection for logs, diagnostics, inspection, or retained errors. RFC 103's checked secret types and non-disclosure contract apply independently, including failures before secret construction. An adapter or serializer must not use these metadata keys to weaken secret protection. Documentation and tooling must distinguish data-exchange direction from language-level access restrictions and secrecy guarantees.

### Defaults and default factories

A field may define at most one defaulting mechanism.

An RHS default value defines a direct default:

```incan
count: int = 0
```

`default_factory` identifies a callable used to produce a default when the field is omitted. A factory needing no other fields takes zero arguments:

```incan
tags [default_factory=list[str]]: list[str]
```

The `default_factory` callable must be a statically resolvable synchronous callable symbol or constructor surface. It may take zero arguments or declare explicitly typed parameters identifying dependencies on canonical stored fields of the owning model. Each dependency parameter must resolve by its name to a canonical field, with the resolved field value assignable to its declared parameter type. Wire aliases and ambient variables must not substitute for these field identities. Variadic or otherwise unresolvable dependency signatures must receive a diagnostic. The callable must return either a value assignable to the target field type or `Result[T, E]`, where T is assignable to that field type and E provides RFC 131's common Error interface. Signature checking uses the declared callable contract rather than executing the factory or inspecting its body to guess dependencies or failures. Checked metadata and tooling must expose the dependency field identities, parameter types, callable identity, and fallible result contract.

Dependencies must resolve to their effective normalized stored values before the factory runs, including explicit constructor inputs, presets, other defaults, and applicable construction hooks. A factory receives those values through ordinary checked argument rules; it does not receive or expose a partially initialized Self or the construction context. A later declaration may supply a dependency, so field declaration order alone must not make a valid acyclic factory dependency unavailable. Among eligible factories whose dependencies are ready, canonical field declaration order supplies deterministic scheduling. RFC 122's authored fieldmethod order must remain intact: dependency scheduling must not silently reorder those methods. A conflicting read/write schedule must receive a diagnostic identifying the participating fields and hooks.

Supplying the target field explicitly skips its factory and its dependency resolution unless another active construction operation needs those fields. The declared factory dependency graph must be acyclic. The compiler must reject a cycle with a diagnostic showing the dependency chain and available source locations; an otherwise infallible constructor must not acquire an undocumented runtime failure path from dependency resolution. Resolution must not repeatedly recompute values until they appear to stabilize. If a dependency fails, its dependent factory must remain unexecuted and observable as blocked. CollectAll may continue independent eligible factories but must not fabricate failures from blocked factory bodies. This follows RFC 033's dependency-resolution principle while retaining ordinary model construction and RFC 143 cloning contracts.

A fallible factory makes the model's canonical construction contract `Result[Self, ValidationError]`, including for a model without `@derive(Validate)`. This result type must remain consistent when a caller supplies values that skip every fallible factory. Generated construction captures original factory failures under the non-generic ValidationError umbrella, preserving their concrete identities, payloads, hierarchy, and diagnostics. A model already using that umbrella must not acquire an additional nested Result. Caller-supplied effective field values, including constructor presets, bypass that field's default factory. Each invoked factory runs at most once per construction attempt, with no implicit retry or fallback after Err. No failed construction may expose a partially initialized model.

Callable references remain safe metadata: resolving, extracting, or documenting the reference must not run its body. A factory's runtime body may perform ordinary permitted application operations; restrictions on metadata evaluation do not prohibit those runtime operations. RFC 122 construction forms must preserve the canonical result type. RFC 143 cloning retains stored baseline values and must not rerun their default factories; the keyword overload still preserves the canonical Result type and finalization requirements, while zero-argument Clone remains `Self`.

Factory evaluation must expose an explicit selectable failure policy using FailFast and CollectAll semantics. FailFast is the default for ordinary construction and must stop factory evaluation at the first observed Err in canonical construction order. CollectAll must continue with the remaining eligible factories in that order and preserve each produced failure under the common ValidationError umbrella. No concurrent factory execution is implied. Checks requiring unavailable field values must remain unexecuted and observable as blocked; failures from unexecuted code must not be invented. Policy selection, including runtime selection, must not change the constructor's static Result type. Ignoring a failed factory cannot establish a field value or authorize successful construction of an incomplete model.

It is a compile-time error to specify both `default_factory` and an RHS default value on the same field.

### Field validation metadata

`validate=check` must reference a statically resolved synchronous check callable. Its one explicitly typed parameter must accept the field's checked value type under ordinary argument rules, and its result must be `Result[None, E]` where E provides RFC 131's common Error interface. The compiler must diagnose unsupported signatures and non-callable values rather than dispatching through strings. The check declares acceptance or failure; it must not return a replacement field value. Simple type-shaped constraints remain expressible in the type lane rather than being moved to metadata.

The following application declarations describe proposed behavior:

```incan
error LabelError:
    Empty:
        message = "A label cannot be empty."

def non_empty(value: str) -> Result[None, LabelError]:
    if value == "":
        return Err(LabelError.Empty)
    return Ok(None)

field name [validate=non_empty]: str

model Product:
    field name
```

The attached check is part of the reusable field contract. It must be retained wherever that contract is used, with the target field and original callable identity available to diagnostics. RFC 087 additionally admits model-owned `@validate(field_name)` checks and bare `@validate` whole-model checks. A model-specific check must not mutate the shared field declaration or infect unrelated users of that field. An explicit metadata override must not erase retained validation obligations.

Field checks must inspect final normalized values, including effective defaults and factory results, without running during metadata resolution or extraction. Registered checks make canonical construction return `Result[Self, ValidationError]` without an additional `@derive(Validate)` requirement, preserving each original typed error through RFC 131's umbrella. All supported construction and finalization paths must preserve this checked result contract. RFC 122 owns execution integration, check eligibility, composed-pipeline coordination, and the breaking migration from legacy Validate registration. For validate, this RFC selects a single callable metadata value; additional list-valued or multiple-target validation syntax has not been selected.

Field check callables and their transitive helpers must be deterministic and free of external side effects under RFC 122's construction-safety rules. I/O, ambient environment access, mutable global state, clock access, randomness, and asynchronous work must receive compiler diagnostics identifying the forbidden operation and reachable call chain. This applies when a callable is registered as a check; metadata extraction still must not execute it. Default factories retain their separate permitted runtime effects. Capability-governed external validation may be considered separately; no such exception is selected here.

After normalization, field-targeted checks run in canonical field declaration order before whole-model checks, which run in their declaration order. All inspect the same final normalized state. Retained whole-model checks follow recursive left-to-right from-source traversal before target-owned checks; first appearance fixes a same-origin, same-binding check's position and later paths do not rerun it. Checks must handle their inputs independently of whether another check has run or succeeded. For several checks on one field, reusable/source checks run first under first-appearance coalescing, then target-local checks in declaration order. Complete the field's eligible checks before moving to the next field. RFC 122 specifies the full traversal and coverage contract.

Registered checks default to CollectAll, retaining all failures from eligible checks under the same ValidationError umbrella. Explicit FailFast is available without changing the constructor's static result type. Checks requiring unavailable data remain blocked with clear diagnostics, and Ignore cannot permit an invalid model. Models declare the checks, while the runtime selects execution policy through RFC 122's scoped validation_policy context manager. Policy is not a model field or a constructor data argument, and the scope preserves the canonical Result type and restores the prior policy on exit. Ordinary helper and library calls within the same execution context inherit the active scope; function boundaries do not reset it. RFC 122 owns check scheduling and eligibility; RFC 094 and RFC 142 own suspension and spawned-task integration respectively. This validation default is separate from the FailFast default for factory evaluation above.

### Explicit normalization metadata

A target model may explicitly resolve overlapping field-backed normalization through `field name [normalize=[TrimmedName.name, LowercaseName.name]]: str`. In this single-field form, each entry must resolve to a retained source field-backed fieldmethod targeting that canonical field, with its input type matching the field type. The first step receives the effective newly supplied input; each subsequent step receives the value bound by its predecessor. The ordered list must account for every distinct conflicting source normalizer. RFC 122 owns construction activation, input forwarding, and execution; the metadata does not introduce a second normalization engine or make fieldmethods ordinary callable values.

This local declaration refines the composed field's normalization rather than replacing the field contract. Its type and other structural facts must remain compatible, and all retained validation obligations remain enforced on the final value. Checked metadata must preserve operation identities, target bindings, order, and source provenance without executing the operations during extraction. The same spelling admits computed source operations with multi-field write sets: a computed entry receives its own captured named construction input, while a following field-backed entry receives the resulting value of its target field. The computed write set must include the field carrying normalize metadata and preserve its other targets. Competing effective inputs to the same pipeline step must receive a diagnostic rather than implicit precedence; stored defaults and retained clone state are not newly supplied competing inputs. RFC 122 supplies the complete duration example and retains remaining activation questions; extraction must preserve these input and write-target identities without executing the operations.

### Type-shaped constraints

Type-specific constraints should be expressed in type syntax, constrained primitives, validated newtypes, or named semantic types:

```incan
age: int[ge=0]
email: EmailAddress
amount: PositiveCents
```

Field metadata must not become the primary location for numeric bounds, string patterns, length constraints, item constraints, or enum membership when the type system can express those constraints.

Adapters may project type constraints into external schemas. For example, a JSON Schema adapter may map `str[pattern="..."]` into a JSON Schema `pattern`. That is an adapter projection of checked type facts, not a separate field metadata truth.

Field metadata must not redefine the checked Incan source type or its constraints. An adapter must map from the structured Incan type descriptor to its own type system. Explicit namespaced metadata may request a target mapping, such as `postgres.type`, under RFC 086's adapter-owned compatibility contract. These requests must use checked adapter-owned enum values, including safe constant payloads when needed, rather than SQL fragments or other strings encoding target types. The owning adapter must validate the request before using the projection and reject incompatible, unsupported, or unestablished mappings. A request cannot silently narrow source constraints, discard information, introduce unchecked coercion, or weaken secret protection; metadata precedence cannot waive those checks.

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
- attached validation callable identity, checked parameter and result contracts, and provenance;
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
- `default_factory` target is not a synchronous callable with a supported field-value or `Result[T, E]` return contract;
- a factory dependency parameter does not resolve to a canonical stored field or has an incompatible type;
- declared factory dependencies form a cycle or conflict with the checked fieldmethod execution order; diagnostics must show the participating fields, dependencies, and available source locations;
- a fallible factory's error type does not provide the common Error interface;
- a caller treats a fallible model constructor's Result as a completed model value; diagnostics must show the generated Result signature and explain the need to handle or propagate the validation failure;
- validate metadata references a non-callable, unsupported signature, incompatible field-value parameter, or error type without the common Error interface;
- invalid namespaced key syntax;
- namespaced metadata appears where the grammar does not allow metadata;
- field metadata attempts to redefine the checked Incan source type or its constraints;
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

### Failure-policy prior art

Pydantic provides [opt-in FailFast for sequence validation](https://docs.pydantic.dev/latest/concepts/performance/#failing-early-with-failfast), while its [normal validation reports can contain multiple failures](https://docs.pydantic.dev/latest/concepts/models/#error-handling). That is prior art for making early stopping an explicit policy, not evidence that Pydantic normally defaults all validation to FailFast or provides this same factory-policy toggle. Incan deliberately chooses FailFast as the ordinary default for potentially effectful factory evaluation, with explicit CollectAll preserving multiple original failures through the ValidationError umbrella. RFC 142 supplies the shared policy vocabulary; factory evaluation does not inherit every policy available to stream consumers.

### Relationship to RFC 048 and RFC 082

RFC 048 owns checked metadata extraction and stable metadata artifacts. RFC 082 owns documentation generation from checked metadata. This RFC defines additional field metadata facts that those systems must be able to preserve and project.

For metadata-layer and blast-radius ambitions, the important contract is that these field facts are extracted as checked facts with stable anchors, not rediscovered by parsing source text or generated Rust. Downstream systems can attach mutable governance and evidence records to those anchors while the source model remains the readable row-contract declaration.

### Compatibility / migration

This RFC is additive. Existing RFC 021 metadata remains valid.

Code using only `alias` and `description` can continue unchanged.

An infallible default factory does not itself change an ordinary constructor's result type. Introducing a fallible default factory into a previously infallible public model changes its construction contract to `Result[Self, ValidationError]` and therefore requires callers to migrate. Checked API metadata, signature help, and diagnostics must expose this change even when a particular call supplies the affected field explicitly. Adding field metadata is not a promise that every change to compiler-semantic metadata is behaviorally or signature compatible.

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
- **IR Lowering / Emission**: must preserve construction semantics for defaults and default factories, capture fallible factory failures under ValidationError, and preserve canonical result types while making field metadata available to reflection and artifacts.
- **Stdlib / Runtime**: must expose field metadata consistently through reflection.
- **Formatter**: must format inline metadata and multiline metadata lists deterministically.
- **LSP / Tooling**: should provide hover, completion, diagnostics, and go-to-definition behavior for standard field metadata.
- **Docs**: should render high-signal field metadata from checked metadata without letting adapter-specific keys dominate ordinary docs.

## Design Decisions

- Treat ordered normalize metadata as compiler-semantic resolution of composed operations. Preserve source identities, target bindings, complete computed write sets, and validation obligations. Computed entries receive their named construction inputs; following field-backed entries receive resulting target values. Competing effective inputs to one step must receive a diagnostic. RFC 122 owns execution and remaining activation questions.
- Treat `validate=check` as compiler-semantic callable metadata, retained with reusable field contracts under RFC 087. Checks receive a typed field value and return `Result[None, E]`; they do not normalize or replace values. Registered checks activate validated construction without an extra Validate derive, retaining original errors under the common ValidationError umbrella. RFC 122 owns detailed execution integration.

- Registered validation checks default to CollectAll with explicit FailFast available. Both preserve the canonical Result type and typed failures. Blocked checks remain distinguishable from executed failures; factory evaluation retains its separate FailFast default under this RFC.

- Default factories may return `T` or `Result[T, E]` with a common-interface error. A fallible factory makes canonical model construction consistently return `Result[Self, ValidationError]`; the umbrella retains the original typed failure. Supplying the field skips the factory without changing the signature. Invoked factories run at most once per attempt, with no implicit retry, and failed construction never exposes a partial model. Metadata extraction does not execute factories.
- Factories may declare typed field dependencies through parameter names resolving to canonical stored fields. They receive effective normalized values, with dependency-based scheduling, deterministic ordering among ready factories, and diagnostics for cycles or conflicts with RFC 122's preserved authored hook order. Failed dependencies block dependent factories. RFC 033 provides the same resolution principle for ctx; ordinary model construction does not acquire ctx's shared lazy publication or resource semantics.
- Factory failure handling is configurable: ordinary construction defaults to FailFast, and explicit CollectAll attempts remaining eligible factories in canonical order and retains their original failures under ValidationError. The return type remains unchanged, dependent checks remain blocked when required values are unavailable, and neither policy retries factories or promises effect rollback. Pydantic is prior art for configurable early stopping, not for the selected Incan default.
- RFC 122 construction preserves this canonical result type. RFC 143 cloning retains already stored values without replaying their default factories; keyword cloning preserves the canonical construction result and zero-argument Clone remains `Self`.
- Stabilize the standard descriptive key set in this RFC as `description`, `title`, `examples`, `deprecated`, `classification`, `tags`, `primary_key`, `primary_key_position`, `business_name`, `read_only`, and `write_only`.
- Keep `classification` and `tags` as unqualified standard descriptive keys. They are intentionally lightweight checked facts, not a full governance policy model.
- Keep `read_only` and `write_only` as data-exchange contracts consumed and enforced by explicitly documented adapters. They do not independently restrict Incan construction, access, mutation, or visibility, and do not provide or weaken secret protection. RFC 086 owns projection interpretation; language visibility, RFC 132 freezing, and RFC 103 secrecy remain separate contracts.
- Permit namespaced target-mapping requests under RFC 086's owning-adapter compatibility checks. Source field types and constraints remain authoritative. Incompatible, unsupported, or unestablished mappings require diagnostics, and metadata override precedence cannot waive those checks.
- Support structural enum-case metadata with recursively safe, checked payloads. Preserve enum and case identities and payload structure without executing arbitrary constructors or user code.
- Include lists and dictionaries in the first safe metadata value set. Descriptor serialization must support them rather than deferring them.
- Allow `examples` to contain safe metadata values assignable to the field type, with string examples allowed when typed examples are not available.
- Keep `primary_key` and `primary_key_position` as field metadata. Composite key order is represented by field-level positions; richer model-level identity constraints can be defined by a future RFC if needed.
- Treat stable field anchors as source-field and model-field metadata identities, not as external alias identities. A canonical field rename is a new source identity unless a future migration or rename-marker feature explicitly preserves continuity. External aliases and adapter mappings do not by themselves preserve the Incan field anchor.
