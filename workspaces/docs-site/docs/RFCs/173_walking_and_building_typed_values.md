# RFC 173: Walking and reconstructing typed values

- **Status:** Planned
- **Created:** 2026-11-07
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 021 (model field metadata and aliases)
    - RFC 024 (extensible derive protocol)
    - RFC 085 (field metadata and rename continuity)
    - RFC 103 (secret values)
    - RFC 122 (model construction contracts)
    - RFC 141 (type matching and named type operations)
    - RFC 146 (typed JSON decoding and complete validation)
    - RFC 164 (explicit ownership and borrowing)
    - RFC 165 (typed embedded storage)
    - RFC 168 (`std.primitives`)
- **Issue:** [#2519](https://github.com/encero-systems/incan/issues/2519)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Every model, enum and newtype gets two members that the compiler fills from its checked declaration. `__walk__` visits a value with each field's declared type, and `__reconstruct__` restores stored values from a source and validates the resulting state without replaying constructor input transformations. Neither knows any format. A codec, a hash or a reference finder is ordinary library code that implements `Walker[E]` or `Source[E]`, and a type that wants another stored form writes the members by hand. `__shape__()` gives those libraries the facts of the declaration: field identities, `canonical` settings and canonical type spellings.

## Motivation

Existing reflection exposes field metadata through `__fields__()` and typed public-field values through `__field_value__()` and `__field_items__()`. The latter preserve a common field type or a union of exposed field types; they do not stringify every value. These views do not supply the complete field-by-field typed reconstruction protocol defined here, including private stored fields and model-owned final validation. Existing format-specific derives provide their own generated conversion paths. A library needs a shared protocol for complete typed traversal and reconstruction instead of a new compiler integration for each format.

Without a shared structural protocol, each format must obtain equivalent traversal and reconstruction through its own generated code or handwritten adapters. RFC 165's Crumb encoding, its hashes and its search for retained references would each need the same, and RFC 165 forbids that shortcut: a format that only the compiler can write is one no library author can write.

The compiler already knows every field, type and default. It should hand them to library code once, in a form any format can use, and then know nothing about formats.

## Goals

- Let a library read every field of a value with its declared type, and construct a value, with no compiler support for that library.
- Keep the canonical construction contract: no format can produce an invalid value.
- Make unsupported values a refusal that names the field path, wherever they are reached.
- Cost nothing where it is not reached.

## Non-Goals

- A wire format. Formats are library code.
- General runtime reflection. A walk is resolved statically through a bound, and RFC 141's `TypeInfo` remains the way to inspect a type's structure.
- Replacing the derive protocol of RFC 024, or the adapters of RFC 086.
- Changing ordinary field access, assignment or constructor-argument visibility. Walking exposes the complete data contract through a separate model-owned protocol.

## Guide-level explanation

### The model owns its data contract

The model's checked declaration supplies its compiled traversal and reconstruction code. A codec uses that code; it does not implement a second interpretation of the model's fields or validation rules. During encoding, the model's `__walk__` calls the writer through typed operations. During decoding, the format parser reads its representation and supplies a `Source` to the selected model's `__reconstruct__`, which restores stored fields and enforces final validation. Nested models follow their own compiled contracts through the same mechanism.

The format owns syntax, framing, encoded type and schema compatibility checks, and format-specific failures. The model owns the meaning and validity of the reconstructed instance. A document identifies data and its declared shape; it does not supply executable reconstruction code. A stored description alone is not a replacement for the selected model's compiled implementation.

The visitor and source implementations satisfy the ordinary checked trait contracts. This RFC does not introduce a separate runtime negotiation mechanism for statically known field types.

### What a model has

```incan
model Point:
    x: int
    label: Option[str]
```

`Point` needs no derive. Its generated walk behaves as follows, where `POINT_X` and `POINT_LABEL` stand for the `FieldInfo` of each field:

```incan
    def __walk__[E, W with Walker[E]](self, mut walker: W) -> Result[None, E]:
        walker.begin_model(Point.__shape__())?
        walker.field(POINT_LABEL, self.label)?
        walker.field(POINT_X, self.x)?
        return walker.end_model()
```

Fields are visited in ascending identity order. This illustrative expansion assumes `POINT_LABEL.identity` sorts before `POINT_X.identity`; source declaration order does not decide that order. Each call passes the field as itself, with its own type, without requiring a copy of the field value.

The generated `Point.__reconstruct__(source)` reads those stored fields, checks their types and applicable model validation, and returns a complete `Point`. It does not delegate to an ordinary constructor that could transform the supplied values again.

### Write a walker

A walker implements only what it cares about, because `Walker[E]` gives every member a default that walks on into the value. This one lists the fields it passes:

```incan
from std.reflection import FieldInfo, Walk, Walker

class FieldNames with Walker[str]:
    pub names: list[str]

    def field[V with Walk](mut self, info: FieldInfo, value: V) -> Result[None, str]:
        self.names.append(info.name)
        return value.__walk__(self)

def field_names[T with Walk](value: T) -> Result[list[str], str]:
    mut walker = FieldNames(names=[])
    value.__walk__(walker)?
    return Ok(walker.names)
```

A nested model, a list of models and an optional model all reach `field` the same way, because each of them walks itself. RFC 165's Crumb encoder is a walker, its hashes are computed by another, and its search for retained references is a third.

### Reconstruct through a source

A source answers what `__reconstruct__` asks. It finds the field by identity, reads it, and says what the input lacks:

```incan
from std.reflection import Reconstruct, Source

def restore[T with Reconstruct, E, S with Source[E]](mut source: S) -> Result[T, E]:
    return T.__reconstruct__(source)
```

The caller supplies a format-owned `Source` implementation. RFC 165's `Page.from_crumb(...)` creates the Crumb source and delegates to the same model hook; the compiler needs no knowledge of Crumb framing.

### Virtual construction inputs and stored fields

A **virtual construction input** is an input accepted during construction whose value is used to establish stored fields. It is not itself a stored field. RFC 122's parameterized fieldmethods declare these inputs and their exact stored-field targets.

For example, `hours` is a virtual construction input and `minutes` is a stored field:

```incan
error DurationError:
    Negative(minutes: int)

model Duration:
    minutes: int = 0

    @fieldmethod(minutes)
    def hours(mod, value: int):
        mod.minutes = value * 60

    @validate
    def non_negative(self) -> Result[None, DurationError]:
        if self.minutes < 0:
            return Err(DurationError.Negative(minutes=self.minutes))
        return Ok(None)
```

The following statements are function-body fragments using RFC 122 construction and RFC 165 Crumb conversion:

```incan
duration = new Duration.hours(2)?
assert duration.minutes == 120

crumb = duration.crumble()?
restored = Duration.from_crumb(crumb)?
assert restored.minutes == 120
```

Construction supplies `hours=2`, executes the fieldmethod to establish `minutes=120`, then validates the resulting state. The crumb contains `minutes=120`. It contains neither an `hours` field nor a record of the call that produced the minutes. Reconstruction restores `minutes=120` and runs `non_negative`; it does not run `hours`. A stored `minutes=-120` fails validation.

| Operation | What runs | What it means |
| --- | --- | --- |
| Ordinary construction | Resolve supplied construction inputs through fieldmethods, then apply final validation | Establish stored fields from field-backed inputs and virtual construction inputs. |
| Walking or crumbling | Visit stored fields; apply the codec's encoding rules | Read the current state. Virtual construction inputs are absent from the stored shape. Encoding can fail its own type, resource or format checks. |
| Reconstruction | Decode stored fields, restore them through the model-owned hook, then apply final validation | Restore a valid instance without replaying construction fieldmethods. |
| Later mutation through ordinary permitted APIs | The behavior of the invoked operation | RFC 173 adds neither mutation permission nor a new automatic validation-on-assignment rule. |

Both construction and reconstruction can execute at runtime. Their difference follows from the inputs they consume: construction inputs versus stored fields. A codec does not store construction behavior and selectively replay it. Field-backed normalization is not replayed either: reconstruction restores the field's stored value, not a fresh input to its fieldmethod.

Validation checks must express requirements on final model state. A check that accepts or rejects a virtual construction input belongs to that input path; it is not a substitute for final-state validation during reconstruction. Authors must not put value replacement inside final validation to make restoration work. A legacy representation requiring conversion uses an explicit migration or custom reconstruction, with final validation retained.

Stored data carries state, not permission to execute construction fieldmethods, and not proof that the restored state is valid. Reconstruction must not return an unchecked instance even when the original value was validated before it was stored.

### Choose your own stored form

A type writes all three members to replace what the compiler fills. A class has none until it does, because a class is a live object until its author says what its data is:

```incan
from std.reflection import Reconstruct, ShapeInfo, Source, Walk, Walker

class Token with Walk, Reconstruct:
    pub text: str

    def __walk__[E, W with Walker[E]](self, mut walker: W) -> Result[None, E]:
        return walker.str(self.text)

    @staticmethod
    def __reconstruct__[E, S with Source[E]](mut source: S) -> Result[Token, E]:
        return Ok(Token(text=source.str()?))

    @staticmethod
    def __shape__() -> ShapeInfo:
        return ShapeInfo.of[str]()
```

### When a value cannot be walked

```incan
# Application-owned runtime resource; no walking protocol is supplied.
class LiveSession:
    session_id: int

model Pending:
    session: LiveSession
```

Calling `pending.crumble()` is refused at that use, naming `Pending.session` and its unsupported `LiveSession` type. This checks the model's reachable data contract through ordinary trait bounds; it is not runtime discovery of an unknown field type.

The declaration of `Pending` is fine. A model only needs the members where something reaches them.

## Reference-level explanation

### The traits

`std.primitives.reflection` owns these traits, also available through the `std.reflection` re-export:

```incan
pub trait Walk:
    def __walk__[E, W with Walker[E]](self, mut walker: W) -> Result[None, E]

    @staticmethod
    def __shape__() -> ShapeInfo

pub trait Reconstruct with Walk:
    @staticmethod
    def __reconstruct__[E, S with Source[E]](mut source: S) -> Result[Self, E]
```

### Which types have them

- `Walk` and `Reconstruct` must be automatic derives of `model`, `enum` and `newtype`, holding for a declaration when every field, payload or underlying type has them. They hold for a generic declaration at each instantiation whose type arguments do.
- `bool`, every integer and float type, `decimal`, `str`, `bytes`, `None`, `Option`, `Result`, `list`, `dict`, `set`, tuples, union types and their frozen forms must have them when their element types do.
- A `class`, a callable, a `rust::` type and a live runtime resource must not have them unless the type's author writes them. `SecretStr` and `SecretBytes` must not have them at all, so every format refuses a value holding one by the same rule (RFC 103).
- A declaration without the members must not be refused for that. A use is refused: a call, or a bound through a type parameter. The diagnostic must name the field path, show the field's declaration as a second location, and say that the type can write the members.
- A program that reaches none of these members must compile and run as if they did not exist.

### What the compiler fills

`__walk__` on a model must call `begin_model`, then `field` once for each stored field in ascending identity order, then `end_model`. On an enum it must call `begin_variant` with the value's variant, then `field` for each payload field, then `end_variant`. On a newtype it must walk the value it holds.

`__reconstruct__` must ask for the same stored fields in the same order. It must restore their values through model-owned reconstruction and enforce the applicable final field and whole-model validation obligations from RFC 122 before returning an instance. It must not replay input normalization or construction fieldmethods. These obligations apply recursively to nested values. A handwritten reconstruction must uphold the same validity contract.

- Each field is requested with its full declared type `V`. `source.field[V](info)` returns `Result[Option[V], E]`: the outer `None` means the field was absent, while `Some(value)` contains the stored value, including any optional layers inside `V`. Absence of an optional field resolves to the field value `None` under the format's compatibility contract. A present optional value must never be flattened or confused with absence.
- Any other absent field may use its declared default only when the source explicitly permits that default through `source.takes_default(info)`; otherwise it fails with `source.missing(info)`. RFC 165 Crumb permits an absent collection with a declared empty default, but rejects other absent required fields even if their current declarations have non-empty defaults. The protocol does not broaden that compatibility rule or run a default factory during shape inspection.
- A model validation failure must reach the caller through `source.invalid(error)`, which keeps the original `ValidationError`. A newtype restores its underlying stored value and enforces its constraints. Neither path may expose invalid final state.
- A supplied stored value must not be overwritten by a changed default. Missing-value handling follows the source and schema compatibility contract below; it must not guess constructor inputs from stored values.
- Virtual construction inputs are not stored fields and must not appear in the stored shape. A codec must diagnose or handle them under its explicit unknown-field policy, never invoke a same-named fieldmethod. Schema migrations must be explicit; reconstruction must not normalize an incompatible stored value into acceptance.
- Codec canonicalization under a declared value profile remains separate from model input normalization. Reconstruction does not disable the codec's documented representation rules.
- Error collection follows the source's explicit policy and the active validation policy. `source.collects()` selects whether independently decodable fields continue after a field failure. The source retains each located decode failure once, including a missing-field failure it creates, and `source.collected()` returns the resulting error when failures exist. The generated reconstruction retains availability separately from values: a failed field is not an absent optional field. It runs only validation checks whose required restored values are available, merges their original failures through `source.invalid(error)`, and never returns a partial instance. A structurally unrecoverable source stops with honest coverage. RFC 146's default CollectAll and explicitly selected FailFast must not be reset at this boundary.

### Visibility

The generated members cover every stored field, including fields without `pub`. A private field remains part of the model's complete data contract. Its presence must not disable walking or reconstruction, and must not require an explicit derive. The protocols are callable wherever the model's exported protocol is accessible; their visibility must not be reduced to that of the least visible field.

Ordinary callers still cannot initialize or assign private model fields directly. Model-owned reconstruction can populate them from stored data, subject to final validation. Direct field access retains its existing visibility rules. A visitor can observe private values through the walk: field privacy is not a confidentiality guarantee. RFC 103 secret protections apply independently, including to nested secrets.

`__shape__()` describes the complete stored shape used by this protocol. Existing public-field reflection helpers retain their own visibility contract. An intentionally redacted inspection view must not be described as a complete round-trip representation.

### What a walker sees

A walker sees kinds of value, and each kind has its members. A source answers with the matching reads, each returning `Result[..., E]`.

| Kind | `Walker[E]` members | `Source[E]` members |
| --- | --- | --- |
| `bool`, `str`, `bytes`, `decimal` | One member each, named for the kind and taking the value | One member each, named for the kind and returning the value |
| Integers | `integer(value: i128)`, and `unsigned(value: u128)` for `u128` | `integer()`, `unsigned()` |
| Floats | `float(value: f64)`; an `f32` arrives as the `f64` it equals | `float()` |
| `None` | `none()` | `none()`, returning `None` |
| `Option` | `none()`, `some[V with Walk](value: V)` | `some[V with Reconstruct]()`, returning `Option[V]`; nested optional layers remain nested |
| `list`, `set`, tuple | `begin_sequence(kind, length)`, `element[V with Walk](value: V)`, `end_sequence()` | `begin_sequence(kind)`, `element[V with Reconstruct]()` returning `Option[V]` with `None` past the last, `end_sequence()` |
| `dict` | `begin_dict(length)`, `entry[K with Walk, V with Walk](key: K, value: V)`, `end_dict()` | `begin_dict()`, `entry[K with Reconstruct, V with Reconstruct]()` returning `Option[tuple[K, V]]` with `None` past the last, `end_dict()` |
| Model | `begin_model(shape)`, `field[V with Walk](info, value: V)`, `end_model()` | `begin_model(shape)`, `field[V with Reconstruct](info)`, `end_model()` |
| Enum, including `Result` | `begin_variant(info)`, `field[V with Walk](info, value: V)`, `end_variant()` | `begin_variant(shape)` returning the value's `VariantInfo`, `field[V with Reconstruct](info)`, `end_variant()` |
| Union | `member(spelling)`, then the member's own value | `member()` returning the spelling, then the member's own value |
| Typed reference | `reference(kind, identity)`, written by the reference type's own `__walk__` | `reference(kind)` returning the identity, read by the reference type's own `__reconstruct__` |

- A newtype and an alias are invisible here: a walker sees what they hold. A frozen collection arrives as its collection.
- Every member must have a default in `Walker[E]`: a value member does nothing, and a structural member walks on into what it holds. A walker that overrides only `reference` finds every reference in a value, however nested, with no scan of bytes.
- `Source[E]` adds `missing(info)`, `takes_default(info)`, `invalid(error)`, `collects()` and `collected()`. `field[V with Reconstruct](info)` returns `Result[Option[V], E]`, `None` meaning the input lacks the field.
- Numeric decoding must preserve the requested type's value constraints. A narrower integer must refuse an out-of-range value through the source's typed failure channel; a float conversion must follow the selected codec value profile rather than silently choose a lossy conversion.
- Source implementations must expose typed failure conversion for malformed values, numeric range failures, and shape mismatches. End operations validate that the selected container or frame is complete under the format's contract. Tuple arity, enum variant selection, union membership and nested declared types are checked rather than guessed.
- Depth, sizes and allocation are the format's to bound. The generated recursion must carry the source or walker's accounting through nested values; calling a nested model must not reset its limits. The members add no arbitrary universal limit.

### Shape

`__shape__()` returns a `ShapeInfo`, without needing a value:

| Field | Type | Description |
| --- | --- | --- |
| `name` | `FrozenStr` | The type's name, for diagnostics |
| `identity` | `FrozenBytes` | An opaque identity of the checked model or enum declaration, distinct from its structural model hash; empty for other types |
| `spelling` | `FrozenStr` | The type's canonical spelling, with nested models and enums folded in, under *Record hashes* in RFC 165 |
| `fields` | `FrozenList[FieldInfo]` | Every stored field, in ascending identity order |
| `variants` | `FrozenList[VariantInfo]` | An enum's variants, in ascending identity order |

`FieldInfo` must gain these, and `__fields__()` must carry them too:

| Field | Type | Description |
| --- | --- | --- |
| `identity` | `FrozenBytes` | The field's identity under RFC 165 |
| `canonical` | `bool` | RFC 085's `canonical`, true when absent |
| `stored_type` | `FrozenStr` | The field's declared type in canonical spelling: aliases resolved, a newtype as what it holds, generic arguments concrete, a nested model or enum by its identity so that its own evolution does not move this text |
| `default_is_empty` | `bool` | Whether the declared default is an empty collection |
| `position` | `int` | The field's place in declaration order |
| `renamed_from` | `FrozenList[FrozenStr]` | The field's earlier names under RFC 085, earliest first, empty when it has none |
| `renamed_from_lifecycle` | `Option[LifecycleInfo]` | The lifecycle facts of a `renamed_from` block: optional `since`, `retire_in`, `message`, `by` and `issue` |

`VariantInfo` must gain `identity`, `renamed_from` and `renamed_from_lifecycle`, with the same meanings. `ShapeInfo.of[T]()` returns the same shape as `T.__shape__()` and supplies the explicit scalar representation used by the custom class example. The `identity` of a shape is not a digest-based claim that two unrelated declarations are interchangeable. A format that keeps declaration order sorts by `position`. `__fields__()` keeps its order and still leaves out what it leaves out today.

`stored_type` and `spelling` are specified text for formats to store and compare. They are not RFC 141's `TypeInfo`, and `type_name` remains display text. The compiler supplies checked field and variant identities under RFC 165 and diagnoses collisions within a declaration. This protocol introduces no replacement identity width or cross-revision compatibility policy. A format computes its model hash from `spelling` under its own named profile; structural spelling must use deterministic back-references for recursive model shapes, so recursion produces a finite description. Declared type identity and structural model hash remain distinct. A new codec hash profile needs no compiler change.

### Interaction with existing features

- **Derives:** writing `Walk` or `Reconstruct` in `@derive(...)` adds nothing, as for any automatic derive. A type that writes any of `__walk__`, `__reconstruct__` or `__shape__` must write all that its traits ask for, and the compiler fills none of them.
- **Generics:** a static member called through a bounded type parameter must reach the adopter's member, whether written or filled. `Self` must be valid as a call head wherever it is valid as a type, so `Self.__reconstruct__(source)` works in a type's own methods and in a trait default, where it names the adopter.
- **Borrowing:** each value reaches a walker as a duckborrowed value under RFC 164. Traversal itself must not require cloning or allocating a dynamic tree. A visitor retaining data beyond that borrow must obtain an owned value or an explicitly permitted longer-lived borrow. Its own output allocation and numeric representation conversions are separate from this traversal guarantee.
- **Field metadata:** `renamed_from` moves no identity and `alias` moves no `name`. `description` and other descriptive keys stay on `FieldInfo` and enter nothing else.

### Compatibility

The protocol adds new language hooks. A type that already declares a method named `__walk__`, `__reconstruct__` or `__shape__` with another signature is refused, and the double-underscore names are reserved for language hooks. `FieldInfo` gains fields, and its existing fields keep their meaning. Existing explicit construction or exhaustive patterns over that carrier may require migration; source compatibility must not be claimed merely because fields were added.

## Alternatives considered

- **Members for one format, filled by the compiler:** the compiler fills `encode`, `decode` and `schema` on a Crumb trait. Each further format needs another fill, and the first format's author still has a privilege nobody else has.
- **A built-in body for `from_crumb`, as `from_json` has:** smallest for one format, and exactly the shortcut RFC 165 forbids.
- **A dynamic value tree:** a walk that produces one `Value` per field needs no generic members. It allocates for every node of every record, and a hot store write would pay for that in every format.
- **A structural derive per format, where the trait names its own bound:** the format keeps its own bound and the compiler folds it over the fields. Every format then needs `int` and `list` to adopt its trait, and a third-party format cannot add that to a type it does not own. A shared `Walk` lets N formats meet M types through one contract.
- **Build on RFC 141's `TypeInfo`:** it describes a type and has no way to read a value or construct one, and it must not authorize a runtime-selected instantiation.
- **Disable external walking for a model with private fields, or require an explicit derive:** private fields are still part of the model's stored data contract. This would add ceremony or require duplicate export models for complete persistence. Ordinary field access stays restricted while the model-owned walk exposes the full stored state.
- **Reconstruct by replaying ordinary constructor inputs:** stored values have already undergone input transformations. Repeating them can change the value, so reconstruction restores final stored state and validates it instead.
- **Visit in declaration order:** the formats that need identity order are the ones with hot paths, and they would sort on every record. A format that wants declaration order sorts by `position`.

## Drawbacks

The kinds in `Walker[E]` become a public contract, and adding one means a walker sees it through a default rather than through a member it wrote. Each pair of a type and a walker it reaches is compiled separately, so a program with many models and many formats builds more code. The list of types that have the members is a list the language has to keep.

## Layers affected

- **Typechecker:** must treat `Walk` and `Reconstruct` as automatic derives that hold conditionally, must refuse a use with the field path, and must preserve ordinary field privacy while allowing the complete model-owned protocols. It must resolve a static trait member through a bounded type parameter, and must accept `Self` as a call head wherever it is valid as a type.
- **Lowering / IR emission:** must fill the members only where they are reached, and must preserve duckborrowing through the calls.
- **Stdlib:** the traits and carriers must be owned by `std.primitives.reflection` and re-exported as the same identities through `std.reflection`. Primitive codec consumers must not depend on higher standard packages to reach them.
- **Checked metadata:** identities, `canonical`, type spellings and `position` must cross package boundaries with the declaration.
- **Documentation:** the derive, reflection and `std.reflection` references must describe the members and the kinds.

## Inspectability and tooling surface

- **Artifact or metadata:** `T.__shape__()` is ordinary reachable data, and the same facts are in checked metadata.
- **Inspection command:** `incan check --format json` reports the refusals. An editor hover over a filled member shows its signature.
- **Diagnostics:** a refused use names the field path, shows the field's declaration and says the type can write the members.
- **Provenance:** a filled member is anchored at the declaration it was filled from.
- **Not implicit:** nothing runs unless a call or a bound reaches it, and a filled body is what the guide shows. No format is known to the compiler.

## Conformance

| Case | Must show |
| --- | --- |
| Library codec | A format written in a package of its own encodes and rebuilds nested models, enums, collections and optional values, with no compiler change. |
| Order and copies | A walk visits fields in identity order without cloning fields or building a dynamic value tree, counted across a large list field. Visitor-owned output copies are measured separately. |
| Reconstruction | Restore already-normalized values without replaying fieldmethods; run final validation and retain the original `ValidationError` through the source. A duration constructed through virtual input `hours=2` stores only `minutes=120`; reconstruction restores that field and validates it, while a stored `minutes=-120` fails. Test both a binary codec and a textual codec. |
| Collecting | Preserve active CollectAll and FailFast policies, retain each independent located failure once, distinguish unavailable from absent fields, report blocked validation, and never return a partial model. |
| Optional layers | Round trip `None`, `Some(None)` and `Some(Some(7))` through `Option[Option[int]]`; separately exercise a missing field. No failed decode becomes `None`. |
| Evolution and containers | Preserve renamed field identities and reordered fields; enforce Crumb's absent-value rules. Round trip nested payload enums, lists, tuples, dicts and newtypes; refuse invalid variants, lengths and out-of-range values with their paths. |
| Refusal | A field with no members is refused at a call and through a bound, naming the path, in nested, optional and collection positions. |
| Visibility | A public model with private fields round trips outside its defining context without an explicit derive; ordinary caller initialization and assignment of those fields remain rejected. A private stored value survives a later change to its declaration default. |
| By hand | A class with written members works through the same bounds as a model, and a model that writes `__walk__` without `__shape__` is refused. |
| Cost | A program that reaches no member compiles the same amount of code as one that declares no such members. |

## Design Decisions

- **One general pair:** one set of members for every format, because each format the compiler knew about would be another privilege and another path through it.
- **The shared contract is kinds of value:** a format implements the kinds once and meets every type through them, including types declared after it was written.
- **Identity order:** the hot formats need it, and it costs them nothing when the walk arrives that way.
- **Facts, not hashes:** the compiler gives identities and spellings, and formats hash them under their own profiles.
- **Models, enums and newtypes have it automatically; classes by hand:** a model is its data, and a class is a live object until its author says what its data is.
- **Private fields remain data:** automatic walking and model-owned reconstruction include them without an explicit derive. Ordinary field access, initialization and assignment remain governed by their existing visibility rules; secret protection is independent.
- **Reconstruction restores final state:** `__reconstruct__` replaces `__build__`. It restores stored values, including private fields, and enforces final validation without repeating input normalization or construction fieldmethods. Explicit migrations and codec value profiles retain their own roles.
- **Refusal at the use, not the declaration:** a model holding a task handle is a fine model until something tries to store it.
