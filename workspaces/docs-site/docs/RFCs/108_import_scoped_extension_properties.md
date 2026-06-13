# RFC 108: Import-scoped extension properties

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

This RFC defines import-scoped extension properties: library-authored `pub vocab` blocks may expose property-shaped receiver vocabulary such as `3.days`, `2.5.percent`, `"/users/{id}".route`, or `order.status.badge` only where that vocabulary is explicitly imported. The flagship use case is typed unit authoring, but the north-star feature is broader: a vocabulary can publish statically checked extension-property facts for ordinary Incan receivers without reopening types, mutating global member lookup, or relying on runtime method discovery. Extension-property facts are derived from typed Incan property bodies written as `property name on ReceiverType -> ResultType:`.

## Core model

1. **Vocabularies own extension properties:** a `pub vocab` block is an importable vocabulary surface that may contain extension-property declarations.
2. **Imports activate the surface:** importing a vocabulary activates its extension properties in the importing module or lexical scope; dependency installation alone does nothing.
3. **Receivers are not reopened:** `int`, `decimal`, `FrozenStr`, custom nominal types, and foreign-backed types do not gain global members when a package is loaded.
4. **Authoring is ordinary typed Incan:** `property days on int -> TimeDelta:` has a body, `self` has type `int`, and the compiler derives descriptor facts from the checked declaration.
5. **Use sites are ordinary property reads:** callers write `receiver.property`, not `receiver.property()`, following RFC 046's field-like property model.
6. **Descriptor facts stay inspectable:** the compiler records the vocabulary identity, property name, receiver type, result type, lowering target, source span, evaluation classification, const eligibility, family metadata when present, and conflict behavior.
7. **Typed units are the proving case:** temporal amounts, byte sizes, layout lengths, rates, basis points, and similar scalar wrappers demonstrate why receiver-first property syntax is valuable.
8. **Other exact receivers are valid when scoped:** `FrozenStr` route patterns and custom nominal UI projections are legitimate vocabulary overlays when the behavior belongs to an imported vocabulary rather than the receiver's core type.
9. **Runtime magic is out of bounds:** extension properties must be statically resolved, typechecked, lowered, documented, and diagnosed; load order, string lookup, monkey patching, and dynamic dispatch do not define the feature.

## Motivation

Incan already has computed properties, typed numeric values, newtypes, temporal intervals, frozen const-friendly values, custom nominal types, and vocabulary-driven DSL surfaces. What it lacks is a source-level way for a library to say: "when this vocabulary is imported, this receiver type has this value-like property, and the compiler can see exactly how it lowers." Constructor calls such as `TimeDelta.days(3) + TimeDelta.hours(12)` are clear and should remain valid, but they are heavier than the domain idea being expressed. `3.days` is short, readable, and still statically typed if the property resolves through an imported vocabulary fact.

The same shape is useful outside temporal units. `256.mib` can carry byte-size semantics instead of a raw integer. `2.5.percent` can carry rate semantics instead of a float or string. `"/users/{id}".route` can parse and validate a route pattern from a `FrozenStr` before runtime. `order.status.badge` can be an admin-UI projection imported from the UI layer rather than a permanent core-domain member on `OrderStatus`.

The design must not reopen the dynamic-language tradeoff that Incan deliberately avoids. The language should not let libraries mutate primitive types globally, shadow owned members by load order, or hide expensive or fallible runtime work behind property syntax. The useful Ruby/Rails lesson is the fluent domain shape; the Incan lesson is that the shape must remain explicit, typed, scoped, and inspectable.

The earlier typed-unit framing is still important, but too narrow as the authoring model. Test-driving the syntax against `int`, `decimal`, `FrozenStr`, and custom nominal receivers shows that the language feature is import-scoped extension properties, with typed units as the strongest initial family of examples. Naming the broader feature now avoids painting RFC 108 into a unit-only corner while still keeping the unit examples concrete.

This RFC also points toward a larger dogfooding direction without taking ownership of it. RFC 027 currently exposes vocab registration through Rust companion crates. A future Incan could author more of its vocabulary metadata, descriptors, and compiler-adjacent surfaces in Incan itself, much as Rust uses Rust to author much of rustc. This RFC's `pub vocab` syntax should align with that direction, but it does not rewrite the existing vocab desugarer architecture.

### Prior art: Ruby and Rails Active Support

Ruby on Rails' Active Support is direct ergonomic prior art for the motivating temporal shape. Active Support exposes duration helpers on Ruby numeric values and documents examples such as `1.month.ago`, with duration values supporting methods such as `ago` and `from_now` in the Rails API docs: https://api.rubyonrails.org/classes/ActiveSupport/Duration.html.

Rails authors this by adding methods to existing Ruby classes. Conceptually, `Numeric` gets methods such as `days` and `hours` that return `ActiveSupport::Duration`, and `ActiveSupport::Duration` owns anchor methods such as `ago` and `from_now`. Ruby's runtime method table is the registry. That is a good explanation of the ergonomic surface and a poor model for Incan's safety goals.

Incan should give Rails credit for the read shape while making the registry explicit, typed, import-scoped, and inspectable.

### Prior art: Rust extension traits

Rust is the closest prior art for the safety model. A Rust crate can define an extension trait, implement it for a primitive receiver, and make receiver method syntax such as `3.days()` available only when the trait is imported. That gives scoped activation, static checking, coherent lowering, and no runtime mutation of the primitive type. Incan should preserve those properties while using Incan's existing field-like property access for value-like nullary operations.

Rust's limitation for this feature is syntax, not semantics. Rust has no field-like extension property surface, so the safe shape is method-like. Incan already has computed properties, so `property days on int -> TimeDelta:` can express the same imported capability without making users write `3.days()`.

## Goals

- Define `pub vocab` as the source-level authoring surface for import-scoped extension properties.
- Define `property name on ReceiverType -> ResultType:` as the extension-property declaration form inside a `vocab` block.
- Make imported vocabularies activate extension properties at use sites with ordinary property-read syntax.
- Allow receiver types beyond numeric primitives when the receiver is statically known, including `FrozenStr` and custom nominal types.
- Keep owned member lookup, trait member lookup, and extension-property lookup coherent and deterministic.
- Derive descriptor facts from checked Incan declarations rather than requiring authors to hand-maintain metadata fields.
- Preserve typed-unit examples as the primary readability and safety motivation.
- Support ordinary value composition such as `3.days + 12.hours` and leave room for aggregate vocabulary properties when the receiver is a checked family composition.
- Distinguish pure value construction from runtime-anchored conveniences such as `.from_now`, `.ago`, or clock-reading projections.
- Leave the larger RFC 027 vocab registration and desugarer rewrite as future-compatible but out of scope.

## Non-Goals

- This RFC does not add Ruby-style monkey patching, class reopening, method-table mutation, or load-order-dependent behavior.
- This RFC does not rewrite the current Rust `incan-vocab` companion crate model or WASM desugarer runtime.
- This RFC does not make all vocab metadata, parser extensions, or desugar functions authorable in Incan.
- This RFC does not define a full dimensional-analysis type system.
- This RFC does not commit the standard library to build temporal, data-size, layout, finance, routing, UI, science, or percentage vocabularies as part of this RFC. Those names are examples and pressure tests for the mechanism.
- This RFC does not make all nullary methods callable as properties.
- This RFC does not introduce setter properties or mutable extension state.
- This RFC does not allow extension properties to override owned fields, methods, computed properties, or trait members.
- This RFC does not bless arbitrary cute receiver nouns such as `3.users`, `404.not_found`, or `5.retry_policy` as good API design merely because the syntax can express them.

## Guide-level explanation

A vocabulary author writes a `pub vocab` block and declares extension properties with explicit receivers:

```incan
pub vocab temporal_units:
    property days on int -> TimeDelta:
        return TimeDelta.days(self)

    property hours on int -> TimeDelta:
        return TimeDelta.hours(self)

    property minutes on int -> TimeDelta:
        return TimeDelta.minutes(self)
```

Inside each property body, `self` has the receiver type named after `on`. The declaration is ordinary Incan code, not a metadata table. The compiler typechecks the body, records that `days` is available on `int` when `temporal_units` is active, and lowers `3.days` through the checked property body.

Users activate the vocabulary by importing it:

```incan
from std.datetime.units import temporal_units

deadline = (3.days + 12.hours).from_now
retry_window = 250.milliseconds
```

The call-site rule is simple: if the vocabulary is active and the receiver type matches, `receiver.property` is available as a property read. Without the import, `3.days` is rejected with a missing-vocabulary diagnostic rather than falling back to runtime lookup.

Data-size and layout units use the same authoring and call-site model:

```incan
pub vocab byte_units:
    property kib on int -> ByteSize:
        return ByteSize.kibibytes(self)

    property mib on int -> ByteSize:
        return ByteSize.mebibytes(self)
```

```incan
from std.data.units import byte_units

upload_limit = 256.mib
chunk_size = 64.kib
```

Finance and analytics examples show why decimal receivers matter:

```incan
pub vocab rate_units:
    property percent on decimal -> Rate:
        return Rate.percent(self)

    property bps on int -> Rate:
        return Rate.basis_points(self)
```

```incan
from std.finance.units import rate_units

fee_rate = 2.5.percent
spread = 120.bps
```

`FrozenStr` lets vocabularies define compile-time string surfaces without admitting arbitrary runtime strings:

```incan
pub vocab route_literals:
    property route on FrozenStr -> RoutePattern:
        return RoutePattern.parse_const(self)
```

```incan
from std.web.routes import route_literals

users = "/users/{id}".route

const HEALTH: FrozenStr = "/health"
health = HEALTH.route

path: str = read_path()
dynamic = path.route  # error: `.route` requires FrozenStr, not runtime str
```

This spelling reuses the existing `FrozenStr` concept instead of inventing a special `const str` receiver syntax. A literal or const-derived string may satisfy a `FrozenStr` receiver; a runtime `str` must use an explicit parser such as `RoutePattern.parse(path)` or another library-owned fallible API.

Custom nominal receivers are valid when the property belongs to an imported vocabulary layer rather than the receiver's core type:

```incan
pub vocab admin_ui:
    property badge on OrderStatus -> Badge:
        match self:
            OrderStatus.Pending => return Badge.warning("Pending")
            OrderStatus.Paid => return Badge.success("Paid")
            OrderStatus.Cancelled => return Badge.muted("Cancelled")
```

```incan
from app.admin.ui import admin_ui

badge = order.status.badge
```

This is appropriate when `badge` is an admin presentation concern. If `OrderStatus.badge` is core domain behavior, the property should live on `OrderStatus` itself instead of being injected by a vocabulary.

The syntax can express bad APIs too. The RFC should make that visible rather than pretending syntax solves taste:

```incan
pub vocab suspicious:
    property users on int -> list[User]:
        return db.users.limit(self)

    property json on str -> Json:
        return Json.parse(self)
```

Those examples hide I/O, fallibility, or expensive runtime parsing behind property reads. They should be rejected by diagnostics when the compiler can prove the effect boundary, or strongly discouraged by docs and lints when the library surface is technically expressible but misleading.

### Calling syntax summary

Authoring syntax:

```incan
pub vocab <name>:
    property <property_name> on <ReceiverType> -> <ResultType>:
        <body using self>
```

Calling syntax:

```incan
from some.module import <name>

value = receiver.<property_name>
```

## Reference-level explanation

### Vocab declaration

A `vocab` declaration defines an importable vocabulary surface. A `pub vocab` declaration exports that vocabulary from the module like other public declarations. Importing the vocabulary activates its extension-property facts according to ordinary import and reexport rules.

The declaration form is:

```incan
pub vocab Name:
    property member on ReceiverType -> ResultType:
        body
```

The `pub` modifier follows the ordinary declaration visibility model. A private `vocab` may be used internally by the declaring module or package according to the same visibility rules as other declarations.

### Extension-property declaration

An extension-property declaration is valid inside a `vocab` body:

```incan
property name on ReceiverType -> ResultType:
    body
```

`name` is the property name exposed at use sites. `ReceiverType` is the source type that may receive the property when the vocabulary is active. `ResultType` is the property result type. The body is an ordinary property body, except that `self` is bound to the receiver value rather than to an enclosing class or model instance.

The compiler must typecheck the body under `self: ReceiverType` and against the declared result type. The descriptor fact must be derived from the checked declaration. The lowering target is the property body or an equivalent generated helper with the same checked semantics. Authors must not need to repeat fields such as `constructor =`, `eval =`, or `const =` in a parallel metadata table.

If future syntax allows author-provided purity, const, or runtime-anchor annotations, the compiler must verify those annotations against the body or against trusted lower-level metadata. An unchecked author assertion that a property is pure or const-evaluable does not satisfy this RFC.

### Activation and scope

An extension property is active only when its owning vocabulary is imported, reexported, or otherwise activated by a scoped-vocabulary mechanism accepted by the language. Activation must be visible in source or in checked package metadata. A dependency being installed or loaded must not alter member lookup by itself.

When a module imports a vocabulary, that vocabulary's extension properties are visible in the covered scope. Public reexports may make a vocabulary available to downstream consumers if the reexported API surface explicitly includes it.

Tooling must be able to report which vocabularies are active for a file, which extension properties they contribute, and where each property was declared.

### Receiver eligibility

The receiver expression for an extension property must typecheck as compatible with the declaration's `ReceiverType`. This RFC's examples include `int`, `decimal`, `FrozenStr`, and custom nominal types such as `OrderStatus` and `UserId`.

`FrozenStr` is the preferred spelling for const-friendly string receivers. A string literal or const-derived string may satisfy a `FrozenStr` receiver when the active extension property supplies that context. A runtime `str` must not satisfy `FrozenStr` merely because its value happens to be immutable at runtime.

The north-star design may admit richer receiver predicates such as numeric families, trait-bounded receivers, or literal-only refinements, but those predicates must remain statically decidable, inspectable, and coherent. This RFC does not require Rust-shaped `impl` syntax or arbitrary foreign-for-foreign implementation rules to express receiver eligibility.

### Lookup and ambiguity

For a property read `receiver.name`, the checker must first typecheck the receiver. If ordinary owned member lookup on the receiver type finds a field, method, computed property, or trait member named `name`, ordinary member rules apply.

If ordinary member lookup does not resolve the read, the checker may consider active extension-property facts whose receiver type accepts the receiver expression and whose property name is `name`. If exactly one fact applies, the read has that fact's result type and lowers through that fact's checked body.

If more than one active fact applies and the language has no deterministic specificity rule for the pair, the checker must report an ambiguity diagnostic that names the competing vocabularies. The checker must not choose by import order, dependency order, or runtime load order.

An extension property must not silently override an owned field, owned method, owned computed property, or trait member. This preserves the no-monkey-patching guarantee and keeps primary behavior attached to the type that owns it.

### Descriptor facts

Each checked extension-property declaration produces a descriptor fact containing at least:

- the stable vocabulary identity;
- the source-level property name;
- the receiver type or receiver predicate;
- the result type;
- the checked lowering target;
- source spans for the declaration and property name;
- package and module provenance;
- evaluation classification, such as pure construction, runtime anchor, or effectful operation when the compiler can classify it;
- const-evaluation eligibility when the body and dependencies permit const evaluation;
- optional semantic family membership for unit composition or aggregate properties;
- conflict and overlap metadata sufficient for deterministic lookup and diagnostics.

The descriptor is a compiler-facing artifact. It may be serialized into package metadata, surfaced in LSP hover/completion, used by docs generation, and consumed by downstream builds. Its meaning is determined by checked Incan declarations, not by generated Rust names or runtime reflection.

### Composition and aggregate properties

Extension properties return ordinary typed values. Ordinary operators, methods, and owned properties should be used when the returned value can naturally own the next operation:

```incan
deadline = (3.days + 12.hours).from_now
```

In that example, `3.days` and `12.hours` produce `TimeDelta`, `+` combines `TimeDelta`, and `from_now` can be an owned property or method on `TimeDelta`.

Some vocabularies may still need aggregate properties over checked compositions, such as a tuple whose elements all belong to a semantic family:

```incan
deadline = (3.days, 12.hours).from_now
```

If aggregate extension properties are admitted, their descriptors must state the accepted aggregate receiver shape and family contract. The compiler must typecheck each component, reject incompatible families, and lower through an ordinary checked helper. The compiler must not infer tuple aggregation by concatenating source text or reading property names.

### Runtime anchors and effects

Property syntax should remain value-like. Pure unit construction such as `3.days`, `256.mib`, and `2.5.percent` fits naturally. Runtime-anchored properties such as `.from_now` and `.ago` require more care because they consult a clock or other ambient runtime capability.

A runtime-anchored property must not be const-evaluable. It must be visible to inspection and diagnostics as runtime capability use. Libraries that provide runtime-anchored convenience properties should also provide explicit-context spellings suitable for tests and governed runtimes, such as `duration.after(clock.now())`, `duration.before(anchor)`, or another library-owned equivalent.

Effectful, fallible, I/O-bound, or expensive operations should normally be methods or functions rather than properties. If the language later admits effectful extension properties, the compiler and tooling must make the effect boundary visible rather than allowing Ruby-style surprise behavior.

### Const contexts

A pure extension property may be valid in a `const` initializer only when its body is const-evaluable and every required conversion is permitted in const context. A runtime-anchored or effectful property must be rejected in const and compile-time-only contexts.

### Diagnostics

The compiler must diagnose:

- missing vocabulary activation;
- unknown extension property for the receiver type;
- ambiguous active vocabularies;
- receiver type mismatch, including runtime `str` where `FrozenStr` is required;
- invalid runtime-anchored or effectful property use in const contexts;
- aggregate property use on incompatible receiver families;
- attempted conflicts with owned members that the lookup model cannot resolve safely.

Diagnostics should name the property, receiver type, owning vocabulary when known, and suggested import when a known vocabulary is missing. Diagnostics should avoid suggesting class reopening, monkey patching, or load-order fixes.

## Design details

### Syntax

This RFC adds a declaration form, not a new use-site expression form:

```incan
pub vocab temporal_units:
    property days on int -> TimeDelta:
        return TimeDelta.days(self)
```

Use sites continue to use ordinary property access:

```incan
3.days
256.mib
"/users/{id}".route
order.status.badge
```

The keyword `vocab` should align with the existing RFC 027 vocabulary direction. This RFC does not require `pub vocab` to replace every Rust companion-crate registration surface, but the spelling should be chosen so future Incan-authored vocabulary metadata can grow in the same direction.

### Relationship to computed properties

RFC 046 computed properties define field-like member reads on owned types. Extension properties reuse the same property-read mental model but change ownership. A computed property declared in a model, class, enum, or concrete trait implementation belongs to that receiver type. An extension property declared in a `vocab` belongs to the vocabulary and is visible only when imported.

The `on ReceiverType` clause is the explicit source of `self` for an extension property. This avoids the ambiguity of a surrounding `for int:` block and keeps the receiver visible on every exported property.

### Relationship to RFC 027 vocab surfaces

RFC 027 defines the current Rust-facing registration and desugarer architecture for richer vocabulary surfaces. RFC 108 should not conflict with that architecture. A `pub vocab` block can be understood as an Incan-authored producer of checked vocabulary metadata. The extension-property subset can coexist with Rust companion crates, scoped symbols, scoped operators, block declarations, and external WASM desugarers.

Future work may allow more RFC 027-style metadata and desugar hooks to be authored in Incan. This RFC deliberately avoids specifying that larger rewrite. The compatibility requirement is that `pub vocab` extension properties produce metadata that can live beside existing vocab metadata rather than forming a parallel system.

### Relationship to Rust-shaped extension behavior

Rust extension traits are the safety baseline for this feature, but not the exact source shape. In Rust, a trait import can make `.days()` visible for an integer receiver without changing the integer type globally. Incan wants the same import-scoped capability model, while allowing `.days` when the operation is value-like and nullary.

The source syntax must not become Rust `impl Trait for Type`. Incan's user-facing model is vocabulary import plus property declaration. The backend may lower through Rust traits, helper functions, generated methods, or other static artifacts as long as the checked source contract is preserved.

### Relationship to type ownership

Extension properties are best used for vocabulary overlays, not primary behavior. If a project owns `OrderStatus` and `badge` is domain behavior, `OrderStatus` should declare an owned property. If `badge` belongs to an admin UI package, an imported `admin_ui` vocabulary may define `property badge on OrderStatus -> Badge:`.

This guidance matters because extension properties can otherwise scatter important behavior across imports. The language provides scoped expressiveness; API design still needs ownership discipline.

### Standard library examples

The standard-library-looking modules in this RFC are examples and pressure tests. They show that the mechanism should be able to support temporal, data-size, layout, finance, route, UI, scientific, or percentage vocabularies without new compiler special cases for each family. They are not delivery commitments. Each standard vocabulary, if pursued, should still have its own RFC, issue, or stdlib design note defining its value types, semantics, overflow behavior, const behavior, display, parsing, and runtime capabilities.

### Compatibility and migration

This RFC is additive. Existing code that does not import a vocabulary is unaffected. Constructor-style APIs such as `TimeDelta.days(3)` and explicit parsers such as `RoutePattern.parse(path)` remain valid and should continue to be documented where they are clearer, fallible, dynamic, or not value-like.

## Alternatives considered

1. **Constructor functions only.** This preserves the current explicit style but leaves common scalar units and const string surfaces noisier than necessary and encourages raw values in source.
2. **Global primitive extension methods.** This gives the nicest use-site syntax but reintroduces monkey-patching hazards and makes receiver behavior depend on dependency graph accidents.
3. **Rust-shaped `impl` syntax.** Clear to Rust readers, but it creates a second source-level conformance grammar that Incan has already avoided elsewhere. `pub vocab` keeps the feature Incan-shaped while preserving Rust's safety properties.
4. **A metadata-only DSL such as `unit_vocab` with `constructor = ...`.** Rejected as the main authoring model because it asks authors to maintain a parallel metadata table and provides places to lie about purity, const behavior, and lowering.
5. **String-based units such as `unit(3, "days")`.** Easy to implement but loses checked names, completion, docs, refactoring, and source-level type evidence.
6. **Only support numeric unit receivers.** Too narrow. Test-driving the syntax shows `FrozenStr` and custom nominal overlays are legitimate import-scoped extension-property use cases.
7. **Rewrite all vocab authoring in Incan now.** Directionally attractive, but too broad for this RFC. RFC 108 should align with that future without owning the desugarer/runtime rewrite.

## Drawbacks

- Member lookup becomes more complex because active imports can contribute extension-property facts.
- API authors may overuse property syntax for behavior that should remain an explicit function or method.
- Import-scoped behavior can make source less obvious if tooling does not surface active vocabularies clearly.
- Custom receiver overlays can scatter behavior if package authors ignore type-ownership boundaries.
- Runtime-anchored properties such as `.from_now` are convenient but require careful docs and diagnostics so they do not hide clock or policy dependencies.
- The `pub vocab` authoring surface overlaps conceptually with RFC 027, so the metadata model must stay unified rather than creating a second vocabulary system.

## Implementation architecture

This section is non-normative. A practical implementation can model `pub vocab` declarations as producers of checked vocabulary metadata. During symbol collection or typechecking, each `property name on ReceiverType -> ResultType:` declaration is checked as a property body with an explicit receiver binding. The resulting extension-property facts are attached to the vocabulary symbol and serialized with package metadata. Member lookup asks ordinary owned members first, then active extension-property facts, and lowering emits a static call or generated helper for the selected property body.

Existing RFC 027 metadata and desugarer artifacts can remain valid. `pub vocab` should produce compatible metadata records for the extension-property subset, allowing future Incan-authored vocab declarations to grow beside current Rust companion crates.

## Layers affected

- **Parser / AST**: parse `vocab` declarations and `property name on ReceiverType -> ResultType:` inside vocab bodies while preserving spans for the vocabulary name, property name, receiver type, and result type.
- **Typechecker / Symbol resolution**: bind `self` to the declared receiver type inside extension-property bodies, check result types, derive extension-property facts, activate imported vocabularies, and resolve property reads through owned members before active extension facts.
- **IR Lowering**: lower selected extension-property reads to checked property bodies, generated helpers, or equivalent static calls while preserving vocabulary provenance and evaluation order.
- **Emission**: emit static code for extension-property bodies without runtime member lookup, string dispatch, or monkey patching.
- **Stdlib / Runtime (`incan_stdlib`)**: standard vocabularies may provide value types, helper functions, explicit dynamic alternatives, and runtime-anchor APIs used by extension properties.
- **Library packaging**: package artifacts must carry vocabulary metadata and extension-property facts so consumers can resolve imports without source-layout assumptions.
- **Formatter**: format `vocab` blocks and extension-property declarations consistently with existing declaration and property formatting.
- **LSP / Tooling**: completion, hover, go-to-definition, generated docs, and diagnostics must show active vocabularies, extension-property receiver types, result types, provenance, missing imports, and runtime-anchor status.

## Unresolved questions

- Is `vocab` the right keyword for this source-level authoring surface, or should it reuse a more explicit RFC 027 spelling?
- Should `property name on ReceiverType -> ResultType:` be the final ordering, or should the receiver appear in another position while preserving readability?
- What receiver predicate language should be admitted beyond named receiver types such as `int`, `decimal`, `FrozenStr`, and custom nominal types?
- How should the language represent evaluation classification, const eligibility, and runtime anchors when they cannot be inferred from ordinary Incan bodies?
- Should string literals be contextualized as `FrozenStr` by active extension-property lookup, and what diagnostics should explain that behavior?
- Should aggregate extension properties over tuples or other family compositions be part of this RFC's accepted surface, or should ordinary value composition plus owned properties carry most examples?
- Should `.from_now` and `.ago` be properties, methods, or both, given that they read runtime clock state?
- How should `pub vocab` metadata compose with existing Rust companion-crate vocab registrations and future Incan-authored desugar functions?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
