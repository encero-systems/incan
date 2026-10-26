# RFC 087: Reusable field contracts and model contract composition

- **Status:** Planned
- **Created:** 2026-04-29
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 017 (validated newtypes with implicit coercion)
    - RFC 021 (model field metadata and schema-safe aliases)
    - RFC 048 (checked contract metadata, Incan emit, and interrogation tooling)
    - RFC 085 (field metadata and type-shaped constraints)
    - RFC 086 (schema descriptors and adapters)
    - RFC 103 (secret types and non-disclosure)
    - RFC 122 (model construction hooks and finalization)
    - RFC 131 (typed validation errors and umbrellas)
- **Issue:** #474
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC introduces reusable field contracts, named validation checks, and complete model contract composition. A module-scope `field` declaration defines one reusable field with its type, constraints, metadata, defaulting, and attached checks. Models compose complete contracts through `model Booking from Window, Capacity:`, retaining their declared normalization and validation obligations without class inheritance or nominal subtyping. Field metadata `validate=check` references an ordinary typed check; model-local `@validate(field_name)` declarations check one field, while bare `@validate` declarations check relationships across the model. The compiler owns construction integration and provenance so authors can describe strict data contracts without manually assembling validation dispatchers.

## Core model

1. **Every model field is semantically a field:** `email: EmailAddress` inside a model is sugar for an explicit local field declaration.
2. **Reusable field contracts are explicit at module scope:** top-level reusable fields must use the `field` keyword.
3. **Simple model fields retain their shorthand:** ordinary field declarations and unambiguous reusable references may omit `field`. The validation examples use explicit `field` declarations to make attached field contracts visible.
4. **Composition preserves complete contracts:** `from` composes fields, constraints, defaulting, normalization, and declared validation. It does not create a subtype relationship or import arbitrary utility methods. `with` remains trait adoption.
5. **Composition merges compatible information:** duplicate canonical fields must agree on type and construction rules. Descriptive and namespaced metadata may combine; contradictory values require explicit reconciliation in the owning model's schema block, with every origin retained.
6. **Provenance is preserved:** metadata extraction must record both the original source field anchor and the composed target model-field anchor.
7. **Checks are contract obligations:** field metadata and named validation declarations are enforced by construction and retained by composition. Schema projection metadata cannot override or suppress them.
8. **Schema overlays stay separate:** RFC 086 can attach projection metadata to composed models, but schema metadata cannot add, remove, or retype fields.

## Motivation

Large row-shaped systems repeat the same fields everywhere: `tenant_id`, `user_id`, `account_id`, `created_at`, `updated_at`, ingestion timestamps, external IDs, partition keys, and business identifiers. In realistic models those fields often carry metadata, descriptions, aliases, defaults, classifications, schema-adapter mappings, and stable extraction anchors. Repeating that contract by hand across many models is both noisy and dangerous.

The problem becomes sharper when one row shape is derived from another. A query join, warehouse projection, API response model, or contract-backed row may absorb fields from multiple existing row definitions. Users should not have to retype `user_id [description="...", classification="...", ...]: UserId` in every derived shape. At the same time, the language must avoid hidden inheritance behavior and silent field conflicts.

RFC 085 defines what individual fields can carry. RFC 086 defines how schema descriptors and overlays consume model metadata. This RFC defines how checked field and model contracts can be reused before schema overlays or adapters enter the picture. Reusing a model must preserve its declared validation and normalization obligations, not merely copy a shape and warn that enforcement was lost.

## Goals

- Add a module-scope `field` declaration for reusable single-field contracts.
- Make `field` an explicit semantic concept for all model fields.
- Allow `field name` inside a model to import a reusable field contract.
- Allow bare `name` inside a model as sugar for `field name` when `name` resolves unambiguously to a reusable field contract.
- Allow `field name: Type` inside a model as an explicit local field declaration equivalent to `name: Type`.
- Use `model Name from SourceA, SourceB:` for complete model contract composition.
- Support shared field check functions through RFC 085 validate metadata and named model-local field or whole-model checks through `@validate`.
- Retain contract normalization and validation without handwritten dispatchers or obligatory validation traits.
- Preserve field metadata, defaults, aliases, type constraints, and source anchors when reusable fields are imported into models.
- Preserve provenance from source field contracts to target model fields in checked metadata extraction.
- Define compatible duplicate-field merging, explicit metadata reconciliation, and strict structural conflict diagnostics.

## Non-Goals

- Defining schema overlays, schema imports, adapter interpretation, or descriptor projection. Those belong to RFC 086.
- Expanding the field metadata key set. RFC 085 owns field metadata keys and type-shaped constraints.
- Introducing class inheritance, model inheritance, method inheritance, or subtype relationships.
- Defining query syntax or query planner behavior.
- Allowing composition or schema metadata to override source field types, constraints, aliases, or construction rules.
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

For larger reusable groups, compose the complete model contract in the header:

```incan
model AuditFields:
    created_at
    updated_at

model User from AuditFields:
    tenant_id
    user_id
    email [classification="personal"]: EmailAddress
    lifecycle_status: LifecycleStatus
```

Composition preserves provenance. A metadata-layer tool should be able to report that `User.user_id`, `UserAccountRow.user_id`, and `SomeApiUser.user_id` all came from the reusable `user_id` field contract, while also treating each target model field as its own checked field anchor.

### Field checks and named model checks

All declarations in this example are application-defined and describe proposed behavior. `ContractError` uses RFC 131 error syntax. The shared field's `non_empty` function is an ordinary check, while the decorated declarations register checks within their owning model contract.

```incan
error ContractError:
    EmptyLabel = "A label cannot be empty."
    ReservedLabel(value: str)
    ReversedWindow(minimum: int, maximum: int)

def non_empty(value: str) -> Result[None, ContractError]:
    if value == "":
        return Err(ContractError.EmptyLabel)
    return Ok(None)

field label [validate=non_empty]: str

model Window:
    minimum: int
    maximum: int

    @validate
    def ordered(self) -> Result[None, ContractError]:
        if self.minimum > self.maximum:
            return Err(ContractError.ReversedWindow(
                minimum=self.minimum,
                maximum=self.maximum,
            ))
        return Ok(None)

model Schedule from Window:
    field label

    @validate(label)
    def not_reserved(value: str) -> Result[None, ContractError]:
        if value == "system":
            return Err(ContractError.ReservedLabel(value=value))
        return Ok(None)

model Product:
    field label
```

Schedule has minimum, maximum, and label fields. It retains `Window.ordered` and the shared field's `non_empty` check, and contributes its own not_reserved check. Product receives the shared `non_empty` check but does not acquire Schedule-specific rules. Named checks do not need a common method name or an authored dispatcher. Their presence activates checked, fallible construction without an additional `@derive(Validate)` requirement. Canonical construction uses RFC 131's ValidationError umbrella while retaining original typed failures.

A local field can reference an existing check without declaring a reusable module-level field:

```incan
model Customer:
    field name [validate=non_empty]: str
```

A check attached to an individual field is not a cross-field model contract. Reusing minimum alone cannot retain Window's relationship with maximum; composing from Window does.

### Composing normalization and validation

The following proposed example combines a shared field check, two source normalization contracts, a model-local field check, and a whole-model check. All declarations are application-defined. RFC 122 supplies fieldmethods; RFC 143 supplies keyword cloning. ValidationError is RFC 131's proposed shared validation umbrella; its final stdlib module placement remains open.

```incan
error AccountError:
    EmptyName = "An account name cannot be empty."
    ReservedName(value: str)
    ReversedLimits(minimum: int, maximum: int)

def non_empty(value: str) -> Result[None, AccountError]:
    if value == "":
        return Err(AccountError.EmptyName)
    return Ok(None)

field name [validate=non_empty]: str

model TrimmedName:
    field name

    @fieldmethod
    def name(mod, value: str):
        mod.name = value.strip()

model LowercaseName:
    field name

    @fieldmethod
    def name(mod, value: str):
        mod.name = value.lower()

@derive(Clone)
model Account from TrimmedName, LowercaseName:
    field name [normalize=[TrimmedName.name, LowercaseName.name]]: str

    minimum: int
    maximum: int

    @validate(name)
    def not_reserved(value: str) -> Result[None, AccountError]:
        if value in ("admin", "system"):
            return Err(AccountError.ReservedName(value=value))
        return Ok(None)

    @validate
    def limits_ordered(self) -> Result[None, AccountError]:
        if self.minimum > self.maximum:
            return Err(AccountError.ReversedLimits(
                minimum=self.minimum,
                maximum=self.maximum,
            ))
        return Ok(None)
```

The local name declaration resolves normalization for the already composed field. It retains the shared non_empty check, which coalesces to one obligation because both source paths refer to the same originating field and check. The normalize entries are checked source fieldmethod identities, not strings or calls. Their explicit order trims the newly supplied name, then lowercases the resulting stored field value. Changing the from-header order does not change that pipeline. All checks inspect the final normalized values; no check or normalizer runs merely because a declaration or construction plan is extracted.

| Explicit inputs                         | Proposed result                                                      |
| --------------------------------------- | -------------------------------------------------------------------- |
| `name=" Alice ", minimum=1, maximum=10` | Ok with `name="alice"`                                               |
| `name="   ", minimum=1, maximum=10`     | ValidationError retaining `AccountError.EmptyName`                   |
| `name=" ADMIN ", minimum=1, maximum=10` | ValidationError retaining `AccountError.ReservedName(value="admin")` |
| `name=" Alice ", minimum=10, maximum=1` | ValidationError retaining `AccountError.ReversedLimits`              |

The same contract applies to keyword cloning:

```incan
def example() -> Result[Account, ValidationError]:
    account = Account(name=" Alice ", minimum=1, maximum=10)?
    updated = account.clone(minimum=2)?
    return updated.clone(name=" BOB ")
```

The first clone retains the stored name alice without rerunning its normalizers. The second supplies a new name and runs the selected pipeline on that input, producing bob. Both keyword clones retain complete final validation and its Result contract. Zero-argument cloning remains `Self`. No clone replays original constructor inputs. Composed-pipeline activation and overlapping-pipeline coordination remain deferred design work in RFC 122. Check scheduling and ordinary-call scope inheritance are specified there; suspension and spawned-task integration belong with RFC 094 and RFC 142.

Registered checks default to CollectAll, with explicit FailFast available. For `Account(name="   ", minimum=10, maximum=1)`, the normalized empty name and reversed limits are both failures: eligible checks retain both original AccountError values under ValidationError. Neither policy returns an invalid Account, and the static construction result remains unchanged. Checks requiring unavailable data remain blocked with clear diagnostics. This validation default does not change RFC 085's separate FailFast default for factory evaluation.

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

Complete model contracts compose in the header, before any existing trait adoption clause:

```text
model_header = "model" IDENT type_params? composition_sources? trait_adoptions? ":" ;
composition_sources = "from" type_path { "," type_path } ;
```

The existing visibility, type-parameter, and with-trait syntax remains applicable. This proposal uses from rather than ellipsis model spreads.

The grammar must distinguish `IDENT ":"` as a local model field declaration and bare `IDENT` as a reusable field reference. A bare identifier in a model body must not be parsed as an arbitrary expression statement.

### Module-scope reusable fields

A module-scope `field` declaration defines a reusable field contract. The contract includes:

- canonical field name;
- field metadata from RFC 085;
- alias sugar from RFC 021 where allowed;
- type expression;
- default or default factory where allowed by RFC 085;
- attached field-validation callable references and their checked identities;
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

### Complete model contract composition

`model Target from SourceA, SourceB:` must compose the complete declared data contracts of the selected source models. Sources must resolve to visible, checked models with statically known contract metadata. The target receives their stored fields, constraints, defaulting, declared normalization, field checks, and whole-model checks, together with source identities. A dedicated field-group kind is not required; ordinary models can provide reusable groups.

The resulting target is a distinct model type. Composition must not imply nominal assignability to a source model, inherit arbitrary utility methods or associated functions, or automatically select source schema overlays. Source contract execution may depend on source-owned helpers, but retaining those dependencies must not expose them as new ordinary target members or bypass visibility.

A source check must remain bound to its defining contract and required fields. The compiler must not reinterpret its self or helper lookup as arbitrary target-model inheritance. Normalized values must contribute to the actual target state; validating a temporary source instance and returning different unnormalized target values does not preserve the contract. Unsupported or incompatible retained construction requirements require a diagnostic rather than silently omitting a rule. These requirements apply equally to local models and models published by external loaves.

RFC 132's Frozen model guarantee is part of the complete composed contract. A target composing any frozen source must explicitly retain Frozen for all its stored fields. A frozen target may compose ordinary sources while preserving their construction and behavior obligations; this creates no model inheritance or mixed field-level freeze mode.

RFC 122 owns construction integration, including the detailed scheduling and binding questions still being reviewed there. This RFC selects the authoring syntax and the requirement to preserve contracts; it does not claim that source ordering alone resolves overlapping normalization or that the current compiler implements contract composition. Creating or extracting a declaration must not execute its checks, defaults, or factories.

### Named validation declarations

Field metadata `validate=check` must follow RFC 085's checked callable contract. It attaches a value check to the reusable or local field declaration.

Inside a model, `@validate(field_name)` must register a model-owned field check. Its declaration must take one explicitly typed field-value parameter, with no self or cls receiver, and return `Result[None, E]` where E provides RFC 131's common Error interface. It is a validation declaration rather than an ordinary instance method missing its receiver. The target must resolve to a canonical stored field of the owning model, including a field obtained through composition. A wire alias, arbitrary string, unknown field, or incompatible parameter type must receive a diagnostic. This form selects one field; no multiple-target spelling has been selected.

Bare `@validate` must register a named whole-model check with an immutable self receiver, no caller-supplied parameters, and `Result[None, E]` return type. It permits checks involving multiple fields without forcing every rule into one method named validate. Check declarations must be synchronous and must not return replacement model or field values; normalization belongs to the construction contract.

New registered field and whole-model checks must be deterministic and free of external side effects, including their transitive helpers and checks retained from composed source contracts. RFC 122 owns the shared construction-safety verifier and diagnostics identifying forbidden operations and their call chains. External application work must occur explicitly outside registered validation; no capability-based exception is selected by this proposal. Legacy Validate registration requires the explicit breaking migration in RFC 122; it is not automatically adapted or run alongside registered checks.

Within a model, registered field checks run after normalization in canonical field declaration order, followed by whole-model checks in their declaration order. Every eligible check inspects the same final normalized state rather than receiving a transformed result from a prior check. Under the default CollectAll policy, later eligible checks still run after an earlier typed failure. Retained whole-model checks traverse from sources recursively left to right, visiting their composed sources before their own declaration-ordered checks, then target-owned checks. First appearance fixes each same-origin, same-binding check's position; later paths neither rerun nor reposition it, even if their source order differs. This orders retained obligations rather than resolving methods or overriding checks: report order and FailFast are observable, while normalization keeps its separately explicit normalize order. Checks must handle their inputs without relying on another check having run or succeeded. Differing source orders alone must not reject valid contract composition. Within one field, reusable/source checks precede target-local checks, with shared identities coalescing at first appearance and local checks retaining declaration order. Complete that field's eligible checks before moving to the next field. RFC 122 specifies this traversal without selecting a general MRO.

Both field-check spellings must feed the same checked field-validation mechanism. Model-local field checks belong to their owning model contract: composing that model retains them, but another model reusing the same module-level field must not acquire them. Check identities must retain their defining package, model or callable, and target field where applicable. Same-spelled checks from distinct source contracts must not collide merely because their names match. Ordinary undecorated model methods remain ordinary behavior, not implicitly registered checks.

The presence of registered checks must require validated construction without a separate Validate derive. Canonical construction must return `Result[Self, ValidationError]` under RFC 131, retaining concrete failures and diagnostic provenance. Field checks inspect final normalized field values, including effective defaults and factory results; they must not transform those values or run during metadata extraction. Every supported construction, decoding, and keyword-clone finalization path must honor the registered contract. RFC 122 records remaining execution details and the explicit breaking migration replacing `@derive(Validate)` and value-returning constructor validation. Checks move to named declarations, transformations to fieldmethods, and external work to application code, with clear migration diagnostics. This proposed replacement must not be represented as implemented today.

### Duplicate fields and conflicts

Duplicate canonical fields introduced by local declarations, reusable references, or composed source models must merge into one stored target field only when their structural contracts agree. Agreement requires the same canonical name, resolved type and constraints, nullability and optionality, compiler-semantic alias, default/default-factory contract, and other compiler-semantic construction facts. Attached validation obligations are retained as described below rather than compared as overridable descriptive metadata. A structural disagreement must receive a compiler diagnostic identifying the contributing contracts and differing facts. Neither schema overrides nor selected overlays may repair such a disagreement by changing the field contract.

Different source normalizers with overlapping declared write targets must require explicit resolution in the target construction contract. Without that resolution, the compiler must diagnose the conflicting normalizers, their source contracts, and shared target fields. The order of sources in a from header must not implicitly select their execution pipeline. Repeated references to the same originating normalizer with the same target binding coalesce to one operation; distinct identities must not be assumed equivalent by inspecting names or observed results. Explicit normalization resolution must not suppress retained validation, and RFC 086 schema metadata overrides must not substitute for construction resolution. For single-field field-backed operations, a local field declaration with `normalize=[SourceA.fieldmethod, SourceB.fieldmethod]` selects the ordered pipeline under RFC 085 and RFC 122. It refines the existing composed field without creating another stored field or replacing its retained checks. The same spelling supports a computed source operation receiving its own named construction input followed by a field-backed operation receiving the resulting target value. Every declared computed write target remains part of the operation. Competing effective inputs to the same composed pipeline step must receive a diagnostic identifying both inputs and the affected field and operation; neither input may silently be discarded. This does not make all overlapping authored write sets mutually exclusive. RFC 122 provides the complete example and retains remaining activation questions.

All contributing validation obligations must remain enforced. The same checked source check associated with the same target field or contract coalesces to one obligation; different callable or contract identities must not be deduplicated merely because names, messages, or current outcomes agree. Metadata := overrides must not remove or replace retained checks. Registered checks default to CollectAll with explicit FailFast available through RFC 122's runtime-owned validation_policy scope. Models declare rules rather than storing or selecting execution policy. Ordinary helper and library calls within the same execution context inherit the active scope. RFC 122 owns scheduling and eligibility; RFC 094 and RFC 142 own suspension and spawned-task integration respectively.

Standard descriptive and namespaced metadata must merge by normalized field and metadata key. Different keys combine. Equal checked values for the same key coalesce while preserving every contributing origin. Different values for the same key remain an unresolved metadata conflict until an explicit := assignment in the owning model's schema block selects the effective value. The model must not become a valid composed declaration while such conflicts remain unresolved. A later selected schema overlay cannot rescue an invalid base model.

For example, two structurally compatible `tenant_id` fields carrying `postgres.name` and `proto.tag` respectively contribute both keys. If both instead specify different `postgres.name` values, the owning model may reconcile them explicitly:

```incan
# BillingFields and DeliveryFields are application-selected imported models.
from billing_contracts import BillingFields
from delivery_contracts import DeliveryFields

model Shipment from BillingFields, DeliveryFields:

    schema:
        postgres:
            tenant_id.name := "shipment_tenant"
```

Composition must not silently select a conflicting value by source order, concatenate arbitrary metadata lists, or infer semantic precedence between unknown namespace values. This RFC does not introduce a special union rule for tags. An explicit override must retain all incoming values and origins together with the resolving statement. Target-mapping reconciliation remains subject to RFC 086 adapter compatibility checks, and metadata reconciliation must not weaken the secret-type guarantees of RFC 103.

Inline metadata originating from a reused field is inherited composition metadata, not newly authored inline metadata on the target model. RFC 086 must treat it as a composition baseline before the owning model's metadata merge. A new inline declaration must not silently settle an unresolved inherited metadata conflict: explicit reconciliation is required. Every accepted merge must preserve all contributing source anchors and the target field anchor; composition must not execute defaults or factories to compare contracts.

### Shared-field extraction warning

When composition coalesces identical complete contracts from distinct, independently authored field declarations, the compiler should emit a warning suggesting extraction into a shared reusable field if all contributing declarations are known to belong to editable project source. For example, independently declaring region: str = "eu" in both StorageFields and ReportingFields can be replaced with one module-scope field region: str = "eu" referenced by both models.

The warning must point to the composition site and identify the contributing declarations. It must not reject an otherwise valid merge or alter the resulting field. Emit at most one such warning per composed target field, grouping repeated paths to the same origins. Repeated references to the same reusable field or the same originating field through multiple composition paths must not trigger it. Compatible fields carrying different metadata are not identical complete contracts and must not trigger this extraction warning merely because their structural facts agree.

Do not emit this refactoring warning when any contributing declaration comes from an external loaf, immutable dependency artifact, or other source that is not established as editable project source. Source editability must come from project and package provenance, not merely from filesystem writability; a writable dependency cache is not authority to change a published contract. These exclusions affect only the extraction suggestion: structural and metadata conflict diagnostics remain required for every source.

### Metadata extraction

Checked metadata extraction must represent reusable field contracts and composed model fields without losing provenance.

The extracted representation must preserve at least:

- reusable field identity and stable anchor where available;
- target model-field identity and stable anchor where available;
- provenance edge from target model field to reusable field or composed source field;
- whether the target field was declared locally, imported by `field name`, imported by bare `name`, or composed through the model header's from clause;
- normalized field metadata after composition;
- source model-contract identities, checked field and whole-model check identities, their target fields, and retained normalization obligations;
- duplicate-field merge provenance, incoming metadata values, and explicit reconciliation records;
- rejected conflict diagnostics where a tooling mode exposes failed extraction.

Blast-radius tools can then answer questions such as which models would be affected by changing the reusable `tenant_id` contract, which query-derived row shapes absorbed `user_id`, or which schema overlays depend on a composed field anchor.

### Interaction with RFC 085

Reusable field contracts use the field metadata, defaulting, safe value, and type-shaped constraint rules from RFC 085. This RFC does not add new metadata keys.

When a reusable field contract is imported into a model, its RFC 085 metadata is preserved as checked field metadata on the target model field. The target field also records that the metadata originated from a reusable field contract.

### Interaction with RFC 086

RFC 086 schema blocks and overlays may attach schema metadata to models built with reusable fields and complete contract composition. Schema metadata still cannot add, remove, reorder, or retype fields.

The owning model's schema block may explicitly reconcile conflicting descriptive or namespaced metadata with := after structural agreement has been established. Schema overlays target a valid composed model descriptor after this reconciliation; they cannot repair an invalid base model. Adapter projection should be able to trace from an overlay field back to the target model field and, where applicable, back to the reusable field contract or composed source field.

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

### Why contract composition is not inheritance

Models are executable data contracts, including their declared normalization and validation. Reusing a contract must preserve those obligations without requiring an object hierarchy, inherited utility methods, method-resolution order, or manually extracted validation traits. The from header makes contract sources explicit while the target remains a distinct model type. Field reuse remains useful for individual declarations; it is not an adequate substitute for retaining cross-field rules from a complete model.

### Compatibility / migration

This RFC is additive. Existing model field declarations remain valid because `field` is optional inside model bodies.

Projects can introduce reusable field contracts incrementally by extracting repeated fields into module-scope `field` declarations and replacing repeated local declarations with `field name` or bare `name` references inside models.

## Alternatives considered

1. **Use `...field_name` for reusable single fields**
   - Rejected because spread syntax visually suggests a larger structure. It is too loud for common scalar fields such as IDs and timestamps.

2. **Allow top-level implicit field declarations**
   - Rejected because module scope should remain explicit and declaration-only. `field user_id: UserId` is clearer than `user_id: UserId` at top level.

3. **Use only complete model composition and no reusable field declarations**
   - Rejected because many reuse cases are one-field contracts. Requiring users to create tiny one-field models for every shared ID or timestamp is awkward and obscures intent.

4. **Use inheritance**
   - Rejected because contract reuse does not require nominal subtyping or arbitrary behavior inheritance. Declared validation and normalization remain part of the composed contract.

5. **Allow local structural overrides during composition**
   - Rejected because choosing a different source type, alias, or construction rule would silently change the contributing contract. Compatible metadata may merge, and the owning model may explicitly reconcile descriptive or namespaced metadata through RFC 086 schema overrides with retained provenance.

6. **Copy model fields and warn that validation was omitted**
   - Rejected because a warning does not preserve an executable data contract. Composition must retain declared checks and normalization or diagnose why the contract cannot be preserved.

7. **Define validators in nested field bodies or decorate a field declaration directly**
   - Not selected. validate metadata references reusable typed functions; model-local `@validate(field_name)` declarations keep local checks close to the model without another field-body scope or a second direct field-decorator spelling.

## Drawbacks

This RFC adds another declaration form and introduces a bare-identifier form inside model bodies. The grammar and diagnostics must be strict so users do not confuse reusable field references with arbitrary statements.

Composition can hide field origins if tooling does not surface provenance well. The metadata extraction requirements are therefore not optional polish; they are part of the feature's correctness story.

Structural disagreements still require users to choose or define compatible field contracts. Metadata reconciliation adds a checking phase: the compiler must retain conflicting inherited values long enough for an explicit owning-model schema override, while refusing to expose an unresolved model as valid.

## Implementation architecture (non-normative)

A useful implementation shape is to normalize model bodies into explicit field entries before downstream checking. Local declarations, explicit reusable field references, implicit bare references, and from-composed contracts can become checked field entries and retained rule obligations with source identities and provenance. Named validation declarations and metadata callable references should share one checked check representation; execution integration remains owned by RFC 122.

Reusable field contracts should participate in the same checked metadata extraction family as models. A composed model descriptor should expose both the final field contract and the source contract ancestry.

## Layers affected

- **Parser / AST**: must support module-scope `field` declarations, optional `field` in model-local field declarations, reusable field references in model bodies, from composition headers, and named validation declarations.
- **Typechecker / Symbol resolution**: must resolve reusable field references, validate model contract sources and registered validation signatures, reject unresolved bare field references, detect duplicate-field conflicts, reject composition that cannot preserve source contract obligations, suggest shared-field extraction for identical independently authored editable-source contracts, and preserve source provenance.
- **Checked metadata extractor**: must preserve reusable field anchors, target model-field anchors, composition provenance, compatible field-merge provenance, explicit metadata reconciliation records, retained validation and normalization identities, and conflict diagnostics where available.
- **IR Lowering / Emission**: must lower composed models as ordinary models with explicit fields and preserved construction/check obligations after composition, without introducing runtime inheritance.
- **Stdlib / Runtime**: should expose composed model field metadata through the same reflection surfaces used for ordinary model fields.
- **Formatter**: must format module-scope field declarations, explicit model-body `field` usage, bare field references, from composition headers, and named checks deterministically.
- **LSP / Tooling**: should provide completion for reusable fields inside models, go-to-definition from model fields to reusable source contracts, hover provenance, duplicate conflict diagnostics, source-aware shared-field extraction warnings, diagnostics for unpreservable contract obligations, and rename support that respects stable anchors.
- **Docs**: should render reusable field contracts and composed fields without hiding where a field contract originated.
- **Build / Packaging**: should preserve reusable field contracts and composition provenance in artifacts that claim checked metadata support.

## Design Decisions

- Use `validate=check` metadata for reusable or local field checks; use model-local `@validate(field_name)` for a named field-value check and bare `@validate` for a whole-model check. Checks return `Result[None, E]`, retain typed failures, and activate validated construction without an extra Validate derive. Nested field validator bodies and direct field-declaration decorators are not selected.

- Merge duplicate canonical fields when their structural contracts agree. Combine disjoint descriptive and namespaced metadata, coalesce equal checked values, and require explicit owning-model schema reconciliation for conflicting values. Reject structural disagreements and preserve every contributing origin.
- Warn when identical independently authored fields from known editable project sources are coalesced, suggesting a shared reusable field. Do not warn for repeated use of an existing source field or when any contributing declaration is from an external loaf or otherwise not established as editable project source. Group the warning per target field and preserve valid composition.
- Use model Name from SourceA, SourceB for complete contract composition. Preserve declared defaults, normalization, field validation, and whole-model checks, including from external loaves. Do not silently drop validation, introduce nominal subtyping, or inherit arbitrary utility methods. RFC 122 owns detailed execution integration.
- Require explicit resolution for different normalizers whose declared write targets overlap. Diagnose unresolved overlap with source and field identities; from-header order does not define the pipeline. Repeated use of the same originating normalizer and binding remains one operation. Select ordered normalize metadata for field-backed pipelines and computed-to-field pipelines: computed entries receive captured named inputs, and following field-backed entries receive resulting target values. Preserve complete write sets. Diagnose competing effective inputs to the same step. RFC 122 retains remaining activation questions.
- Allow from composition to target visible ordinary model declarations. A dedicated field-group declaration is not part of this RFC.
- Treat visibility as a property of the target model declaration, not the reusable field contract. Importing a reusable field contract into a model does not import standalone visibility.
- Use RFC 086 := assignments in the owning model's schema block for descriptive and namespaced metadata reconciliation. Reject unresolved base-model conflicts before overlay selection. Do not allow structural overrides or implicit list concatenation, and do not define a special tags union rule.
- Treat reusable field anchors and target model-field anchors as source identities. Renaming a reusable field contract creates a new identity unless a future migration or rename-marker feature explicitly preserves continuity. External aliases do not preserve the Incan anchor by themselves.
- Formatter behavior must preserve explicit `field name` references when written by the user and must not rewrite them to bare references. Generated source should prefer explicit `field name` for reusable field references in mixed model bodies.
