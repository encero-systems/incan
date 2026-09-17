# RFC 122: Composable model construction with fieldmethods and `new`

- **Status:** Draft
- **Created:** 2026-09-03
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 017 (validated newtypes with implicit coercion)
    - RFC 021 (model field metadata and schema-safe aliases)
    - RFC 084 (RHS partial callable presets)
    - RFC 086 (schema descriptors and adapters)
    - RFC 087 (reusable field contracts and structural model composition)
    - RFC 097 (Rust-hosted Incan caller)
    - RFC 109 (receiver chain combinators)
- **Issue:** [#1295](https://github.com/encero-systems/incan/issues/1295)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

This RFC introduces composable model construction through model-only `@fieldmethod` hooks and context-delimited construction expressions. Every model field receives a synthesized field-backed fieldmethod unless the model authors one, while parameterized `@fieldmethod(...)` declarations add typed computed inputs such as `hours` over an exact stored-field write set. `partial Dog.paws(4).coat_color("brown")` preserves a reusable constructor plan; `new Dog.paws(4)` and `Dog.paws(4)()` finalize one immediately. As declaration-context shorthand, a fieldmethod chain that is the complete right-hand side of a direct module-level assignment may omit `partial` and still preserve a reusable plan. Unmarked chains remain rejected inside executable code, so fieldmethods do not become general static members. Fieldmethods customize pending construction rather than mutate completed model values, and constructor presets remain reusable callable values rather than becoming types or subclasses.

## Core model

1. **A model constructor has a field-aware construction surface:** inside a construction context recognized by this RFC, each accessible model field is callable on that model's constructor and constructor partials.
2. **Fieldmethods customize pending construction:** `@fieldmethod` is valid only in a model and runs against a model-construction context, not a completed model instance.
3. **Ordinary fieldmethods are synthesized:** when no authored fieldmethod exists for a field, the compiler provides one that binds the supplied value to that field.
4. **The decorator form distinguishes the method kind:** bare `@fieldmethod` replaces the synthesized behavior for its same-named stored field, while parameterized `@fieldmethod(minutes)` or `@fieldmethod(days, seconds, nanoseconds)` declares a computed virtual input and its exact stored-field write set.
5. **Construction is context-delimited:** `partial Dog.paws(4)` preserves a reusable constructor plan, while `new Dog.paws(4)` and `Dog.paws(4)()` finalize one construction. A fieldmethod chain that forms the complete right-hand side of a direct module-level assignment may omit `partial` and still denote a reusable plan; an unmarked chain inside executable code is rejected.
6. **All construction spellings converge:** fluent construction, direct keyword construction, RFC 084 model partials, decoding, and supported foreign construction use the same checked construction-plan metadata and canonical finalizer.
7. **Private state remains model-owned:** a public model may retain private fields, and a public computed fieldmethod may provide a controlled construction input for private stored targets without exposing those fields directly.
8. **Fieldmethod and checked-conversion behavior is statically safe:** every fieldmethod and RFC 017 `from_underlying` hook must be a compiler-classified construction-safe callable: synchronous, deterministic, and free of externally observable effects across its transitive call graph. Existing whole-model validators retain their current effect contract in this RFC.
9. **Later calls to the same construction input win:** a later call replaces an earlier preset for the same fieldmethod identity, and superseded fieldmethod bodies do not execute. Stored defaults initialize the context before the surviving fieldmethods run.
10. **Construction preserves type boundaries:** fieldmethod parameters use the declared field type, RFC 017 checked newtype coercion may be inserted at its existing approved sites, and unrelated primitive widening or parsing is not introduced.
11. **Constructor specialization is not inheritance:** a partial derived from `Dog` is a callable that constructs `Dog`; it is not a type, subtype, model declaration, or source of inherited behavior.

## Motivation

Incan models are data-first values with declared fields, keyword-only construction, defaults, validation, schema metadata, and no inheritance. RFC 084 already lets authors publish constructor presets such as `BronzeReader = partial Reader(layer="bronze")`, which covers simple default specialization without subclasses. It does not provide a field-oriented fluent surface, a place to normalize an individual pending field, or a context-delimited construction expression that can be read as a left-to-right model recipe.

Static factory methods do not solve the complete problem. A static `TimeDelta.days(4)` can construct one value, but after that call the receiver is a completed value, so `.hours(3)` must either mutate or copy that value and can no longer participate in required-field checking as pending construction. A static method also cannot deliberately share a field's name under an ordinary single member namespace without creating a field/member collision.

Hand-written builder models can express pending construction, but they duplicate every model's fields, types, defaults, validation boundaries, documentation, and editor surface. Inheritance can specialize constructors in some object-oriented languages, but it introduces subtype relationships, method-resolution rules, and fragile behavioral coupling when the desired result is only a reusable configuration recipe.

The desired surface is smaller and more direct:

```incan
Labrador = partial Dog
    .paws(4)
    .tail_length(20)
    .coat_color("brown")
    .size(DogSize.Medium)

def adopt_dogs() -> tuple[Dog, Dog]:
    Stewie = new Labrador
    Pedro = new Labrador.coat_color("black")
    return (Stewie, Pedro)
```

`Labrador` is a typed constructor specialization. `Stewie` and `Pedro` are `Dog` values. No subclass, duplicate model, or mutable half-object is involved. A caller that prefers callable syntax may also write `Labrador()` or terminate an inline fieldmethod chain with `()`.

## Goals

- Add a model-only `@fieldmethod` method kind for controlling how declared and virtual model inputs participate in construction.
- Synthesize an ordinary fieldmethod for every model field that does not define one explicitly.
- Require authored fieldmethods to declare a compiler-checked stored-field write set, with a same-name shorthand for field-backed methods.
- Allow authored computed fieldmethods to introduce typed virtual construction inputs that update pending model fields.
- Allow model constructors and model-constructor partials to be specialized through fluent fieldmethod chains.
- Extend RFC 084's `partial` marker to preserve declaration-safe fieldmethod chains as top-level constructor partials without module initialization, while permitting omission of that marker when such a chain is the complete right-hand side of a direct module-level assignment.
- Add a contextual `new` expression and an empty postfix finalizer as two equivalent ways to finalize a fieldmethod chain.
- Route ordinary model constructor arguments and validated model constructor arguments through the same fieldmethod behavior.
- Preserve RFC 084's overrideable-default behavior and define a deterministic model-owned execution schedule when fieldmethods contain logic.
- Preserve model field visibility, including private fields on public models, aliases, defaults, validation, newtype coercion, checked API metadata, documentation, and diagnostics through fluent construction.
- Make constructor partials compose without introducing inheritance or nominal subtyping.
- Define and enforce one reusable construction-safe callable classification for fieldmethods and RFC 017 `from_underlying` hooks without introducing a general effect system.

## Non-Goals

- Adding inheritance, subclasses, method inheritance, constructor inheritance, or method-resolution ordering.
- Treating a constructor partial such as `Labrador` as a type or permitting it in type position.
- Defining automatic serialization, deserialization, or a stable wire representation for model-constructor partials.
- Adding fieldmethods to classes, traits, enums, or newtypes.
- Calling fieldmethods on completed model instances or turning them into copy-update methods.
- Replacing ordinary instance methods, static methods, computed properties, RFC 109 receiver combinators, or whole-model validation.
- Applying the construction-safe callable classification to whole-model `validate` methods. Existing validators retain their current effect rules to avoid expanding their established contract inside this RFC; a later RFC may unify validator and construction-hook safety.
- Introducing implicit primitive parsing or widening such as `str -> int` or `int -> float`.
- Changing RFC 017's explicit `from_underlying` recovery surface or its failure behavior for implicit checked coercion.
- Defining frozen constructor bindings in the initial surface; the construction-plan representation must leave room for them without making them part of this RFC.
- Allowing arbitrary top-level execution under the appearance of a constructor chain.
- Extending keyword `new` to class allocation or identity-bearing object construction in this RFC.
- Making bare fieldmethod chains into values outside the direct module-level assignment context or a `partial`, `new`, or terminating empty-call construction context.
- Proving that construction-safe callables terminate, cannot panic, or are evaluable at compile time.
- Introducing source annotations that let ordinary Incan-bodied functions opt into construction safety without compiler verification.

## Guide-level explanation

Model and named-partial declarations in this guide are shown at module scope. When an executable `new`, constructor call, or `assert` appears without a containing function, the snippet is a function-body fragment rather than a claim that executable construction is permitted at module top level.

### Ordinary fluent model construction

Inside a recognized construction context, every accessible model field is available as a fieldmethod on the model constructor. An authored computed fieldmethod may also add logic while declaring its exact stored-field targets:

```incan
model TimeInterval:
    minutes: int = 0

    @fieldmethod(minutes)
    def hours(mod, hours: int):
        mod.minutes += hours * 60

    @fieldmethod(minutes)
    def weeks(mod, weeks: int):
        mod.minutes += weeks * 7 * 24 * 60
```

`hours` and `weeks` are computed construction inputs, while `minutes` retains its synthesized field-backed input. The computed methods declare `minutes` as their exact write target. Sharing a target is explicit at the model definition and does not make the logical inputs aliases of one another. The synthesized `minutes` input replaces the default with the caller's value before computed inputs contribute to the stored representation. A concrete interval can therefore be written as one construction expression:

```incan
interval = new TimeInterval
    .hours(2)
    .minutes(30)

assert interval.minutes == 150

fortnight = new TimeInterval.weeks(2)
assert fortnight.minutes == 20_160
```

The chain after `new` does not call methods on an already completed `TimeInterval`. It accumulates pending fieldmethod inputs. At the end of the complete `new` expression, the effective fieldmethods update one pending model state and that state is finalized once.

The same expression may be written on one line when that is clearer:

```incan
interval = new TimeInterval.hours(2).minutes(30)
```

### Reusable constructor specializations

The `partial` marker preserves a fieldmethod chain as a reusable model-constructor partial and provides an explicit spelling:

```incan
type Paws = newtype int[ge=0, le=4]
type TailLength = newtype int[ge=0]

enum DogSize:
    Small
    Medium
    Large

model Dog:
    paws: Paws = 4
    tail_length: TailLength
    coat_color: str
    size: DogSize

Labrador = partial Dog
    .paws(4)
    .tail_length(20)
    .coat_color("brown")
    .size(DogSize.Medium)

Chihuahua = partial Dog
    .tail_length(10)
    .coat_color("white")
    .size(DogSize.Small)
```

When a fieldmethod chain is the complete right-hand side of a direct module-level assignment, `partial` is optional:

```incan
BrownDog = Dog.paws(4).coat_color("brown")
ExplicitBrownDog = partial Dog.paws(4).coat_color("brown")
```

Both declarations preserve reusable model-constructor partials with the same effective inputs and projected callable surface. Neither spelling is preferred or warned about. The LSP may offer an explicit code action to add or remove `partial`, but it must not emit a hint merely because either legal spelling was selected. The formatter must preserve the author's spelling.

The capitalized names are callable constructor partials, not new model types. Finalization is executable and therefore occurs inside an ordinary function rather than at module top level:

```incan
def adopt_dogs() -> tuple[Dog, Dog]:
    Stewie: Dog = new Labrador
    Pedro: Dog = new Chihuahua.coat_color("black")
    return (Stewie, Pedro)
```

Ordinary invocation remains available because constructor partials remain callables:

```incan
def adopt_more_dogs() -> tuple[Dog, Dog]:
    Stewie = Labrador()
    Pedro = Chihuahua(coat_color="black")
    return (Stewie, Pedro)
```

An inline fieldmethod chain may also be finalized with an empty postfix call:

```incan
dog = Dog.paws(4).tail_length(20).coat_color("brown").size(DogSize.Medium)()
```

The postfix form is equivalent to constructing and immediately invoking an unnamed partial. The `new` spelling moves that finalization signal to the beginning of the expression, while ordinary invocation remains useful when code treats a named preset uniformly with other callable values. Inside executable code, a chain without `partial`, `new`, or the terminating empty call is rejected. The only unmarked, unterminated form is the direct module-level assignment shorthand above, which always preserves a reusable plan and never finalizes it.

### Authored fieldmethods

An authored fieldmethod replaces the synthesized binding behavior for one field:

```incan
model Dog:
    paws: Paws = 4
    tail_length: TailLength
    coat_color: str
    size: DogSize

    @fieldmethod
    def coat_color(mod, value: str):
        normalized = value.strip().lower()

        if normalized == "yellow":
            normalized = "golden"

        mod.coat_color = normalized
```

Bare `@fieldmethod` declares a field-backed method because the method name resolves to the same-named stored field. Unlike parameterized `@fieldmethod(...)`, it does not declare a computed input. `mod` is a construction-context receiver. Assignment to `mod.coat_color` commits a value to the pending `coat_color` field and does not recursively invoke the fieldmethod. A field-backed method transforms the caller-supplied value and must not read its own prior or defaulted target before assigning it. The method's construction-context result is implicit, so no `return mod` is written and callers retain the fluent chain:

```incan
GoldenDog = partial Dog.coat_color("  YELLOW  ")

def adopt_golden_dog() -> Dog:
    return new GoldenDog.tail_length(18).size(DogSize.Medium)
```

Fieldmethods are not called on completed model instances:

```incan
dog.coat_color             # field read
dog.coat_color("black")    # compile-time error
```

This keeps model values data-first and prevents the construction surface from becoming implicit mutation or copy-update behavior.

### Computed construction inputs

An authored fieldmethod whose name is not a stored field introduces a virtual construction input. This is useful when user-facing construction vocabulary should be normalized into a smaller stored representation:

```incan
model DateTimeInterval:
    years: int = 0
    months: int = 0
    days: int = 0
    seconds: int = 0
    nanoseconds: int = 0

    @fieldmethod(days, seconds, nanoseconds)
    def hours(mod, value: int):
        normalized = normalize_day_time(
            days=mod.days,
            seconds=mod.seconds + value * 3_600,
            nanoseconds=mod.nanoseconds,
        )
        mod.days = normalized.days
        mod.seconds = normalized.seconds
        mod.nanoseconds = normalized.nanoseconds

    @fieldmethod(days, seconds, nanoseconds)
    def minutes(mod, value: int):
        normalized = normalize_day_time(
            days=mod.days,
            seconds=mod.seconds + value * 60,
            nanoseconds=mod.nanoseconds,
        )
        mod.days = normalized.days
        mod.seconds = normalized.seconds
        mod.nanoseconds = normalized.nanoseconds
```

`hours` is visible on the construction surface but is not stored on completed values:

```incan
interval = new DateTimeInterval.hours(30)
assert interval.days == 1
assert interval.seconds == 21_600

interval.hours       # compile-time error: no stored field or instance method
```

The decorator arguments are canonical stored-field names and form the method's declared write set. The compiler rejects a write to any other field, so the effect of a virtual input remains visible at its declaration. Computed fieldmethods may update multiple pending fields. Their purpose is still model construction; they do not add instance methods or hidden storage.

### Direct constructors use the same hooks

An authored fieldmethod is part of the model's construction contract, not special behavior limited to fluent syntax. Direct keyword construction must therefore use it:

```incan
dog = Dog(
    tail_length=20,
    coat_color="  YELLOW  ",
    size=DogSize.Medium,
)

assert dog.coat_color == "golden"
```

Field-backed constructor arguments and virtual inputs use the same hooks:

```incan
interval = DateTimeInterval(days=14, hours=20, minutes=3, seconds=42)
```

Constructor arguments are collected as effective construction inputs rather than executed in caller source order. Model defaults initialize pending stored state first, effective field-backed inputs run in canonical field declaration order, and effective computed fieldmethods then run in their model declaration order. A later explicit input replaces an earlier preset for the same fieldmethod identity. An authored field-backed method normalizes or otherwise transforms its supplied value before binding the field; it does not use the field's declared default as an accumulator. Computed fieldmethods own additive or cross-field behavior.

### Public models may retain private state

Making a model public does not force every stored field to become public. A public computed fieldmethod can expose a controlled logical input while keeping its canonical representation private:

```incan
pub model Money:
    _minor_units: int
    pub currency: str

    @fieldmethod(_minor_units)
    pub def amount(mod, major_units: int):
        mod._minor_units = major_units * 100
```

External callers may use either construction spelling because both route the public `amount` input through the same computed fieldmethod:

```incan
def prices() -> tuple[Money, Money]:
    fluent = new Money.amount(12).currency("EUR")
    direct = Money(amount=12, currency="EUR")
    return (fluent, direct)
```

They cannot pass, read, pattern-match, or fluently bind `_minor_units` directly. This preserves a model-owned construction boundary without changing `Money` into an identity-oriented class or banning useful private representation from public data types.

### Last binding wins

Constructor partials behave as overrideable defaults, consistent with RFC 084:

```incan
BrownLabrador = partial Dog.coat_color("brown").paws(4)
BlackLabrador = partial BrownLabrador.coat_color("black")

def adopt_black_labrador() -> Dog:
    return new BlackLabrador
```

Only the final pending input for `coat_color` is applied. The superseded `"brown"` fieldmethod invocation does not execute during construction. Replacing an input changes its effective value without changing the fieldmethod's model-defined execution slot.

Duplicate keywords inside one argument list remain errors:

```incan
Dog(coat_color="brown", coat_color="black")  # compile-time error
```

Last-one-in-wins applies across constructor partials, chained fieldmethod calls, and final call-site overrides that target the same construction-input identity. Stored defaults are initialization state rather than competing construction inputs. Different fieldmethods such as `hours`, `minutes`, and `weeks` all execute when supplied, even though they update the same stored field. Their overlap is intentional because the model author declared the same target on each method. The rule does not legalize duplicate syntax in one argument list.

The construction-input identity is the fieldmethod name, not its declared write targets. For the earlier `TimeInterval`, `hours` and `minutes` therefore both contribute regardless of their order:

```incan
morning = new TimeInterval.hours(2).minutes(30)
afternoon = new TimeInterval.minutes(30).hours(2)

assert morning.minutes == 150
assert afternoon.minutes == 150
```

Repeating the same logical input replaces it instead of accumulating it:

```incan
interval = new TimeInterval.hours(1).hours(2)
assert interval.minutes == 120
```

Only the final `hours` hook executes. Because repetition inside one fluent expression is easy to miss and seldom clearer than deleting the earlier call, the compiler warns at both call sites:

```text
warning: `hours` is called twice in this construction chain
  `hours(1)` is discarded; only `hours(2)` survives
```

This warning does not apply when a later construction layer deliberately overrides a named constructor partial, such as `new DefaultInterval.hours(2)`. The later value occupies the same model-defined execution slot. This distinction lets different units contribute to shared storage while preserving predictable override behavior for presets and call-site customization.

Repeating one fieldmethod never accumulates values. Even if its authored body uses `+=`, `append`, or another update operation, replacement occurs before fieldmethod execution and only the surviving body runs. A collection-oriented construction input should therefore accept the complete collection:

```incan
TaggedItem = partial Item.tags(["urgent", "external"])
```

It should not rely on repeated calls:

```incan
TaggedItem = partial Item.tag("urgent").tag("external")  # warning; only `external` survives
```

### Composable inputs and alternatives

Distinct fieldmethods are appropriate when their effects may be combined, including when they intentionally share write targets. `hours`, `minutes`, and `weeks` are separate inputs because a duration may include all three.

Mutually exclusive choices should instead share one construction-input identity whose value is represented by an enum, union, or another type that captures the alternatives. Defining separate fieldmethods for incompatible choices would cause both methods to execute when both are supplied; overlapping write targets do not imply exclusivity or precedence.

### Newtypes remain distinct

A synthesized or authored fieldmethod receives the declared field type:

```incan
type Days = newtype int

model TypedDateTimeInterval:
    days: Days = 0
```

The fluent call may still use the underlying integer because a function argument is already an RFC 017 checked-coercion site:

```incan
interval = new TypedDateTimeInterval.days(-7)
```

Conceptually, the compiler inserts the same checked conversion used by `Days(-7)` and `Days.from_underlying(-7)`. The method's parameter type remains `Days`; `int` does not become identical to `Days`, and the proposal adds no primitive widening. Authors who need recoverable validation and custom diagnostics retain the explicit `Result` form:

```incan
days = Days.from_underlying(external_value)?
interval = new TypedDateTimeInterval.days(days)
```

### Validated models

Keyword `new` must not bypass a model's canonical validation contract. `TypeName.new(...)` is the existing compiler-generated associated constructor for a model deriving `Validate`; it is an ordinary member call on the model type, not an instance method, Python-style `__new__`, or language keyword. Finalizing a construction expression has the same result type and validation behavior as that associated constructor:

```incan
@derive(Validate)
model PortRange:
    start: int
    end: int

    def validate(self) -> Result[Self, str]:
        if self.start > self.end:
            return Err("start must not exceed end")
        return Ok(self)

def configured_range() -> Result[PortRange, str]:
    return new PortRange.start(7000).end(8000)
```

For an ordinary model, a `new` expression has type `Model`. For a model whose canonical validated constructor has type `(...) -> Result[Model, E]`, the `new` expression has type `Result[Model, E]`. Existing raw-construction restrictions remain in force.

## Reference-level explanation

### Terminology

A **model constructor** is the compiler-known callable surface that constructs a particular model. A **model-constructor partial** is a callable construction plan derived from a model constructor with zero or more pending construction inputs. A **construction context** is the temporary, compiler-owned model state against which fieldmethods run before final model construction. A **write target** is a canonical stored field that an authored computed fieldmethod declares and must bind. A **field-backed fieldmethod** uses bare `@fieldmethod`, owns the construction input for its same-named stored field, and replaces that field's synthesized binding behavior. A **computed fieldmethod** uses parameterized `@fieldmethod(...)` and introduces a virtual construction input over its exact declared write targets. A **pending input** is a captured fieldmethod invocation and its source provenance. **Finalization** applies the effective fieldmethods to the construction context and invokes the model's canonical constructor exactly once.

Constructor partials are values with a projected callable surface. They are not nominal types and must not be accepted in type annotations, inheritance positions, trait-adoption positions, or pattern type positions.

### `@fieldmethod` declarations

`@fieldmethod` must be accepted only on a method declared directly inside a model body. Applying it to a class, trait, enum, newtype, extension surface, free function, nested function, or ordinary method must be rejected.

The declaration form is:

```text
FieldMethodDecl      ::= FieldMethodDecorator Newline Visibility? "def" FieldMethodName "(" "mod" "," ValueParam ")" ":" Suite
FieldMethodDecorator ::= "@fieldmethod" | "@fieldmethod" "(" FieldTargetList ")"
FieldTargetList      ::= FieldName ("," FieldName)* ","?
ValueParam           ::= Ident ":" TypeExpr
```

Each parameterized decorator argument must resolve directly to a canonical field declared or structurally composed into the owning model. Aliases and string field names are not accepted in the decorator, duplicate targets are errors, and a missing target is an error. The target list is part of the model's checked construction API.

Bare `@fieldmethod` is permitted only when the method name resolves to a canonical stored field. It infers that field as its single target, declares a field-backed fieldmethod, and replaces that field's synthesized behavior. A parameterized decorator always declares a computed fieldmethod and requires a non-empty exact write-target list. A computed fieldmethod name must not match a canonical stored field or collide with another constructor input, field alias, static member, ordinary method, computed property, or reserved constructor member. The parameterized same-name spelling `@fieldmethod(field_name) def field_name(...)` must be rejected with a diagnostic directing the author to bare `@fieldmethod`; this keeps the declaration form sufficient to distinguish field-backed and computed behavior.

A field-backed fieldmethod's value parameter type must be exactly the declared model field type after ordinary type-alias normalization. A broader primitive, narrower unrelated newtype, or merely assignable widening must be rejected. A computed fieldmethod may declare another concrete value type because it introduces its own construction input. RFC 017 checked conversion may adapt an underlying value at a call site where that RFC already permits it, but it does not make the supplied and declared types identical.

`mod` is a reserved receiver spelling for construction contexts. It must not be annotated with `self`, `cls`, `mut`, or a user-written construction-context type. The receiver permits construction-context field assignment by definition; it is not a mutable borrow of a completed model.

A fieldmethod must not declare positional parameters beyond its one value parameter, keyword parameters, variadic parameters, or a user-chosen return type. Its successful result is the same construction context and is returned implicitly. `return mod`, returning another value, or returning a replacement model must be rejected.

Every normally completing control-flow path through a fieldmethod must bind every target before completion. It must not assign, mutate, or update a stored field outside that target set. Authors must therefore declare exactly the stored fields the method writes: no omitted writes and no speculative extra targets. These rules make the decorator an exact construction effect rather than documentation, preserve required-field reasoning, and prevent a supplied construction input from silently leaving one of its declared outputs absent.

A field-backed fieldmethod transforms a caller-supplied value into its stored binding. It must not read its own target before assigning it, even when that field has a declared default; after assignment, ordinary definitely-assigned reads and updates are permitted. This prevents explicit field input from accidentally accumulating against initialization state. Additive behavior over a default belongs in a computed fieldmethod. Other construction-context reads follow the plan-insensitive rules in "Construction-context access."

Fieldmethods must not be async, generators, context managers, trait requirements, overloaded methods, static methods, class methods, computed properties, or decorator targets in this RFC.

### Synthesized fieldmethods

For every model field without a same-name authored fieldmethod, the compiler must synthesize behavior equivalent to:

```incan
@fieldmethod
def field_name(mod, value: FieldType):
    mod.field_name = value
```

The synthesized fieldmethod's visibility must follow the field's constructor visibility. A caller that cannot provide a private field to the ordinary constructor must not bind it through a synthesized or authored fieldmethod.

A synthesized or authored field-backed fieldmethod and its field intentionally share canonical field identity and spelling. This is not a collision. Computed fieldmethods and ordinary instance members, static methods, computed properties, and unrelated declarations do not receive this exception and remain subject to existing member collision rules.

For a code-spellable model field alias, fluent construction may use either the canonical field name or the alias under the same access rules as keyword construction. Both spellings must resolve to the same canonical fieldmethod identity. An authored fieldmethod is declared under the canonical field name only.

### Construction-context member resolution

Fieldmethods are not added to a model's general associated-member surface. Their member lookup is enabled only while checking one of the four construction contexts defined by this RFC:

- after the `partial` marker, `ModelName.fieldmethod(value)` or `ConstructorPartial.fieldmethod(value)` extends a reusable construction plan;
- after the `new` marker, the same forms extend a construction plan that will be finalized at the expression boundary;
- as the complete right-hand side of a direct module-level assignment, an unmarked fieldmethod chain over a model or constructor partial produces a reusable constructor partial;
- as the operand of a terminating empty call, a fieldmethod chain extends a construction plan that the empty call finalizes;

Completed model values remain outside all four contexts:

- `model_value.field` remains an ordinary field read.
- `model_value.fieldmethod(value)` must not resolve to a fieldmethod.

Outside those construction contexts, `ModelName.fieldmethod(value)` and `ConstructorPartial.fieldmethod(value)` must be rejected rather than produce a constructor partial implicitly. In executable function scope, the diagnostic should suggest prefixing the chain with `partial` or `new`, or adding a terminating empty call when immediate finalization is intended, and should note that only the complete right-hand side of a direct module-level assignment permits the reusable form without `partial`. The parser provides the construction context, but the typechecker still resolves the target model, member category, accessibility, input type, and canonical identities. An ordinary static member that returns a callable therefore retains its existing call behavior and is not reinterpreted as a fieldmethod chain.

The compiler must resolve a fieldmethod through canonical model and fieldmethod identity and retain its canonical write-target identities; a field-backed fieldmethod must additionally retain its canonical same-named field identity. Lowering and emission must not guess from a member's spelling or rediscover targets by inspecting the body.

### Explicit construction expressions

The conceptual grammar is:

```text
ModelPartialExpr    ::= "partial" ConstructionPlanExpr
NewModelExpr        ::= "new" (ModelPartialBase | ConstructionPlanExpr)
PostfixModelExpr    ::= FieldMethodChain "(" ")"
TopLevelModelPartialDecl ::= Visibility? Ident "=" FieldMethodChain
Visibility           ::= "pub"
ConstructionPlanExpr ::= ConstructorTemplate | FieldMethodChain
ConstructorTemplate ::= ModelPartialBase "(" ConstructionKeywordArgs? ")"
FieldMethodChain    ::= ModelPartialBase FieldMethodCall+
ModelPartialBase    ::= ModelName | ModelPartialName
FieldMethodCall     ::= LineContinuation? "." FieldMethodName "(" ArgumentExpr ")"
```

`partial` preserves the resulting plan as a model-constructor partial for the same target model. `new` finalizes the resulting plan at the expression boundary. A terminating empty call finalizes a fieldmethod chain and is semantically equivalent to creating and immediately invoking the corresponding unnamed partial. A non-empty call after a bare fieldmethod chain is rejected; final call-site overrides use a named partial or one consistently marked construction form instead.

At module top level, a direct assignment whose complete right-hand side is a statically resolvable fieldmethod chain is a declaration-safe constructor-partial declaration whether or not the chain is prefixed with `partial`. It may use the same optional `pub` visibility as an RFC 084 top-level partial declaration. It must not execute authored fieldmethod bodies or construct a model during module initialization. Every supplied top-level argument must satisfy RFC 084's declaration-safe preset-expression rules. The marked and unmarked spellings are equally supported. The LSP may expose an explicit code action to add or remove the marker, but must not diagnose or hint solely because one spelling was selected; the formatter must neither insert nor remove it. `new` and postfix finalization are executable and remain unavailable at module top level.

The omission rule applies only to fieldmethod chains. `X = Dog(paws=4)` invokes the ordinary model constructor and binds a completed `Dog`, while `X = Dog.paws(4)` preserves a model-constructor partial. A constructor-template partial therefore still requires its marker: `X = partial Dog(paws=4)`. This difference is intentional because an unmarked call already has established value-producing semantics, whereas an unmarked fieldmethod chain has no general associated-member meaning outside a construction context.

Inside executable code, fieldmethod argument expressions must be evaluated exactly once, left to right, when the construction expression captures them, matching RFC 084's local partial capture behavior. Under `partial`, authored fieldmethod bodies execute only when the resulting partial is finalized or ordinarily invoked. Under `new` or postfix finalization, they execute only after the complete plan has captured and merged its inputs.

Explicit RFC 084 syntax remains valid and interoperable:

```incan
Labrador = partial Dog(paws=4, coat_color="brown")
BlackLabrador = partial Labrador.coat_color("black")
```

`partial Model(field=value)` and `partial Model.field(value)` are two spellings of one construction-plan abstraction. They must converge on identical target identity, effective bindings, projected callable metadata, provenance, override behavior, and finalization. Under either `partial` or `new`, the target is exactly one constructor template or one fieldmethod chain. Mixed forms such as `partial Model(field=value).field(other)` and `new Model(field=value).field(other)` must be rejected.

The projected callable must retain every stored field not effectively preset as a required or defaulted keyword parameter according to the model constructor surface. Field-backed and computed inputs supplied by the partial remain optional keyword overrides, consistent with RFC 084. An unsupplied computed fieldmethod is optional because it does not represent required stored state.

### Binding merge and evaluation order

Each construction plan must retain at most one effective value for each canonical construction-input identity together with its capture and replacement provenance. Input collection is declarative: caller source order must not determine fieldmethod execution order.

When a later input targets the same canonical field-backed or computed fieldmethod identity as an earlier preset, the later input must replace the earlier effective value without moving that fieldmethod's model-defined execution slot. The superseded fieldmethod body must not execute. Its already-captured local argument value remains subject to ordinary evaluation and destruction rules; replacing an input does not retroactively skip evaluation that already occurred when a local partial captured it.

When the same fieldmethod identity appears more than once within one syntactic fluent chain, the compiler must emit a warning identifying the earlier and surviving calls. The warning must state that the earlier binding is replaced. It must not claim that the earlier argument expression was skipped, because executable local chains evaluate captured arguments left to right even when a later call supersedes the binding. Overriding an input inherited from a separately named or separately supplied constructor partial must not produce this warning.

Replacement must occur before fieldmethod execution, so an effective fieldmethod body executes at most once per finalized construction. Repeated calls to one fieldmethod must not accumulate, merge, append, or otherwise combine their arguments, regardless of the operations in the authored body.

At finalization, model defaults must initialize pending stored fields first. Effective field-backed fieldmethods must then execute in canonical stored-field declaration order. After every effective field-backed input has executed, effective computed fieldmethods must execute in their declaration order within the model. Distinct fieldmethods must all execute even when their declared write targets overlap; the compiler must not infer mutual exclusion, replacement, or precedence merely from target overlap. Assignment by one fieldmethod to a field that is subsequently assigned by an effective computed fieldmethod follows ordinary scheduled behavior: a direct assignment replaces the earlier value, while a computed update such as `+=` observes and updates the earlier value.

Argument expressions must still be evaluated exactly once in their ordinary left-to-right source order when an executable local partial or construction expression captures them. Capture order and fieldmethod execution order are separate: captured values are placed into the construction plan, then only effective fieldmethod bodies run according to the model-defined schedule at finalization.

Duplicate keywords within one constructor, partial argument list, or fieldmethod call must remain compile-time errors. The override rule applies between construction layers and fluent calls, not within a single argument list.

### Construction-context access

Assignment to `mod.field` inside a fieldmethod is a primitive construction-context binding operation. It must not recursively dispatch to that field's authored or synthesized fieldmethod. A fieldmethod may bind only its decorator-declared write targets. It may read any field on its owning model subject to the construction-state rules below; this is what lets a computed input normalize into multiple stored fields without granting an undeclared whole-model mutation surface.

Construction-context read safety is plan-insensitive and checkable from the model declaration. A read of `mod.field` is permitted only when at least one of these conditions holds:

- the field has a declared default, except that a field-backed fieldmethod may not read its own target before assigning it;
- the field is required, is not in the write-target set of any computed fieldmethod, and its field-backed execution slot precedes the current execution point; or
- the current fieldmethod has definitely assigned the field earlier on every control-flow path reaching the read.

All field-backed slots precede the computed phase. A computed fieldmethod may therefore read every defaulted field and every required field that no computed fieldmethod targets: successful finalization guarantees that such a required field was supplied through its field-backed input. A required field targeted by any computed fieldmethod is not considered definitely bound merely because an earlier computed slot could supply it; computed inputs are optional, so that reasoning would depend on the caller's plan. The method must assign such a field before reading it or the declaration is rejected. Within a field-backed method, the required-field rule applies only to earlier field-backed slots. These rules deliberately reject some conditionally safe programs rather than introduce a maybe-bound construction-state type.

An accumulator such as `mod.minutes += value` reads its target before writing it. A computed accumulator therefore requires that target to have a declared default unless the method definitely assigns it earlier on every path. A required field targeted by a computed fieldmethod cannot be used directly as an accumulator merely because some caller might also supply its field-backed input.

Construction-context state must never escape a fieldmethod, be stored in a model field, be returned as a first-class value, be passed to an unconstrained callable, or remain observable after finalization.

### `partial`, `new`, and postfix finalization

`partial` and `new` are contextual expression introducers when followed by a supported model construction target. `partial` preserves a plan; `new` finalizes one. `new` is not a universal allocation operator in this RFC.

`partial ModelName.field(value)` must open a reusable construction plan seeded with the model defaults and the supplied effective binding. `partial PartialName.field(value)` must seed that plan with the named partial's effective bindings before applying the new input. Without that marker, a fieldmethod chain produces a reusable value only when it is the complete right-hand side of a direct module-level assignment. This declaration-context shorthand does not apply inside functions, to nested expressions, or to constructor calls. Consequently, `Dog(paws=4)` constructs a value even at module scope, while `partial Dog(paws=4)` is required to preserve a call-form constructor partial.

`new ModelName` must open and finalize a construction plan seeded with the model defaults. `new PartialName` must do the same with the partial's effective bindings. A `new` expression may contain one fieldmethod chain or one direct constructor template, but not a constructor call followed by fieldmethod calls.

`ModelName.field(value)()` and `PartialName.field(value)()` are postfix-finalized fieldmethod chains. The terminating call must be empty. The typechecker checks the entire operand as a construction chain rather than first requiring the unmarked inner chain to have a standalone value. The result and behavior are identical to the corresponding `new` expression.

A newline followed by an indented leading `.` must continue the same construction expression through Incan's existing fluent-chain continuation grammar. The formatter must use four-space indentation for continued fieldmethod chains, preserve whether the source selected prefix or postfix finalization, and must not rewrite between `new` and `()` or insert or remove an optional top-level `partial` marker automatically.

The full syntactic expression boundary finalizes a `new` expression; the empty terminating call finalizes a postfix expression. Finalization must verify that every required field has an effective binding, apply checked field conversions and effective authored fieldmethods, invoke the target model's canonical constructor exactly once, and return exactly that constructor's result type.

An ordinary model's canonical constructor returns the model value. A model deriving `Validate` retains its generated associated constructor `TypeName.new(...) -> Result[TypeName, E]`; the contextual `new` expression and postfix finalizer must therefore return `Result[TypeName, E]` and must not expose or use forbidden raw construction.

Keyword `new` and the existing member name `.new` occupy structurally distinct syntactic positions. `new Model.field(value)` begins a construction expression because `new` precedes its target. `Model.new(field=value)` remains an ordinary associated validated-constructor call because `new` follows a dot. The parser cannot confuse those forms, and a computed fieldmethod named `new` is prohibited as a collision with the reserved constructor member.

A stored model field named `new` is legal only when the model has no ordinary associated callable named `.new`. When `Validate` is derived, its generated `TypeName.new(...)` constructor counts as such a callable, so the compiler must reject a stored field named `new` on that model. RFC 084 already permits a local partial over a dotted callable target; rejecting the collision preserves the existing meaning of `partial Model.new(...)` rather than making argument shape or construction context silently choose between the validated constructor and a synthesized fieldmethod. On a model without an associated `.new`, `partial Model.new(value)` remains the ordinary field-backed construction form.

Callable construction remains available. `Model(field=value)` and `PartialName(field=value)` invoke callables; `new Model.field(value)` and `new PartialName.field(value)` finalize fluent plans; `Model.field(value)()` and `PartialName.field(value)()` provide the equivalent postfix form. At module scope, `Name = Model.field(value)` and `Name = PartialName.field(value)` preserve fluent plans without changing the call semantics of `Name = Model(field=value)`. `new Model(field=value)` is permitted as the symmetric immediate-finalization form of `partial Model(field=value)`, although ordinary `Model(field=value)` remains the shorter spelling. Mixed syntax such as `new Model(field=value).field(other)` or `partial Model(field=value).field(other)` must be rejected.

Using `new` with a class, newtype, enum, arbitrary function partial, instance, or non-constructor callable must be rejected under this RFC.

### Direct constructor integration

Ordinary model construction through `ModelName(...)`, validated construction through `ModelName.new(...)`, invocation of an RFC 084 model-constructor partial, and every finalization form introduced here must route each effective construction input through the same authored or synthesized fieldmethod contract.

This requirement applies to direct keyword arguments, effective partial presets, field-backed inputs, and computed inputs. A computed fieldmethod introduces an optional constructor keyword of the same name, so `DateTimeInterval(hours=12)` and `new DateTimeInterval.hours(12)` use the same `hours` fieldmethod. The constructor keyword must have the computed fieldmethod's declared input type and no implicit default value beyond absence. This prevents a model author from defining normalization that fluent syntax enforces while ordinary construction bypasses it.

Stored defaults initialize the construction context directly before authored fieldmethods run; they are not fieldmethod invocations and therefore do not execute authored logic merely because the caller omitted a field. An explicit value for a defaulted field does invoke its field-backed fieldmethod. This distinction keeps a declared default stable while ensuring every caller-supplied value uses the authored construction contract.

Adding an authored fieldmethod may therefore change how future constructions of that model bind that field. This is an intentional source-level API change and must be reflected in checked API metadata and compatibility tooling.

### Deserialization and adapters

Decoding a stored model field is a construction path and must not bypass its authored field-backed fieldmethod. A schema adapter must construct a model in this order:

1. Resolve each present wire alias to its canonical stored-field identity.
2. Seed every omitted defaulted field directly from its declared default without invoking a fieldmethod.
3. Route every present decoded stored value through that field's authored or synthesized field-backed fieldmethod in canonical field order.
4. Run computed fieldmethods only when the adapter explicitly maps a wire input to that virtual construction input.
5. Assemble the concrete model and run whole-model validation once.

Computed fieldmethods are not wire fields merely because they are constructor keywords. An adapter may deliberately expose one, but the mapping must be explicit and must preserve its canonical fieldmethod identity and declared input type. Whether a private stored field participates in a wire schema remains governed by the model's existing derive, schema, and adapter rules; fieldmethod visibility neither includes nor excludes that field from serialization.

### Foreign and generated-Rust construction

Compiler-generated Rust representation is an implementation artifact rather than an alternative model-construction contract. A supported Rust-hosted or other foreign boundary that constructs an Incan model must enter through compiler-owned construction metadata and the canonical finalizer when raw construction would bypass an authored fieldmethod or whole-model validation. Generated code may assemble raw storage internally only after those contracts have run.

The backend may satisfy this rule with sealed generated fields, a generated construction function, a caller adapter, or another representation appropriate to the selected carrier. It must not expose a stable public struct-literal or tuple-constructor path that lets foreign callers bypass authored field-backed behavior. Models whose construction is behaviorally identical to direct field binding need not acquire unnecessary wrapper ceremony, but checked metadata must still distinguish that case from a model with authored construction behavior.

### Required fields and static checking

The compiler must reject immediate finalization when its statically known construction plan leaves a required field unbound. A fieldmethod call guarantees every declared target is bound because every normally completing path must commit each target. A computed fieldmethod may therefore satisfy required fields through its checked target set.

Runtime construction must not silently synthesize missing required values. If the compiler cannot prove that every declared target is bound on every normally completing path, the fieldmethod declaration itself must be rejected rather than degrading required-field checking at call sites.

Ordinary constructor-partial expressions may remain incomplete because their purpose is to produce a callable requiring the remaining fields. Tooling must display the projected required and optional keyword surface.

### Validation and newtype conversion

Field-backed fieldmethod call arguments must typecheck against the exact declared field type. Computed fieldmethod call arguments must typecheck against their explicitly declared input type. The compiler may insert RFC 017 validated-newtype coercion when the supplied expression has an approved underlying type and the call is an approved coercion site. It must not treat primitive widening, parsing, unrelated conversion traits, or arbitrary constructors as implicit fieldmethod coercion.

Checked newtype conversion must occur before the authored fieldmethod body observes the value. If implicit conversion fails, RFC 017's ordinary failure behavior applies. Authors requiring recoverable boundary diagnostics should perform explicit `from_underlying` conversion before supplying the typed result to a fieldmethod.

Whole-model validation must run only after all effective fieldmethods have completed and the concrete model value has been assembled. Fieldmethods must not bypass, replace, or suppress model validation. This RFC does not require the existing whole-model `validate` method itself to be construction-safe; its established effect rules remain unchanged, so the construction-safety guarantee covers fieldmethods and RFC 017 conversion hooks rather than every operation performed by a validated constructor.

### Construction-safe callables

A **construction-safe callable** is a reusable compiler classification for code that may participate in deterministic construction and validation. For fixed explicit inputs and compiler-known immutable facts, a construction-safe callable must be synchronous and deterministic and must have no externally observable effect other than returning a value, completing with an ordinary deterministic failure, or, for a fieldmethod, binding its declared fields on the current construction context. Mutation confined to local temporary values is permitted. I/O, filesystem or network access, clock access, randomness, process or environment access, reads from or writes to mutable global or `static` state, randomized-hashing dependence, asynchronous work, and other externally observable effects are not construction-safe. Reading compiler-known `const` values is permitted.

Builtin `dict` and `set` values have no iteration-order contract. Order-independent operations such as lookup, membership, length, and equality may remain construction-safe, but raw traversal and operations that expose their traversal order must be rejected in construction-safe code. This is a conservative operation-level rule, not a dataflow analysis and not an attempt to prove that one particular fold happens to be order-independent.

The sole traversal exemption is a compiler-recognized canonicalization form that directly consumes the unordered traversal and guarantees the same ordered result for every input permutation. A sort qualifies only when its element comparison is compiler-known to be total and deterministically distinguishes unequal elements. Stable sorting with a non-unique custom key does not qualify because ties retain the unordered input order; if such a keyed sorting API is added later, it must supply or infer a deterministic tie-breaker before the verifier can recognize it. `OrderedDict`, `OrderedSet`, `SortedDict`, and `SortedSet` retain their declared traversal contracts without an additional population-provenance rule.

Construction safety is inferred for Incan-bodied callables rather than asserted through a new source annotation. The typechecker must inspect the callable body and its resolved transitive callees. It must compute a summary for every exported callable as well as every fieldmethod, RFC 017 hook, and helper reachable from those roots. Recursive functions and mutually recursive call groups are classified as strongly connected call-graph components; recursion does not itself make a callable unsafe. A component is construction-safe only when its own operations are permitted and every call edge leaving it resolves to a construction-safe callable.

A direct function call, method call, or trait method call whose concrete implementation is statically selected may be classified by inspecting that implementation. A call through an open trait target, a dynamically unresolved callable, or another call site whose possible implementation set is not closed must be rejected in construction-safe code. This RFC does not introduce a trait-level construction-safety requirement; a future RFC may add one if open trait dispatch proves necessary.

Imported Incan callables must carry their inferred classification in checked package metadata. Extern functions, Rust-backed functions, and host-backed stdlib leaves have no Incan body available to the verifier and are construction-safe only when compiler-recognized interop metadata or the stdlib registry certifies them. Such certification is an explicit foreign-boundary contract, not a fact inferred from an opaque implementation. An opaque callable without that metadata is unverified and must be rejected from construction-safe code.

Every authored fieldmethod and every RFC 017 `from_underlying` hook must be construction-safe. This applies the same transitive enforcement mechanism to RFC 017's existing deterministic, side-effect-free validation contract; RFC 017's additional rule that `from_underlying` reports invalid input as `Err` rather than panicking remains unchanged. Construction safety is not totality: panic, assertion failure, arithmetic failure, and out-of-bounds indexing are deterministic and do not by themselves make a callable construction-unsafe. A fieldmethod introduces no typed validation or recovery channel of its own; deterministic runtime failures retain their ordinary language behavior, while domain validation and user-facing rejection diagnostics should normally use checked newtypes or whole-model validation.

Construction safety is also not compile-time evaluability. Const-evaluable code must satisfy the stricter const-expression rules in addition to being free of forbidden effects; a construction-safe runtime callable is not thereby accepted in a `const` initializer.

### Visibility, aliases, and composed fields

A public model may retain private stored fields. Model visibility does not promote each field to public and does not require callers to receive raw access to all construction state. A private field remains inaccessible outside its declaring model boundary for reads, patterns, direct keyword construction, aliases, and its synthesized or authored field-backed fieldmethod.

A computed fieldmethod uses its declared method visibility and may deliberately expose a public logical input whose exact declared targets include private stored fields. This is a controlled construction gateway owned by the model: callers can supply the public logical input but cannot name, read, pattern-match, or fluently bind the private targets themselves. A required private field may be satisfied by a declared default, an accessible computed fieldmethod, or model-owned construction code. A public model with no public route to all required private state is intentionally not externally constructible; the declaration remains valid because public visibility does not imply a public all-fields constructor.

Public constructor partials must not expose private fieldmethod surfaces or capture private preset values in ways RFC 084 forbids. A public partial may use a public computed fieldmethod that encapsulates private targets because its checked API records the public input and canonical private effects without exporting direct field access.

Fields introduced by RFC 087 structural model composition receive synthesized fieldmethods on the target model like locally declared fields. Authored fieldmethods, including computed fieldmethods, are not inherited from the spread source. If the target model wants authored construction behavior for a composed field, it must declare that behavior on the target model itself.

Aliases must continue to preserve canonical field identity, wire identity, diagnostics provenance, and privacy. Fluent aliases are code spellings only; they do not introduce additional pending fields.

### Diagnostics

The compiler must provide targeted diagnostics for at least:

- `@fieldmethod` used outside a model;
- bare `@fieldmethod` used on a method whose name is not a canonical stored field;
- parameterized `@fieldmethod(...)` used on the same-named stored-field input instead of bare `@fieldmethod`;
- missing, duplicate, aliased, or unknown decorator write target;
- write to a stored field outside the fieldmethod's declared target set;
- control flow that may complete without binding every declared write target;
- field-backed fieldmethod value parameter differs from the declared field type;
- computed fieldmethod name collides with a field alias or existing member;
- stored field named `new` collides with an ordinary associated `.new` callable, including the generated constructor on a model deriving `Validate`;
- computed fieldmethod lacks an explicit input type or explicit write-target list;
- malformed construction-context receiver or unsupported additional parameters;
- explicit return value;
- fieldmethod access through a completed model instance;
- inaccessible private fieldmethod surface;
- bare fieldmethod chain appears outside the permitted direct module-level assignment context and lacks `partial`, `new`, or a terminating empty call;
- postfix fieldmethod-chain finalization supplies arguments instead of an empty call;
- top-level `partial` construction contains an expression that is not declaration-safe;
- `new` or postfix finalization is used at module top level;
- `new` target is not a model constructor or model-constructor partial;
- one expression mixes a constructor template with a fieldmethod chain;
- immediate construction leaves required fields unbound;
- externally requested construction leaves a required private field unbound; the diagnostic must not instruct the caller to set the inaccessible field, must suggest any accessible computed inputs that can bind it, and otherwise must explain that the model exposes no public construction path for that required state;
- a model-constructor partial is used as a nominal type;
- unsupported construction-context escape or potentially unbound field read;
- collision between an authored fieldmethod and an ordinary member that is not the permitted field-backed field pairing;
- implicit conversion would require primitive widening, parsing, or a non-RFC-017 conversion;
- fieldmethod or RFC 017 `from_underlying` hook directly or transitively reaches a forbidden effect;
- raw traversal or an order-exposing operation on builtin `dict` or `set` appears outside a compiler-recognized total canonicalization form; the diagnostic must suggest a qualifying `sorted(...)` form or an appropriate ordered or sorted collection;
- construction-safe verification reaches an open trait dispatch or dynamically unresolved callable;
- extern, Rust-backed, host-backed, or imported callable lacks checked construction-safety metadata.

The compiler must additionally warn when one syntactic fluent chain supplies the same canonical fieldmethod identity more than once. The warning should label both calls and identify which binding survives.

Diagnostics should identify the target model, construction-input identity, construction layer that supplied or replaced the input, and canonical field or fieldmethod declaration when authored behavior is involved. A construction-safety diagnostic should identify the first unsafe or unverified operation and show the reachable call chain from the fieldmethod or RFC 017 hook to that operation.

## Design details

### Why fieldmethods are model-only

Models have a compiler-known data shape, keyword constructor surface, defaults, schema metadata, and no inheritance. Those properties make their pending field set statically projectable. Classes may also store fields, but they are behavior- and identity-oriented and may require initialization protocols that are not equivalent to declarative field binding. Restricting fieldmethods to models is therefore based on model construction semantics, not on the inaccurate claim that classes have no stored fields.

### Why the field and fieldmethod may share a name

The two surfaces occur in different semantic contexts. `dog.paws` reads a field from a completed `Dog`; `partial Dog.paws(4)`, `new Dog.paws(4)`, `Dog.paws(4)()`, and `Labrador = Dog.paws(4)` bind the canonical `paws` input inside recognized construction contexts. The compiler selects construction-context lookup before resolving the member and preserves canonical identity through lowering. Ordinary user-defined members do not gain a general permission to collide with fields. An unmarked `Dog.paws(4)` remains invalid inside executable code; as the complete right-hand side of a direct module-level assignment, it is valid and always preserves a constructor partial.

### Why fieldmethods declare write targets

The fieldmethod name identifies a logical construction input; its decorator arguments identify the stored representation that input is allowed and required to produce. Those identities are deliberately separate. In `@fieldmethod(minutes) def hours(...)`, a later `hours` input replaces an earlier `hours` input, while a distinct `minutes` input still executes even though both affect the same stored field.

Requiring an exact target set keeps construction behavior inspectable without inferring semantics from arbitrary method bodies. Authors declare only the fields the method writes, and every declared target must be bound on every normally completing path. The compiler can therefore reject accidental writes, prove which required fields become bound, expose useful API and editor metadata, and later enforce frozen bindings against canonical field identities. Canonical identifiers rather than strings preserve rename safety and avoid a Pydantic-style runtime field-name lookup surface.

### Why computed fieldmethods exist

Not every useful construction input should become stored state. A normalized interval may store days, seconds, and nanoseconds while accepting hours, minutes, milliseconds, and microseconds during construction. Requiring every fluent name to match a stored field would either make the motivating interface impossible or pollute the model's stable data shape with redundant units. Computed fieldmethods keep those inputs typed and model-owned while making their lack of instance storage and their stored-field effects explicit.

A computed fieldmethod remains narrower than a general builder method: it accepts one typed construction input, returns the construction context implicitly, cannot be called on completed values, and must bind its complete declared target set on every normally completing path.

### Why `mod` is not `self` or `cls`

`self` denotes an existing value receiver, and `cls` denotes a type-oriented receiver. A fieldmethod has neither: it operates on temporary construction state whose lifetime ends at finalization. A distinct `mod` receiver makes that boundary visible and prevents an incomplete model from masquerading as a valid instance.

### Why construction expressions are context-delimited

Fieldmethods deliberately do not appear on the model's general associated-member surface. `partial` states that the chain remains reusable, while `new` states that it produces one concrete result at the expression boundary. A terminating `()` provides the callable-shaped equivalent of `new` for an inline chain. A direct module-level assignment supplies a fourth context in which an unmarked fieldmethod chain always preserves a reusable plan. Its meaning follows from declaration position and never depends on whether the chain happens to bind all required fields.

Prefix markers give parsers, typecheckers, completion, and hover an early construction context. A direct module-level assignment also establishes its declaration context before its complete right-hand side is checked. The postfix form cannot provide that context to a left-to-right editor until its final `()` is present, but its completed syntax remains unambiguous to semantic checking. That tooling asymmetry is accepted in exchange for preserving the ordinary callable finalization spelling. Inside executable code, an unmarked and unterminated chain is rejected because no declaration context establishes reusable-plan intent.

### Why `new` is contextual

Many languages use `new` to mark construction, although they commonly construct before subsequent method calls. Here `new` marks the whole model-construction expression and finalizes it at the expression boundary. It does not promise heap allocation or object identity. The generated validated constructor `TypeName.new(...)` is an associated member reached after a dot, while contextual `new TypeName...` appears before its construction target. Their shared word creates conceptual overlap but no grammatical ambiguity, and both converge on the model's canonical validation behavior.

Incan already parses an indented leading-dot chain as continuation of its receiver expression, so multiline `new` expressions reuse an established layout rule rather than introducing a colon-delimited block or a new continuation form.

### Why constructor calls and fieldmethod chains do not mix

`partial Dog(coat_color="brown")` and `partial Dog.coat_color("brown")` are two spellings of one reusable plan. Their immediate counterparts are `new Dog(coat_color="brown")`, ordinary `Dog(coat_color="brown")`, and `new Dog.coat_color("brown")`. Within one marked expression, however, a target is either one constructor template or one fieldmethod chain. Allowing `new Dog(coat_color="brown").paws(4)` or `partial Dog(coat_color="brown").paws(4)` would mix both grammars, duplicate inputs, and blur whether the inner call constructs or presets. Callers choose one form per expression and may compose further through a named partial.

### Why later bindings replace earlier fieldmethod execution

RFC 084 treats presets as overrideable defaults. If an overridden fieldmethod still executed, normalization, assertions, or other construction logic for a value that does not survive could affect the final model. Replacing only the pending value while retaining the method's stable execution slot makes the semantic plan match the visible last-one-in-wins configuration story without turning caller order into execution order.

### Why execution order belongs to the model

Constructor partials describe effective named inputs, not an imperative operation log. If caller source order controlled fieldmethod execution, ordinary keyword order could change meaning and a reusable computed preset could fail merely because a required field is supplied only by its eventual caller. The model-defined schedule first establishes effective stored-field inputs, then applies computed construction logic in authored declaration order. Callers can therefore reorder fluent or keyword inputs without changing the constructed value.

Distinct fieldmethods may intentionally share targets. `hours`, `minutes`, and `weeks` can all contribute to one stored `minutes` field because the author explicitly declared that target on every method. Their execution order is the order of their declarations in the model, not the order chosen by each caller.

### Determinism and effects

Fieldmethods define repeatable model construction semantics, so their deterministic and effect-free behavior is a checked transitive property rather than a convention. The construction-safe callable classification is deliberately narrower than a general effect system: it answers only whether a callable may participate in deterministic construction and validation. It is broad enough to share between fieldmethods and RFC 017 hooks without claiming that construction-safe code is total or compile-time evaluable.

Ordinary deterministic failures remain possible. A panic, failed assertion, arithmetic failure, or out-of-bounds access is not an ambient input or externally observable effect merely because it interrupts construction. Fieldmethods have no separate typed error return, so authors should put domain validation and its diagnostics in checked newtypes or whole-model validation even though ordinary language failures retain their normal behavior.

### Comparison with Python and Pydantic

Python's [`dataclasses`](https://docs.python.org/3/library/dataclasses.html) generate an initializer from annotated fields. `InitVar` and `__post_init__` can accept construction-only inputs and derive stored fields, while [`functools.partial`](https://docs.python.org/3/library/functools.html#functools.partial) can publish a callable with overrideable preset keywords. These mechanisms can reproduce pieces of the proposed behavior, but they remain separate runtime conventions: the partial does not acquire a compiler-known field surface, `__post_init__` receives one completed initializer call rather than a typed fluent plan, and specialization is commonly expressed through wrappers or inheritance.

[Pydantic field and model validators](https://docs.pydantic.dev/latest/concepts/validators/) can validate or transform input and express cross-field checks. The decorator form uses string field names and class methods, field validators observe already validated fields in declaration order, and defaults do not run validators unless configured. Pydantic can also derive new runtime model types with [`create_model`](https://docs.pydantic.dev/latest/examples/dynamic_models/), including inheritance of validators and computed fields.

Fieldmethods are neither validators nor dynamic model types. Their role is to build one statically known pending model value: decorator targets are canonical compiler-resolved field identities, constructor partials remain callables returning the original model type, fieldmethods run before the model's canonical whole-model validation, and no inheritance or metaclass protocol is introduced.

### Interaction with RFC 084

RFC 084 remains the general callable-preset mechanism. This RFC adds a field-aware projection for model constructors and their partials. `partial Model(field=value)` and `partial Model.field(value)` must converge on the same construction-plan metadata, projected callable signature, override rules, and final constructor behavior. A call-form preset always requires the RFC 084 marker because bare `Model(field=value)` constructs a value. A fieldmethod chain may omit that marker only as the complete right-hand side of a direct module-level assignment. The same shorthand applies when extending an existing model partial: `BlackLabrador = Labrador.coat_color("black")` is legal at module scope and equivalent in plan contents to `BlackLabrador = partial Labrador.coat_color("black")`; inside executable code, the marker remains required.

General function, class, newtype, and method partials do not gain fieldmethods, postfix model finalization, or contextual `new`. This RFC intentionally specializes only the model-constructor target kind. Model-constructor partials retain RFC 084's ordinary projected callable type with richer checked metadata; this RFC introduces no new source-spellable nominal partial type.

### Constructor partials are code values

A model-constructor partial is a callable construction plan and may capture local values and authored construction behavior. This RFC does not give such plans a stable data schema or wire identity and does not make them automatically serializable. Applications that persist user-defined templates should represent those templates as ordinary models and reconstruct the appropriate construction plan or concrete value when executing them.

### Interaction with RFC 109

RFC 109's `tap` and `then` operate on completed receiver values. Fieldmethods operate on pending model construction. A fieldmethod chain must not resolve through general receiver combinators, and a completed model must not acquire its fieldmethod construction surface through `tap` or `then`.

### Compatibility and migration

The syntax is additive for models that do not declare members colliding with the new contextual forms. Every existing model gains synthesized construction metadata, but fieldmethods are visible only inside a recognized construction context and completed instance member resolution remains unchanged. A bare expression such as `Dog.paws(4)` remains invalid inside executable code when `paws` is only a field; the improved diagnostic explains the `partial`, `new`, and terminating `()` choices and notes that an unmarked reusable chain is legal only as the complete right-hand side of a direct module-level assignment.

An authored `@fieldmethod` changes all future construction paths for that field, including direct constructors and partial invocation. Adding, removing, changing, or reordering a fieldmethod in a way that changes its model-defined execution slot is therefore an API-significant change and should be visible to compatibility tooling.

The inferred construction-safety classification of an exported callable is also API-visible because downstream fieldmethods and RFC 017 hooks may rely on it without access to the callable's body. If a private helper gains logging or another forbidden effect, every exported callable that transitively reaches it may change from construction-safe to construction-unsafe. Checked package metadata and API-diff tooling must surface that change on the affected exported callable. When its source is available, local diagnostics should explain the responsible internal call chain; checked public metadata need not expose private declaration names. The private helper does not itself become a public export.

Checked package metadata produced before the construction-safety field exists must be treated as unverified rather than implicitly safe. A dependency must be rebuilt with compatible metadata before its exported callables can be used from construction-safe code.

Introducing contextual `new` may affect code that currently uses `new` as an identifier in the newly reserved prefix position. The lexer and parser must preserve ordinary identifier use outside that precise contextual form. Existing associated calls such as `TypeName.new(...)` are unaffected because the member name follows a dot and cannot be parsed as the contextual prefix.

A model that declares both a stored field named `new` and an associated `.new` callable becomes invalid. This includes a stored `new` field on a model deriving `Validate`. Authors must rename the field or associated callable; the compiler must not choose between RFC 084 callable partial application and fieldmethod construction from positional-versus-keyword argument shape.

## Alternatives considered

1. **Static factory methods named after fields.** A static factory can create an initial value but cannot preserve pending required-field state through a chain. It also overloads ordinary associated behavior for what is fundamentally field binding.
2. **Instance methods that copy or mutate a completed model.** This constructs too early, makes intermediate invalid states possible or forces defaults for required fields, and confuses construction with value mutation.
3. **RFC 084 partials only.** `partial Dog(paws=4)` remains sufficient for simple presets, but it does not provide authored per-field construction behavior or the intended readable fluent surface.
4. **Unmarked chains produce constructor partials everywhere.** Accepted only in the narrow declaration context. `Labrador = Dog.paws(4)` and `BlackLabrador = Labrador.coat_color("black")` are legal when each chain is the complete right-hand side of a direct module-level assignment, because that context unambiguously preserves a reusable declaration-safe plan. The broader alternative remains rejected: inside executable code or a nested expression, an unmarked chain would add fieldmethods to general member resolution and obscure whether the author intended reuse or finalization. The marked and unmarked module-level spellings are equally supported.
5. **Unmarked chains finalize immediately.** `Dog.paws(4)` could construct a value while `partial Dog.paws(4)` preserves a plan, but the unmarked form looks like an ordinary static call and makes the result change from error to execution without an explicit construction delimiter. The accepted postfix `Dog.paws(4)()` retains callable ergonomics while marking finalization.
6. **Require a final `()` or `.build()` as the only finalizer.** This is mechanically explicit and the empty-call form remains supported, but it is visually redundant in longer multiline recipes. `new` provides the finalization signal where readers first establish the expression's intent.
7. **Object-initializer braces or an indented colon block.** `new Dog { paws=4 }` or `new Dog: ...` would delimit construction clearly, but they create a second field-assignment grammar and do not naturally reuse callable constructor partials.
8. **Hand-written builder models.** Builders remain possible for protocols more complex than field binding, but requiring one for ordinary model specialization duplicates type and validation surfaces.
9. **Inheritance.** Subclasses can carry constructor defaults in some languages, but they also create nominal subtyping and inherited behavior. Constructor partials solve specialization without importing that hierarchy.
10. **Return a transformed value from each fieldmethod.** A user-written return type would let one call leave the target model's construction surface and would weaken static required-field tracking. The construction-context result is therefore implicit and fixed.
11. **Execute every overwritten fieldmethod in chain order.** This preserves a literal operation log but lets superseded defaults affect the result through hidden behavior. Effective-binding replacement better matches RFC 084's override semantics.
12. **Infer write targets from the method body.** Body inspection can detect assignments in a local implementation, but it makes the public construction contract implicit, weakens checked API metadata, and creates brittle behavior across helper calls and separate compilation. Decorator targets keep the permitted and guaranteed field effects explicit.
13. **Mix constructor templates and fieldmethod chains.** `new Dog(coat_color="brown").paws(4)` and its `partial` counterpart are superficially convenient, but duplicate the same construction inputs within two grammars and obscure whether the inner call is a constructor or a preset. One expression must choose one form.
14. **Allow arguments in the call that terminates a bare fieldmethod chain.** `Dog.paws(4)(coat_color="black")` would combine contextual postfix recognition with ordinary partial invocation and call-site overrides in one special form. It is rejected: the contextual postfix finalizer is exactly an empty `()`. Code that needs call-site overrides must first produce an actual partial value, for example `(partial Dog.paws(4))(coat_color="black")`, or invoke a named constructor partial.

## Drawbacks

- `new` introduces a new contextual expression, and marked construction expressions as well as unmarked module-level fieldmethod chains rely on the existing fluent newline-continuation rule.
- The same spelling denotes a field on completed values and a fieldmethod inside construction contexts, increasing the importance of precise contextual diagnostics and tooling.
- Model declaration order becomes semantically significant for fieldmethods that read or bind overlapping fields, so moving a field or computed fieldmethod may be an API-significant behavioral change.
- Fluent chains look method-like even though they collect declarative inputs and do not promise caller-ordered fieldmethod execution.
- Routing every construction path through authored fieldmethods makes those hooks powerful and API-significant; apparently local normalization changes can affect all callers.
- Keeping captured local argument evaluation separate from eventual fieldmethod execution requires the construction plan to preserve both values and source provenance.
- The word `new` can imply allocation or identity in other languages even though Incan models are values and this RFC promises neither.
- Prefix `new` and postfix `()` are two spellings of immediate fluent finalization, and postfix completion cannot expose the construction-only member surface as early as a prefix marker in a left-to-right editor.
- Moving an unmarked module-level fieldmethod-chain declaration into a function body requires adding `partial`; the targeted diagnostic and an explicit LSP code action should make that contextual requirement clear.
- Construction safety makes the behavior of every reachable helper relevant to a fieldmethod or RFC 017 hook; a seemingly private effectful change can therefore invalidate construction-safe callers and become visible through an exported callable's compatibility metadata.
- Static completeness becomes more complex if fieldmethods may conditionally inspect or bind other pending fields.

## Implementation architecture

This section is non-normative. A coherent implementation should represent marked fluent construction, declaration-context unmarked model partials, postfix finalization, RFC 084 model-constructor partials, direct constructor arguments, deserialization inputs, and supported foreign construction as one typed construction-plan abstraction. The plan should preserve the target model's canonical identity, captured argument values, effective fieldmethod inputs, capture provenance, replacement provenance, model-defined execution slots, fieldmethod and field identities, projected callable signature, and canonical finalizer.

Fieldmethod resolution should occur in the compiler frontend against canonical model and fieldmethod identity, with canonical field identity attached to field-backed methods. Lowering should consume resolved construction-plan operations rather than rediscovering fieldmethods from source names. The final backend representation may use a generated wrapper, builder-like temporary, or direct constructor expansion, provided it evaluates captured expressions exactly once, invokes only effective fieldmethods, constructs the model exactly once, and preserves the canonical validation result.

Checked API metadata should expose constructor partial provenance, authored versus synthesized fieldmethods, canonical write-target sets, projected required/defaulted fields, and whether the target uses ordinary or validated finalization. Tooling should consume the same metadata for completion, hover, signature help, and diagnostics.

Construction-safety verification should reuse the compiler's existing pattern of context-specific transitive validation rather than introduce a general effect system. The typechecker should build a resolved callable graph, classify recursive strongly connected components as units, reject direct forbidden operations, and propagate unsafe or unverified reasons through callers. Incan-bodied callables are inferred from that graph. Builtins and host-backed stdlib leaves use registry metadata; imported callables use checked package metadata; extern and Rust-backed leaves use compiler-recognized interop metadata. The registry and typechecker must classify raw unordered-collection traversal as unsafe and recognize only syntactically direct canonicalization forms with compiler-known total ordering and deterministic tie-breaking; it must not require a general order-taint analysis. Persisted metadata must be versioned and retain the public classification and foreign-certification provenance needed for downstream checking and API diffing. Local compiler state may retain private call-chain reasons for actionable diagnostics, but public metadata must not expose private declaration identities merely to justify a summary.

## Layers affected

- **Lexer / Parser / AST**: contextual `new`, extended and module-optionally omitted `partial`, direct module-level assignment construction context, postfix empty-call finalization, `@fieldmethod`, construction-context receivers, and model-constructor chains that reuse the existing indented leading-dot continuation surface.
- **Formatter**: stable multiline formatting for marked and declaration-context unmarked model partials, `new` construction expressions, and postfix finalization without rewriting between equivalent spellings or inserting or removing optional `partial`.
- **Typechecker / Symbol resolution**: construction-context fieldmethod identity, canonical exact write-target sets, field-backed and computed construction inputs, plan-insensitive read safety, exact field-type contracts, required-field projection, definite assignment, visibility, aliases, partial typing, override ordering, and RFC 017 coercion integration.
- **Construction-safety verification**: transitive callable-graph analysis, recursive-component classification, closed concrete trait dispatch, rejection of open or unresolved dispatch, operation-level rejection of unordered traversal with narrowly recognized total canonicalizers, and shared enforcement for fieldmethods and RFC 017 `from_underlying` hooks.
- **IR Lowering / Emission**: one declarative construction plan, superseded-binding elimination, exactly-once argument evaluation, model-defined fieldmethod scheduling, and canonical constructor finalization.
- **Validation / Derives**: preservation of `TypeName.new(...) -> Result[...]` for validated models, decoded-input routing through field-backed hooks, and whole-model validation after effective field binding.
- **Rust-hosted caller boundary**: canonical construction entrypoints for projected models whose authored fieldmethods or validation make raw native construction a semantic bypass.
- **Checked API / Compatibility metadata**: authored and synthesized fieldmethod surfaces, canonical write-target sets, constructor-partial signatures, inferred construction-safety summaries and reasons, foreign certification provenance, construction behavior provenance, and API-diff visibility.
- **LSP / Tooling**: fieldmethod completion on constructors and partials, field reads on instances, signature help, hover distinctions, rename by canonical field identity, targeted diagnostics, and explicit add/remove-`partial` code actions without unsolicited hints for either legal module-level spelling.
- **Documentation**: model construction, callable presets, validated models, newtypes, formatting, and migration guidance.

## Design Decisions

- Construction-context reads use a plan-insensitive rule. A fieldmethod may read a defaulted field, a required field established by an earlier field-backed slot and not targeted by any computed fieldmethod, or a field it has definitely assigned on every path before the read. Required computed targets are never assumed bound merely because a caller might supply them, and accumulators therefore need a default or an earlier definite assignment.
- Model-constructor partials retain RFC 084's ordinary projected callable type. This RFC adds checked construction-plan metadata but no source-spellable nominal partial type.
- Repeated construction inputs use last-binding-wins replacement. Frozen constructor bindings are deferred; the canonical plan representation must preserve enough field identity and provenance for a later RFC to add them without changing this RFC's callable surface.
- Construction safety is a reusable compiler classification, not a general effect system and not a synonym for const evaluability or totality.
- The compiler infers construction safety transitively from Incan bodies without a new opt-in source annotation. Recursive call groups are classified together rather than rejected merely for containing cycles.
- A statically selected concrete function, method, or trait implementation may participate when its transitive body is construction-safe. Open trait dispatch and dynamically unresolved call targets are rejected; this RFC does not add a trait-level safety requirement.
- Imported Incan callables carry inferred summaries in checked package metadata. Opaque extern, Rust-backed, and host-backed callables require compiler-recognized interop or stdlib-registry certification and are rejected when that certification is absent.
- Deterministic runtime failures remain permitted by the construction-safe classification. Fieldmethods add no typed recovery channel, and RFC 017 retains its stricter `Err`-based invalid-input contract.
- Builtin unordered collections remain unordered. Raw traversal is rejected in construction-safe code unless a syntactically direct, compiler-recognized canonicalizer guarantees an input-order-independent result through total ordering and deterministic tie-breaking; no general order-taint analysis is introduced.
- Whole-model `validate` methods retain their existing effect contract. Applying construction-safety verification to validators is deferred rather than silently widening this RFC's compatibility impact.
- A stored field named `new` is rejected when its model also has an associated `.new` callable, including the generated constructor on a `Validate` model. This preserves RFC 084 dotted-callable partial semantics without argument-shape disambiguation.
- Fieldmethods and RFC 017 `from_underlying` hooks use the same construction-safety verifier. A safety change to an exported callable is API-visible and must be reported by compatibility tooling.
