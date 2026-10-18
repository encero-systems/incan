# RFC 143: Cloning with field overrides

- **Status:** Draft
- **Created:** 2026-10-11
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 024 (extensible derive protocol)
    - RFC 025 (trait-backed method dispatch)
    - RFC 033 (frozen, non-cloneable configuration contexts)
    - RFC 084 (callable presets)
    - RFC 085 (field metadata and fallible default factories)
    - RFC 087 (named validation and complete model contract composition)
    - RFC 122 (composable model construction)
    - RFC 128 (callable signatures and binding)
    - RFC 129 (shared bindings and explicit cloning)
    - RFC 132 (frozen classes)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Add a default, typed `clone` overload for field-backed models and classes: `config.clone(timeout=30)` first calls the receiver's zero-argument `clone()`, then constructs Self from that cloned baseline and selected replacements. The keyword overload preserves the construction result: `Self` for ordinary construction, or `Result[Self, E]` for validated construction. The existing zero-argument `Clone.clone() -> Self` contract remains intact, including handwritten copying behavior; authors may separately implement the keyword overload. Overrides follow checked construction and visibility rules, and a frozen class produces a completed frozen replacement without exposing writable fields. This is proposed behavior, not an implemented API.

## Motivation

Configuration and operation values often need a small change while retaining every other field. Restating their constructors is repetitive and can accidentally reset fields when a type evolves. Cloning followed by field assignment is unsuitable for frozen classes and exposes an unnecessary intermediate state.

The desired operation reads as a variation of copying: clone this value, replacing these named inputs. It belongs beside the existing `.clone()` operation and does not require a new keyword or a conversion through a dictionary.

## Goals

- Construct a replacement through `.clone(name=value, ...)` with checked names and types.
- Retain unspecified stored values from the zero-argument clone rather than reevaluating their defaults.
- Preserve the existing zero-argument `Clone` contract and generic callers.
- Preserve construction invariants, privacy, ownership, and RFC 132 freezing.
- Describe collisions and unsupported construction surfaces explicitly.

## Non-Goals

- General arbitrary method overloading or dynamically typed keyword overrides.
- Mutating the receiver, thawing an instance, or recursively freezing its contents.
- Cloning resolved ctx values: RFC 033 excludes whole-context cloning, even when its contained fields are individually cloneable. Ordinary contained model values and resource handles retain their own capabilities.
- Automatically adding replacement operations to handles, Rust types, enums, or newtypes.
- Defining nested paths such as `clone(database.timeout=30)`.
- Turning a callable preset into a type or replaying a source value's constructor history.

## Guide-level explanation

### Copy a value with a small change

```incan
model RequestConfig:
    endpoint: str
    timeout: int = 10
    attempts: int = 3

def example() -> None:
    original = RequestConfig(endpoint="https://example.com", attempts=5)
    copied = original.clone()
    updated = original.clone(timeout=30)

    assert original.timeout == 10
    assert updated.timeout == 30
    assert updated.attempts == 5
```

The default override overload invokes the original's zero-argument `clone()` and reconstructs `RequestConfig` from that copy plus the supplied changes. It does not reset `attempts` to its declaration default. Omitting an override and explicitly supplying `None` are different operations for an `Option` field; an implementation must not use a user-visible optional value as its omission sentinel.

```incan
updated = original.clone(timout=30)    # rejected: unknown override name
updated = original.clone(timeout="30") # rejected: incompatible value type
updated = original.clone(30)           # rejected: overrides are named
```

These are illustrative function-body fragments. All override examples describe proposed syntax and semantics.

### Reuse custom cloning by default

The zero-argument method and keyword overload are distinct trait-backed methods. The provided keyword implementation first invokes the selected zero-argument `clone()`, uses that returned value as its retained-state baseline, and constructs Self with the supplied checked overrides before final validation.

A library's handwritten zero-argument clone may, for example, create a fresh private cache. The default keyword overload must use that cloned state rather than bypass the method and copy the original cache. Merely implementing zero-argument cloning does not disable keyword overrides. An author who needs different keyword behavior supplies that overload separately; the authored implementation replaces its provided default.

The keyword inputs remain statically checked construction inputs. Applying them means constructing the replacement, not permitting unchecked casts, a runtime keyword dictionary, or post-construction assignment into a frozen clone. Exact reconstruction hooks remain subject to the construction contract below.

### Run construction hooks only for supplied inputs

RFC 122 fieldmethods transform supplied construction inputs into stored state. The keyword clone overload starts from the cloned baseline and runs only the hooks for explicitly supplied inputs, using RFC 122's canonical execution order. Unchanged normalized fields retain their stored values without being normalized again. The following application-defined model illustrates the proposed interaction:

```incan
model Duration:
    days: int = 0
    minutes: int = 0

    @fieldmethod(days, minutes)
    def hours(mod, value: int):
        total_minutes = mod.minutes + value * 60
        mod.days += total_minutes // 1440
        mod.minutes = total_minutes % 1440

def duration_example() -> None:
    original = Duration(hours=23)
    extended = original.clone(hours=2)

    assert original.days == 0
    assert original.minutes == 1380
    assert extended.days == 1
    assert extended.minutes == 60
```

Here `hours=2` is a virtual construction input whose hook contributes to the copied duration and updates both declared targets. The operation does not replay the original `hours=23` input or retain a constructor recipe. Recalculation follows declared hooks and the supported finalization path; reading a field in some other method does not create an automatic dependency or cause that method to rerun. Complete final validation still applies to the replacement.

### Preserve recoverable validation failures

Registered checks make canonical model construction return `Result[Self, ValidationError]`. Keyword cloning must preserve that result because supplied changes can invalidate an otherwise valid value. All declarations below are application-defined; error declarations, registered checks, and the keyword clone overload are proposed. ValidationError is RFC 131's shared umbrella, whose final stdlib module placement remains open:

```incan
error PortError:
    OutOfRange:
        value: int

@derive(Clone)
model Port:
    value: int

    @validate(value)
    def in_range(value: int) -> Result[None, PortError]:
        if value < 1 or value > 65535:
            return Err(PortError.OutOfRange(value=value))
        return Ok(None)

def change_port(original: Port) -> Result[Port, ValidationError]:
    copied = original.clone()  # Port: the zero-argument copying contract.
    return copied.clone(value=70000)  # Validate the replacement.
```

`change_port` returns ValidationError retaining the original `PortError.OutOfRange` rather than exposing an invalid Port. Callers can use ordinary matching or `?` to handle the result. Zero-argument cloning keeps returning Self; the keyword overload preserves the selected canonical construction result. The distinction is statically determined by the checked overload and construction contract, not by whether particular values happen to pass validation. RFC 122 replaces legacy Validate registration through an explicit breaking migration rather than an automatic adapter.

### A frozen replacement is frozen from construction

```incan
@derive(Frozen)
class InputSpec:
    pub label: str
    pub attempts: int

def frozen_example() -> None:
    original = InputSpec(label="atoms", attempts=2)
    updated = original.clone(attempts=3)

    assert original.attempts == 2
    assert updated.attempts == 3
    updated.attempts = 4  # rejected: frozen field
```

The operation returns another complete `InputSpec`. Its default implementation obtains a clone as the baseline, then constructs the replacement with the overrides; it must not write into the completed baseline's frozen fields. A mutable binding can still be replaced with the returned value under RFC 129; it cannot make either instance's fields writable.

Shallow freezing retains its meaning. An interior mutable handle may retain shared state according to its own clone contract. This operation promises neither recursive independence nor a stable snapshot of that state.

### Private state remains private

Only names exposed by the type's checked construction surface are eligible at the call site. A readable field is not automatically a replaceable construction input. The operation must not expose private state, computed properties, methods, or type-owned constants as additional override names.

The generated reconstruction can retain private stored fields internally without making them public. Ordinary factory methods have no automatic role in cloning: their existence does not make them mandatory construction paths or add their parameters to the override surface. Authors who want a particular factory to participate implement the keyword overload explicitly, while existing enforced conversion and validation contracts remain in force. Unsupported default reconstruction must be diagnosed, rather than silently falling back to raw field writes.

## Reference-level explanation

### Availability and dispatch

1. Eligible field-backed models and classes must expose a default trait-backed instance overload accepting one or more named overrides and preserving the construction result: Self for ordinary construction, or `Result[Self, E]` for validated construction. The successful value must preserve the receiver's concrete type and generic arguments. A handwritten zero-argument `clone()` must not by itself disable this overload.
2. A call with no supplied overrides must select the existing zero-argument `clone()` operation. The generated override overload must not compete for that call, even if every field can be omitted.
3. Override names and their accepted types must come from checked construction metadata. Unknown names, positional overrides, incompatible arguments, and duplicate explicitly supplied names must be rejected under the normal call rules.
4. Generic code bounded only by `Clone` must retain access to zero-argument `clone()` only. Field overrides require a statically known replacement surface; this RFC introduces no structural field capability on `Clone`.
5. The zero-argument and keyword overloads must participate in checked trait-backed method dispatch as distinct callable contracts across imports and aliases. This must not authorize unrelated duplicate methods.
6. The default keyword overload must call the selected zero-argument `clone()` implementation, including a handwritten one. Authors may implement the corresponding keyword overload separately, replacing its provided default under the ordinary trait implementation rules. Implementing only the zero-argument method must leave the default keyword overload available. Duplicate or ambiguous authored contracts must receive normal diagnostics; an intentional implementation of the keyword overload must not be rejected merely because a generated default exists.

### Reconstruction and validation

1. The receiver and supplied override expressions must be evaluated once, in the language's normal call evaluation order.
2. The default keyword overload must invoke the receiver's selected zero-argument `clone()` exactly once and use its returned Self as the reconstruction baseline. Each stored field not replaced by a supplied input or affected by declared construction hooks must contribute the cloned baseline's value under ordinary ownership rules, not the original receiver's raw field value. Supplied replacements must use ordinary argument ownership and checked conversion rules. A hook's affected targets must come from checked construction metadata, not be guessed from field names. The default must not bypass authored copying behavior or its observable effects merely because some cloned fields will subsequently be overridden.
3. Omitted inputs must not reevaluate field defaults; they retain the cloned baseline's stored state. Reconstruction must not mutate the original's or completed baseline's field slots, or consume the receiver as an implicit move.
4. Supplied overrides must pass the same type checking, permitted checked conversions, and visibility checks as the corresponding construction inputs. This RFC must not introduce implicit parsing or primitive widening.
5. Construction must validate the complete resulting state through the type's supported finalization path before exposing a successful value. If canonical validated construction returns `Result[Self, E]`, the keyword overload must also return `Result[Self, E]`, preserving the validator's success value and typed failure without unwrapping, panic conversion, text conversion, or a nested `Result`. Ordinary construction must retain a Self result, and zero-argument `Clone.clone()` must retain its Self result in both cases. The selected construction contract must determine the keyword overload's static result type; validation success or failure must not change that type at runtime.
6. Retained stored values from the cloned baseline must not be mistaken for newly supplied raw constructor inputs. Under RFC 122, reconstruction must seed pending stored state from the baseline, then invoke only the effective fieldmethods for explicitly supplied inputs, exactly once each in RFC 122's canonical order. Outside explicitly resolved composed pipelines, field-backed inputs run in field declaration order, followed by computed inputs in computed fieldmethod declaration order; an explicit pipeline preserves its declared internal order and input bindings. Authored field-backed hooks transform their supplied values; computed hooks may read and update pending state under RFC 122's existing read and exact write-target rules. Their declared targets may replace retained baseline values. Other stored fields must retain their baseline values, and omitted normalized fields must not be normalized again. Reconstruction must not replay original construction inputs, infer dependencies from arbitrary method bodies, or rerun omitted fieldmethods solely because their inputs may once have contributed to stored state. Supported finalization and complete final validation must still run as specified above.
7. Failure must not expose a partially initialized replacement or edit the receiver. Existing permitted effects of validators are not rolled back by this guarantee.
8. Computed properties and type-owned constants must not be copied as stored instance fields. Supported inherited stored fields must retain their declaring identities and access restrictions.
9. The default keyword overload must use the type's checked construction and finalization contract. It must not select ordinary factory methods by name, infer mandatory factory use from their existence, replay the factory that originally produced the receiver, or add factory-only parameters to its override surface. Authors may implement the keyword overload explicitly to use such a factory. If the default reconstruction cannot satisfy the checked contract, the compiler must diagnose the unsupported path, explain the unmet requirement, and identify an authored keyword overload as the customization point. A handwritten zero-argument clone or an ordinary factory alone must not cause that rejection.

RFC 087 and RFC 122's explicit single-field normalize pipelines follow the same supplied-input rule. For an Account whose name pipeline trims and then lowercases, `account.clone(minimum=2)` retains its stored normalized name without replaying either step. `account.clone(name=" BOB ")` runs the selected name pipeline once on the new input, then validates the complete resulting model. The pipeline's internal order is the explicit normalize list, while its activation belongs to the field-backed construction slot. Retained validation obligations remain mandatory in either case. RFC 122 additionally selects named-input binding for computed entries and resulting-target-value binding for following field-backed entries in explicit pipelines. Clone integration must preserve those bindings without replaying original inputs. Competing newly supplied inputs to the same composed pipeline step must receive the same diagnostic as direct construction. Retained baseline values are not newly supplied inputs, so an existing minutes field must not conflict with a newly supplied total_minutes input merely because the computed operation replaces it. Activation with an omitted virtual input and coordination of overlapping pipelines remain under review in RFC 122.

### Frozen classes and models

1. Every replacement of an RFC 132 frozen class or model must retain that same frozen declaration property, including through generic instantiations, factories, imports, and aliases.
2. Construction must establish all fields before exposing the replacement. No authored or generated post-construction field assignment may be used to implement overrides.
3. The receiver and returned instance must reject the same field replacement and writable-alias operations. Overrides must not introduce a thawed intermediate value available to authored code.
4. Contained values must retain their declared types and independent capabilities. The operation must not implicitly convert `list[T]` to `FrozenList[T]`, imply deep freezing, or grant new mutation permissions.
5. Existing class inheritance and privacy rules must remain in force. Frozen inheritance restrictions must not be bypassed by reconstructing through a base-class view.

## Design details

### A typed overload, not a keyword dictionary

The authoring surface is an overload of `clone`, but its parameters are derived from a checked nominal type. Tooling can therefore complete names, explain accepted types, and reject misspellings before execution. A dictionary with runtime-selected keys is not an unchecked escape hatch. Any accepted keyword spread must already prove its names and types under ordinary call rules.

The overload's successful value has the same concrete type as the receiver, with the construction contract determining whether the result is Self or `Result[Self, E]`. It does not select a base class or infer a different generic instantiation from the overrides. RFC 128 callable values must preserve overload identity and result type; taking a bound method value must use the existing checked signature-selection rules rather than guessing between copy and replacement.

### Existing construction proposals

RFC 122 constructor `.new` aliases perform fresh canonical construction. The type call selects a classmethod and the instance call selects a distinct instance method; neither uses the receiver's stored fields as defaults. In contrast, keyword cloning begins with the selected zero-argument clone and preserves its baseline fields except where supplied inputs and declared hooks replace them. An instance `.new(...)` call is not shorthand for clone-with-overrides.

RFC 084 presets supply constructor defaults before an instance exists. This RFC first obtains a copy through zero-argument cloning, then derives the replacement from that copied state and the supplied overrides. RFC 122 fieldmethods operate on pending model construction, so any reuse must operate before the replacement becomes a completed value. This proposal does not call fieldmethods on an existing model.

RFC 085 and RFC 087 registered field and named model checks also require canonical `Result[Self, ValidationError]` construction without an extra Validate derive. Keyword clone finalization must honor every retained check, including source-contract obligations from a model's from clause, while preserving original typed failures. Zero-argument Clone remains Self. Validation policy belongs to RFC 122's runtime execution scope, not the model or its cloned baseline. Keyword clone finalization inherits the active runtime policy through ordinary helper calls and must preserve the canonical checked result under that policy; zero-argument cloning does not become a new validation pass. Remaining task/suspension integration, execution, and the settled breaking migration from legacy Validate registration are governed by RFC 122.

RFC 085 permits fallible default factories and makes that model's canonical construction return `Result[Self, ValidationError]`, including without deriving Validate. The keyword clone overload must preserve that checked Result type. Values retained from the cloned baseline are already supplied stored state, so their default factories must not run again. The presence of a fallible default factory does not make zero-argument Clone fallible or add nested Results. RFC 131 and RFC 146 retain the same typed validation umbrella; legacy Validate declarations and callers require the explicit migration specified by RFC 122.

Eligibility must be advertised consistently by providers. A consumer must not see a replacement overload whose private state or construction invariants cannot be reconstructed through the supported checked boundary.

### Prior art

Python's [dataclass replacement](https://docs.python.org/3/library/dataclasses.html#dataclasses.replace) constructs a new instance with selected changes and invokes initialization. Koheesio's [`from_basemodel`](https://engineering.nike.com/koheesio/0.10.0/api_reference/models/index.html#koheesio.models.BaseModel.from_basemodel) combines an existing model's data with overrides before constructing a replacement. Both illustrate reconstruction with changes; Incan would preserve nominal field typing, ownership, privacy, and compiler-enforced freezing without requiring a dictionary conversion.

## Alternatives considered

- **A separate `.replace(...)` method:** distinguishes copying from reconstruction explicitly, but adds a second operation name for the small variation this proposal places beside `.clone()`.
- **Clone, then assign fields:** works only where mutation is permitted, exposes intermediate state, and violates RFC 132 for frozen fields.
- **Restate the constructor:** remains valid, but repeats unchanged fields and can accidentally restore defaults.
- **Convert through a dictionary:** obscures checked field identities and can bypass intended construction boundaries.
- **Extend `Clone` with arbitrary keyword arguments:** makes a nominal copy trait depend on unrelated field sets and weakens generic signatures.

## Drawbacks

The name `clone` now covers both the existing copying contract and validated reconstruction with changes. Documentation must explain which operation executes and why a validated type's keyword overload returns `Result[Self, E]` while its zero-argument clone returns Self. Adding or removing an eligible public construction input also changes the override overload's public API.

The default overload invokes whole-value cloning even when an override replaces some of the copied fields. Avoiding that extra copying must not bypass a handwritten clone or change observable copying behavior. Derived state and constructor hooks still make automatic reconstruction eligibility less trivial than the call syntax. A type that cannot preserve its invariants through checked reconstruction must not receive a misleading overload. Frozen classes add no shortcut around those concerns.

## Layers affected

- **Typechecker / symbol resolution:** generated overload identities, checked input names and types, visibility, eligibility, and Frozen enforcement.
- **Construction / lowering / emission:** retained field cloning, supplied overrides, validation, and completed reconstruction without post-construction writes.
- **Providers / API metadata:** replacement signatures, inherited field identities, eligibility, and preserved frozen properties.
- **Stdlib:** the relationship between generated replacement operations and the existing `Clone` trait.
- **LSP / documentation:** override completion, diagnostics, and explanation of copy versus replacement behavior.

## Design Decisions

- The default keyword clone overload calls the selected zero-argument clone exactly once, including a handwritten implementation, then constructs and validates Self from that cloned baseline plus checked overrides. A custom plain clone does not disable the keyword overload; authors may implement the keyword overload separately through the trait method contract.
- Zero-argument calls select the existing Clone operation, while override calls select the checked keyword overload. Reconstruction preserves freezing by constructing a complete replacement rather than assigning to the completed clone's fields. Retained values come from the cloned baseline, and optimizations must not bypass authored copying behavior.
- RFC 122 fieldmethods run only for explicitly supplied construction inputs, once each in the existing canonical order, against pending state seeded from the cloned baseline. Declared hook targets update together before final validation; other fields retain their stored state. There is no replay of constructor history or inferred dependency tracking.
- Keyword cloning preserves the canonical construction result: Self for ordinary construction and `Result[Self, E]` for validated construction, with the original error type and no additional result nesting. Zero-argument cloning retains its existing Self result. The distinction belongs to the checked overload signature, not runtime-selected behavior.
- Ordinary factory methods are not automatically selected or replayed by cloning and do not extend the override surface. The default uses checked construction and finalization; authors needing factory-specific behavior implement the keyword overload explicitly. Unsupported default reconstruction receives a diagnostic explaining the unmet construction requirement and the customization point.
