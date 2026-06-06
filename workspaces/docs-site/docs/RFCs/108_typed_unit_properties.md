# RFC 108: Typed unit properties

- **Status:** Draft
- **Created:** 2026-06-06
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 027 (`incan-vocab` block registration and desugaring)
    - RFC 040 (scoped DSL surface forms)
    - RFC 045 (scoped DSL symbol surfaces)
    - RFC 046 (computed properties)
    - RFC 058 (`std.datetime` temporal values, intervals, and runtime timing)
    - RFC 091 (constrained integer newtype storage carriers)
    - RFC 107 (type-directed library APIs and compile-time type tokens)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC proposes import-scoped typed unit properties for primitive numeric receivers, plus the library-authoring descriptor contract needed to build them. A library should be able to expose source forms such as `3.days`, `12.hours`, `256.mb`, `16.px`, and `5.percent` as checked unit values without globally patching primitive types or relying on dynamic member lookup.

## Core model

1. **Unit properties are checked vocabulary:** a library may expose property-like unit names for primitive receivers through explicit import or vocabulary activation.
2. **Primitive receivers stay closed by default:** `3.days` is valid only when an active unit-property descriptor declares that `days` applies to plain integer receivers.
3. **Libraries author descriptors, not compiler patches:** a unit vocabulary is built from ordinary value types and constructor functions plus exported descriptor metadata that maps receiver/property pairs to those constructors.
4. **Properties produce typed values:** a unit property constructs a first-class typed value such as a civil interval, byte size, layout length, percentage, money amount, or domain-specific scalar wrapper.
5. **Composition is typed by unit family:** tuples such as `(3.days, 12.hours)` may participate in aggregate properties only when all elements satisfy a shared unit-family contract.
6. **Runtime anchors are visible:** convenience properties such as `.from_now` and `.ago` may be provided by temporal libraries, but they are runtime capability boundaries and must not be confused with pure unit construction.
7. **No runtime monkey patching:** importing a unit vocabulary does not mutate primitive types globally; lookup remains static, scoped, diagnosable, and visible to tooling.

## Motivation

Incan already has computed properties, typed numeric values, newtypes, temporal intervals, and vocabulary-driven DSL surfaces. What it lacks is a library-authorable way to make primitive numeric literals participate in typed unit member syntax. Constructor calls such as `TimeDelta.days(3) + TimeDelta.hours(12)` are clear and should remain valid, but they are heavier than the value being described. `3.days` is short, reads naturally, and still has a precise type if the compiler resolves it through an imported descriptor rather than runtime member lookup.

This matters beyond temporal code. Byte sizes, percentages, basis points, layout lengths, physical units, money amounts, experiment probabilities, and policy thresholds all benefit from source-level units. A raw integer such as `256 * 1024 * 1024` loses meaning in review. A typed value such as `256.mb` can carry unit identity, display behavior, overflow rules, conversion policy, and schema metadata.

The design must not reopen the dynamic-language tradeoff that Incan deliberately avoids. This RFC is not a general primitive extension mechanism and not a way to attach arbitrary nouns to numbers. The goal is typed scalar authoring with strong static boundaries.

The new surface is not "how to use Incan creatively today." Today a library can offer constructor functions, newtypes, ordinary methods on its own types, and vocab blocks. It cannot safely teach the compiler that an imported library owns `days` as a property on plain integer literals, cannot make that property visible to completion/hover/diagnostics as a typed descriptor, and cannot define aggregate tuple properties such as `(3.days, 12.hours).from_now` without either ordinary helper calls or broader parser/typechecker support. Those descriptor-backed primitive and aggregate property lookups are the new language surface.

### Prior art: Ruby and Rails Active Support

Ruby on Rails' Active Support is direct prior art for the motivating temporal shape. Active Support exposes duration helpers on Ruby numeric values and documents examples such as `1.month.ago`, with duration values supporting methods such as `ago` and `from_now` in the Rails API docs: https://api.rubyonrails.org/classes/ActiveSupport/Duration.html.

The Incan design should give credit for the ergonomic insight without copying the mechanism. Rails achieves this style by extending Ruby's core numeric surface through a dynamic runtime. Incan should instead make the extension explicit, import-scoped, typed, descriptor-backed, and inspectable. The lesson to preserve is that `3.days` reads like a domain value; the lesson to avoid is that primitive behavior changes ambiently through open-class/runtime patching.

## Goals

- Allow imported libraries to expose property-like unit names on primitive numeric receivers.
- Define the minimum authoring contract for a library to export unit-property descriptors.
- Keep unit property activation explicit, scoped, static, and inspectable.
- Make unit properties produce ordinary typed Incan values.
- Make the mechanism general enough for unit families such as temporal intervals, byte sizes, layout lengths, percentages, and currency amounts without requiring core-language special cases for each family.
- Allow typed aggregate properties over homogeneous unit-family tuples, such as `(3.days, 12.hours).from_now`.
- Distinguish pure unit construction from runtime anchored conveniences such as `.from_now`, `.ago`, or similar clock-reading properties.
- Provide diagnostics for missing imports, ambiguous active unit descriptors, invalid receiver types, unsupported const contexts, and unsafe runtime anchors.

## Non-Goals

- This RFC does not allow arbitrary monkey patching of `int`, `float`, `str`, or other primitives.
- This RFC does not define a full dimensional-analysis type system.
- This RFC does not define every standard unit family or decide which unit modules ship first.
- This RFC does not commit the standard library to build temporal, data-size, layout, finance, science, or percentage unit modules as part of this RFC. Those names are examples and pressure tests for the mechanism.
- This RFC does not allow unit properties to run arbitrary runtime code unless a descriptor marks the property as a runtime anchor and the context permits that behavior.
- This RFC does not make external unit libraries globally active through dependency installation alone.
- This RFC does not introduce setter properties or mutable primitive extension state.
- This RFC does not make unit names valid in ordinary member lookup without an active descriptor.

## Guide-level explanation

A temporal library can expose readable interval construction:

```incan
from std.datetime.units import temporal_units

deadline = (3.days, 12.hours).from_now
reminder = 15.minutes.before(deadline)
retry_window = 250.milliseconds
```

The important point is not that `days` became a real field on every integer forever. The imported `temporal_units` surface activates a checked descriptor that says `days`, `hours`, `minutes`, `seconds`, and related names are unit properties for plain integer receivers in this module. Without that import, `3.days` is rejected with a diagnostic that can suggest the intended activation when the compiler recognizes the unit name.

Data-size units use the same mechanism:

```incan
from std.data.units import byte_units

upload_limit = 256.mb
chunk_size = 64.kib
cache_budget = 2.gib
```

Visual and layout libraries can use unit properties without forcing authors into raw CSS strings:

```incan
from std.ui.units import layout_units

card_gap = 16.px
content_width = 42.rem
fade_duration = 200.ms
disabled_opacity = 40.percent
```

Finance and analytics code can make rates explicit:

```incan
from std.finance.units import rate_units

fee_rate = 2.5.percent
spread = 120.bps
sample_rate = 0.1.ratio
```

The feature should stay focused on units and typed scalar wrappers. Names such as `3.days`, `256.mb`, and `5.percent` are strong examples because they attach a unit to a scalar. Names such as `3.users`, `404.not_found`, or `7.retry_policy` should not be accepted merely because a library wants cute syntax; those are better modeled as explicit constructors, enums, or domain functions unless a future RFC defines a more specific safe surface.

### Building a unit vocabulary

From the library author's perspective, a unit vocabulary has two layers:

- ordinary Incan value types and helper functions that own the behavior;
- descriptor metadata that makes selected primitive property spellings resolve to those helpers when the vocabulary is active.

The descriptor syntax below is illustrative, not final grammar. The required contract is the important part:

```incan
unit_vocab temporal_units:
    family TemporalAmount:
        type TimeDelta

    property days on int -> TimeDelta:
        constructor = TimeDelta.days
        family = TemporalAmount
        eval = pure
        const = true

    property hours on int -> TimeDelta:
        constructor = TimeDelta.hours
        family = TemporalAmount
        eval = pure
        const = true

    aggregate property from_now on tuple[TemporalAmount] -> DateTime:
        constructor = temporal_from_now
        eval = runtime_anchor(clock)
        const = false
```

The equivalent descriptor may be produced through a vocab companion crate, an Incan-native descriptor declaration, generated package metadata, or another mechanism accepted by the library system. Regardless of spelling, the compiler-facing facts are the same: `days` on `int` lowers to `TimeDelta.days(receiver)`, `hours` on `int` lowers to `TimeDelta.hours(receiver)`, both values belong to the `TemporalAmount` family, and `.from_now` on a tuple of temporal amounts lowers to a runtime anchored helper.

This is what distinguishes the feature from ordinary methods. The library is not adding fields to `int`. It is exporting a scoped descriptor that the compiler can typecheck, lower, inspect, document, and diagnose.

## Reference-level explanation

### Library authoring contract

A unit-property vocabulary must be built from ordinary typed behavior plus descriptor metadata. The behavior layer defines the value types and helper functions. The descriptor layer tells the compiler which primitive receiver/property spellings are eligible, what type they produce, and what helper performs the construction.

A unit vocabulary must provide, directly or through generated metadata:

- a stable vocabulary identity;
- one or more unit families when aggregate behavior is needed;
- one descriptor per primitive unit property;
- the receiver type family for each property;
- the result type for each property;
- the typed constructor or helper target for each property;
- the evaluation classification for each property: pure construction or runtime anchor;
- const-evaluation eligibility for each property;
- optional aggregate property descriptors for tuple or other aggregate receivers;
- source spans or package metadata sufficient for diagnostics, hover, and inspect output.

The authoring mechanism may reuse the RFC 027 vocabulary registration model, a future Incan-native descriptor syntax, or package-generated metadata. This RFC does not require one exact authoring syntax, but any accepted syntax must preserve the descriptor facts above. A unit-property implementation that relies on source-string matching, runtime `getattr`-style lookup, or generated-Rust member names does not satisfy this RFC.

For a property descriptor, the helper target must be typecheckable as if the receiver were passed explicitly. For example, `3.days` must have an equivalent typed helper shape such as `TimeDelta.days(3)` or `days(3)`. The descriptor must not hide additional untyped arguments or infer behavior from the property name alone.

For an aggregate descriptor, the helper target must be typecheckable from the aggregate receiver and any explicit context required by the descriptor. For example, `(3.days, 12.hours).from_now` must lower to a helper that accepts an aggregate temporal amount or a normalized temporal amount value. The compiler must not invent the aggregation algorithm from the property names.

### Unit property activation

A unit property is active only when the current module or lexical scope explicitly imports or activates a unit-property descriptor. Activation must be visible in source and must not occur merely because a package is installed or a dependency is present.

A unit-property descriptor must define:

- the property name;
- the receiver type family, such as `int`, `float`, `decimal`, or another explicitly supported primitive;
- the result type;
- whether the property is pure unit construction or a runtime anchor;
- whether the property is const-evaluable;
- the owning module or package identity used for diagnostics and tooling;
- any conflicts or aliases that the owning library declares.

### Receiver eligibility

The receiver expression for a unit property must typecheck as one of the primitive receiver forms declared by the active descriptor. A descriptor may limit a unit to plain integer receivers, decimal receivers, floating-point receivers, or another explicit primitive subset. A descriptor must not silently accept every numeric-looking type.

For example, `3.days` may be valid when `days` is declared for `int`, while `3.5.days` may be rejected unless the temporal unit library explicitly declares fractional-day support. If a numeric literal has not yet been assigned a concrete type, ordinary contextual typing may use the unit descriptor as context, but the final receiver type must still satisfy the descriptor.

### Lookup and ambiguity

Unit property lookup must be static. If no active descriptor declares the property for the receiver type, the compiler must reject the access. If more than one active descriptor declares the same property name for the same receiver type and neither descriptor defines an explicit disambiguation rule, the compiler must reject the access as ambiguous.

Ordinary stored fields, methods, and computed properties on non-primitive user-defined types remain governed by normal member lookup. This RFC only adds a descriptor-backed lookup lane for primitive receiver unit properties and explicitly supported unit-family aggregate receivers.

### Unit values

A unit property returns an ordinary typed value. That value may be a model, newtype, enum, or other Incan type. The returned value should preserve enough unit identity for display, conversion, schema metadata, and diagnostics where those surfaces are relevant.

A unit property must not lower to an untyped number followed by ad hoc interpretation in a downstream library. Later code should be able to distinguish `256.mb` from `256.bytes` and `5.percent` from `5.ratio` by type, metadata, or both.

### Tuple aggregate properties

A unit library may declare aggregate properties for tuples whose elements all satisfy a shared unit-family contract. For example, a temporal unit library may define `.from_now` on a tuple of temporal amounts:

```incan
deadline = (3.days, 12.hours).from_now
```

The compiler must typecheck each tuple element before resolving the aggregate property. If a tuple mixes incompatible unit families, the aggregate property must be rejected with a diagnostic naming the incompatible element or family.

Aggregate properties should lower through ordinary typed helper functions or methods owned by the unit library. The compiler must not concatenate source text or infer aggregate behavior by property-name convention alone.

### Pure construction and runtime anchors

Pure unit properties construct typed values from source-visible scalar inputs. Examples include `3.days`, `256.mb`, `16.px`, and `5.percent` when their constructors are deterministic and context-independent.

Runtime anchored properties read or depend on runtime state. Examples include `.from_now`, `.ago`, and any property that consults the current clock, environment, locale, exchange-rate source, policy context, or similar ambient input.

Runtime anchored properties must not be const-evaluable. They must be visible to inspection and diagnostics as runtime capability use. Libraries that provide runtime anchored convenience properties should also provide an explicit-context spelling suitable for tests and governed runtimes, such as `duration.after(clock.now())`, `duration.before(anchor)`, or another library-owned equivalent.

### Const contexts

A pure unit property may be valid in a `const` initializer only when the descriptor marks it const-evaluable and every required conversion is permitted in const context. A runtime anchored property must be rejected in const context.

### Diagnostics

The compiler must diagnose:

- missing unit-property activation;
- unknown unit property for the receiver type;
- ambiguous active descriptors;
- receiver type mismatch;
- invalid use of runtime anchored properties in const or compile-time-only contexts;
- invalid aggregate property use on tuples with incompatible unit families;
- unit property names that collide with ordinary member lookup in a way the descriptor model cannot resolve safely.

Diagnostics should name the unit property, receiver type, owning descriptor when known, and suggested import when a known standard unit vocabulary is missing.

## Design details

### Syntax

This RFC uses ordinary property access syntax:

```incan
3.days
256.mb
(3.days, 12.hours).from_now
```

No new token is required for the use-site syntax. The new behavior is descriptor-backed member resolution for primitive receiver property names when an owning unit vocabulary is active.

### Semantics

Reading a unit property evaluates the receiver once, validates the receiver type against the active descriptor, and constructs the descriptor's result type. Reading an aggregate property evaluates the aggregate receiver once, validates the aggregate contract, and calls the owning library's typed aggregate operation.

### Interaction with computed properties

RFC 046 computed properties define field-like member reads on user-defined types. This RFC builds on the same user-facing access shape but extends lookup to descriptor-backed primitive receivers. Unit properties are not declared inside the primitive type body and do not imply that primitive types are open for arbitrary user mutation.

### Interaction with vocabulary surfaces

Unit property descriptors fit the same scoped ownership direction as RFC 027, RFC 040, and RFC 045. A library owns a vocabulary surface, activation is explicit, and tooling can expose the active surface. Unlike block-shaped DSLs, unit properties appear inside ordinary expressions, so ambiguity and receiver eligibility rules must be stricter.

### Interaction with temporal values

Temporal units are a motivating example, not a commitment in this RFC to build a specific standard temporal unit module. If a temporal unit library exists, it should distinguish elapsed runtime durations from civil calendar intervals. It may expose both fixed elapsed units and civil day/time units, but the return type must make the difference visible. Months and years should not be silently treated as fixed-length day counts.

### Interaction with money

Currency unit properties such as `12.eur` and `500.usd` are examples that test the model, not a commitment in this RFC to build a standard money module. A currency unit property may construct a typed money amount, but cross-currency arithmetic, exchange-rate conversion, rounding, and valuation date policy are outside this RFC and should remain explicit in any money library.

### Standard library examples

The standard-library-looking modules in this RFC are examples and pressure tests. They show that the mechanism should be able to support temporal, data-size, layout, finance, scientific, or percentage units without new compiler special cases for each family. They are not a delivery commitment. Each standard unit family, if pursued, should still have its own RFC, issue, or stdlib design note defining its value types, semantics, overflow behavior, const behavior, display, parsing, and runtime capabilities.

### Compatibility and migration

This RFC is additive. Existing code that does not activate a unit-property descriptor is unaffected. Constructor-style APIs such as `TimeDelta.days(3)` remain valid and should continue to be documented as explicit alternatives where they are clearer or where import-scoped property activation is not desired.

## Alternatives considered

1. **Constructor functions only.** This preserves the current explicit style but leaves common scalar units noisier than necessary and encourages raw numeric values in source.
2. **Global primitive extension methods.** This gives the nicest use-site syntax but reintroduces monkey-patching hazards and makes primitive behavior depend on dependency graph accidents.
3. **General extension properties for all types.** This is broader than the unit problem and would require a larger coherence, conflict, and discoverability design before it is safe.
4. **String-based units such as `unit(3, "days")`.** This is easy to implement but loses checked names, completion, docs, refactoring, and source-level type evidence.
5. **A full dimensional-analysis language feature first.** This may be valuable later, but typed unit properties solve the application-authoring problem without requiring the compiler to own every algebraic unit rule.

## Drawbacks

- Primitive member lookup becomes more complex because some property names may be descriptor-backed.
- Import-scoped syntax can make source less obvious if tooling does not surface active unit vocabularies clearly.
- Unit libraries may be tempted to overuse the feature for nouns that are not scalar units.
- Runtime anchored properties such as `.from_now` are convenient but require careful docs and diagnostics so they do not hide clock or policy dependencies.
- Ambiguity between standard units and domain units must be rejected clearly rather than resolved by import order.

## Layers affected

- **Parser / AST:** no new use-site syntax is required, but the parser must preserve property-access spans precisely enough for descriptor-backed diagnostics.
- **Typechecker / Symbol resolution:** member lookup must include an import-scoped unit-property descriptor lane for eligible primitive receivers and unit-family aggregate receivers.
- **IR Lowering:** unit property reads must lower to typed constructor or helper calls owned by the active descriptor, preserving source spans and evaluation order.
- **Emission:** emitted code must preserve pure construction, runtime anchors, and aggregate helper calls without relying on generated-Rust field names as the semantic contract.
- **Stdlib / Runtime (`incan_stdlib`):** standard unit families may provide descriptors, unit value types, aggregate helpers, and explicit-context alternatives for runtime anchored conveniences.
- **Library packaging:** package artifacts must be able to carry unit-property descriptors so consumers do not need the producer's source layout or companion implementation details at lookup time.
- **Formatter:** formatter output should preserve unit property chains naturally and should not rewrite unit properties into constructor calls.
- **LSP / Tooling:** completion, hover, go-to-definition, and diagnostics should show active unit vocabularies, property result types, runtime-anchor status, and missing imports.

## Unresolved questions

- Should standard unit property vocabularies be activated by importing ordinary modules, by importing dedicated `*.units` descriptors, or by a separate `import std.datetime.units` vocabulary form?
- Should unit-property descriptors be authored in Incan source, in a vocab companion crate, in package metadata generated from Incan declarations, or through more than one accepted producer path?
- Should `.from_now` and `.ago` be properties, methods, or both, given that they read runtime clock state?
- What exact trait or descriptor model should define tuple aggregate properties such as `(3.days, 12.hours).from_now`?
- Which standard unit family should be implemented first to prove the model: temporal units, byte-size units, layout units, or percentages?
- Should fractional temporal units such as `1.5.hours` be accepted, and if so which numeric receiver types should they require?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
