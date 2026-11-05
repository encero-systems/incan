# RFC 108: Import-scoped extensions

- **Status:** Planned
- **Created:** 2026-06-06
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 027 (`incan-vocab` block registration and desugaring)
    - RFC 028 (trait-based operator overloading)
    - RFC 046 (computed properties)
    - RFC 050 (enum methods and trait adoption)
    - RFC 058 (`std.datetime` temporal values, intervals, and runtime timing)
    - RFC 068 (protocol hooks for core language syntax)
    - RFC 109 (`tap` and `then`)
    - RFC 110 (`Ordering.then`)
    - RFC 122 (construction-safe callables)
    - RFC 128 (callable types)
    - RFC 129 (binding and `mut` parameters)
    - RFC 160 (vocabularies, which add syntax)
    - RFC 164 (explicit ownership contracts)
- **Issue:** [#2662](https://github.com/encero-systems/incan/issues/2662)
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

A library can add members to a type it does not own through a named `extension` block, and those members exist only where that block is imported: `3.days`, `256.mib`, `3.days.from_now()`, `order.status.badge`. An extension holds ordinary typed `def` and `property` declarations with bodies. Nothing reopens the type, and a member the type authors itself always wins. A property must be pure and anything that does work is a method, so the parentheses at a use site tell the reader which is which.

## Core model

1. **A block, by name.** `pub extension temporal_units:` declares a group of members that are exported and activated together.
2. **Importing activates.** `from m import temporal_units` makes its members available in that module, and a dependency merely being present does nothing. The standard prelude can activate an extension, which is how one reaches every module.
3. **Methods are `def` with a typed `self`.** `def from_now(self: TimeDelta) -> DateTime:` names its receiver, and `mut self: T` says a method changes it.
4. **Properties name their receiver with `on`.** `property days on int -> TimeDelta:` reads like a field at the use site, so its body must be pure under RFC 122's construction-safe classification.
5. **Receivers are ordinary types.** A concrete type, an instantiated generic, or a type parameter with or without bounds. There is no separate predicate language.
6. **Nothing is reopened.** A body sees only what its module can see, a type's own member wins, and an extension adds no state, trait adoption or operator hook.
7. **Everything is static.** A read resolves at compile time against the extensions active where it is written.

## Motivation

Constructor calls such as `TimeDelta.days(3) + TimeDelta.hours(12)` are clear and stay valid, but they are heavier than the idea they express. `3.days + 12.hours` says the same thing and is just as typed, provided the compiler can see exactly where `days` comes from.

The same shape is useful beyond temporal units. `256.mib` carries a byte size instead of a raw integer. `"/users/{id}".route` turns a constant string into a checked route pattern. `order.status.badge` is an admin presentation of an order status, and it belongs to the admin layer rather than to `OrderStatus`, a type that knows nothing about badges. It is also what lets `tap` and `then` from RFC 109 be library code.

A library has no way to write any of this today. It can publish a free function, `days(3)`, which reads inside-out and loses the receiver in completion, or wrap the value in a type of its own, which every caller then has to unwrap. Letting a library change a type globally, as Ruby does, would be the wrong fix: behavior would depend on what happens to be loaded, and any property read could hide a database query. The fluent shape is worth having only if it stays explicit, typed and scoped.

## Goals

- Let a library add typed properties and methods to any type, active only where imported.
- Keep a type's own members first, so an extension never changes what existing code means.
- Have the compiler check what a property may do.
- Let an `enum` declare its own properties, as `model` and `class` bodies can, so an extension is never the only way to give a type a property.
- Let the prelude activate an extension, so library code can be available everywhere with no compiler knowledge of its names.
- Show at every use where a member comes from.

## Non-Goals

- Changing what a type is: no new fields, stored state, trait adoption or operator hooks.
- Adding syntax. New keywords, blocks and DSLs belong to RFC 160's vocabularies.
- Setter properties, which RFC 046 also excludes.
- A dimensional-analysis type system.
- Defining standard units. Temporal, byte-size, rate and route types belong to whatever owns those types, and the module names below are examples.
- Legislating taste. `404.not_found` compiles, and it is still a poor API.

## Guide-level explanation

### Write an extension

```incan
from std.datetime import DateTime, TimeDelta

pub extension temporal_units:
    property days on int -> TimeDelta:
        return TimeDelta.days(self)

    property hours on int -> TimeDelta:
        return TimeDelta.hours(self)

    def from_now(self: TimeDelta) -> DateTime:
        return DateTime.utc_now() + self

    def ago(self: TimeDelta) -> DateTime:
        return DateTime.utc_now() - self
```

Every member names its receiver. In a `property`, it follows `on`. In a `def`, it is the type of `self`. One block can cover several types, as this one covers `int` and `TimeDelta`, because the block is about a subject rather than a type.

### Use it

```incan
from std.datetime.units import temporal_units

retry_window = 3.days + 12.hours
deadline = 3.days.from_now()
reminder = (3.days + 12.hours).from_now()
```

Without the import, `3.days` is refused, and the diagnostic names the extension that provides it:

```text
`int` has no member `days`
help: `temporal_units` provides it: from std.datetime.units import temporal_units
```

A member read binds tighter than a unary operator, as it does everywhere, so `-3.days` is `-(3.days)`.

### Properties compute, methods act

A property reads like a field, and a reader should be able to trust that. Its body must be pure, and the compiler checks this with the classification RFC 122 already uses for model construction: synchronous, deterministic and free of external effects, all the way through what it calls.

```incan
pub extension suspicious:
    property users on int -> list[User]:      # refused: reads a database
        return db.users.limit(self)

    property from_now on TimeDelta -> DateTime:      # refused: reads the clock
        return DateTime.utc_now() + self
```

Both are refused with a diagnostic pointing at the call and suggesting `def`. So in `3.days.from_now()`, `days` is a value and the parentheses on `from_now()` show that something happens. A test that needs a fixed clock writes `anchor + 3.days`.

### Any type can be a receiver

```incan
pub extension byte_units:
    property kib on int -> ByteSize:
        return ByteSize.kibibytes(self)

    property mib on int -> ByteSize:
        return ByteSize.mebibytes(self)

pub extension rate_units:
    property percent on float -> Rate:
        return Rate.percent(self)

    property bps on int -> Rate:
        return Rate.basis_points(self)

pub extension admin_ui:
    property badge on OrderStatus -> Badge:
        match self:
            OrderStatus.Pending => return Badge.warning("Pending")
            OrderStatus.Paid => return Badge.success("Paid")
            OrderStatus.Cancelled => return Badge.muted("Cancelled")

pub extension ordered_lists:
    property in_order[T with Ord] on list[T] -> list[T]:
        return sorted(self)
```

```incan
upload_limit = 256.mib
fee_rate = 2.5.percent
badge = order.status.badge
names = customer_names.in_order
```

A receiver is checked the way an argument is checked against a parameter, so existing conversions apply and no new ones appear. That makes a string literal a valid `FrozenStr` receiver and refuses a runtime `str`, as it would for any `FrozenStr` parameter:

```incan
pub extension route_literals:
    property route on FrozenStr -> RoutePattern:
        return RoutePattern.parse_const(self)
```

```incan
from std.web.routes import route_literals

users = "/users/{id}".route

const HEALTH: FrozenStr = "/health"
health = HEALTH.route

path: str = read_path()
dynamic = path.route                  # refused: `route` needs a FrozenStr
parsed = RoutePattern.parse(path)?    # a runtime string can fail, so it has its own call
```

### Changing the receiver

`mut self` works as it does on any parameter under RFC 129, including an unshared temporary. RFC 109's `tap` is written this way:

```incan
pub extension chain:
    def tap[T](mut self: T, observer: Callable[(mut T) -> None]) -> T:
        observer(self)
        return self
```

### When names collide

A type's own member always wins, and that is not a conflict:

```incan
cmp(a, b).then(cmp(c, d))    # Ordering's own `then`, not the one in std.chain
```

An imported extension wins over one the prelude activates, so a new prelude member cannot break existing code. Between imported extensions, the one with the more specific receiver wins. If neither is more specific, the read is refused with both named, and you say which you mean by calling it through its extension, with the receiver first:

```incan
from std.datetime.units import temporal_units
from billing.periods import billing_units

temporal_units.days(3)
billing_units.days(3)
```

Nothing depends on import order.

### Should this be an extension?

If you own the type and the behavior is part of what the type is, make it a member of the type. An extension is for an overlay: a layer adding words about a type that should not have to know about that layer. `badge` belongs in an `admin_ui` extension only if badges are an admin concern.

A type's own property is written inside the type, and an `enum` works the same way as a `model` or a `class`:

```incan
enum OrderStatus:
    Pending
    Paid
    Cancelled

    pub property is_open -> bool:
        match self:
            OrderStatus.Pending => return true
            _ => return false
```

## Reference-level explanation

### Declaration

1. `extension name:` must declare an extension in a module's top level. `pub` must export it under the ordinary visibility rules, and an unmarked extension must be usable only in its module. `extension` and `on` must be contextual: outside these positions, they remain ordinary identifiers.
2. An extension body must contain only `def` and `property` members and an optional docstring. Fields, constants and nested declarations must be refused.
3. Members must take the visibility of their extension, and a member marked `pub` must be refused. Importing an extension activates all of it, so a helper is an ordinary function in the module.
4. A member named as a hook, such as `__add__` or `__iter__`, must be refused. Operators and protocol syntax stay with the type, under RFC 028 and RFC 068.

### Members

5. The first parameter of a `def` member must be `self` or `mut self` with a type annotation, and that type is the member's receiver. The member is otherwise an ordinary function declaration: type parameters, overloads, `async`, decorators and docstrings work as they do elsewhere.
6. A `property` member must have the form `property name on Receiver -> Result:` or `property name[T] on Receiver -> Result:`, with `self` of type `Receiver` in its body. It is otherwise an RFC 046 property: no parameter list, no setter and no `()` at a read. `self` must be read-only.
7. Bodies must be checked as ordinary Incan. `self` must give no access beyond what the declaring module can see, so a receiver's private members stay private.

### Purity of properties

8. A property body must be construction-safe under RFC 122, inferred through its transitive call graph. A body that is not must be refused with a diagnostic naming the offending call and suggesting `def`. Purity must not be assertable by annotation.
9. A property read in a `const` initializer must be valid exactly when its body is const-evaluable under the ordinary rules, so a failing `const USERS = "/users/[".route` fails the build.
10. A `def` member must have no such restriction.

### Receivers

11. A receiver must be a concrete type, an instantiated generic such as `list[int]` or `Result[T, E]`, or a type parameter of the member with or without bounds. Any type may be a receiver, including one from another Loaf or a foreign one.
12. A receiver expression must be checked against `self` as an argument is checked against a parameter, with the same conversions, literal rules and ownership rules under RFC 129 and RFC 164. A string literal therefore reaches a `FrozenStr` receiver and a runtime `str` does not.
13. A numeric literal followed by `.` and an identifier must read as a member of that literal: `3.days` reads `days` on the integer `3`, and `2.5.percent` reads `percent` on the float `2.5`. A `.` followed by a digit continues a float literal.

### Activation

14. An extension must be active in the module that declares it, wherever its name is imported, including under an alias, and in every module if the standard prelude activates it. Importing only the module it lives in must not activate it.
15. A dependency being present or loaded must not activate anything.
16. A read must resolve against the extensions active where it is written. A caller's imports must not affect a function it calls, including a generic one, and a re-export must make an extension importable without activating it.
17. An extension reached through two import paths must count as one extension and must not conflict with itself. Two different extensions imported under one name follow the ordinary import conflict rules.

### Resolution

18. For `receiver.name`, the checker must check the receiver and then look for `name` in this order, stopping at the first step that finds an applicable member:
    1. the receiver type's own members, including those from adopted traits;
    2. members of extensions active through declaration or import;
    3. members of extensions the prelude activates.
19. A shadowed extension member must not be reported as a conflict at the read.
20. Within a step, the applicable member with the most specific receiver must win. A receiver is more specific than another when every type it accepts is accepted by the other, and not the reverse: `int` over `T`, `T with Ord` over `T`, and `FrozenStr` over `str`. If no member is most specific, the read must be refused with a diagnostic naming the competing extensions. Import order, dependency order and load order must never decide.
21. A receiver whose static type is a type parameter must resolve against its bounds at the generic function's definition, not when it is instantiated.
22. Extension members must not count toward a trait bound, a hook or an adoption.
23. `extension_name.member(receiver, ...)` must call that member directly, whatever the steps above would select, and a property takes its receiver as its only argument.
24. Reading a method without calling it must follow the ordinary bound-method rules of RFC 128. `()` after a property must give RFC 046's property-called-as-method diagnostic.
25. A read must evaluate the receiver once and then run the member. Dispatch must be static, with no runtime member lookup and no string dispatch.

### Diagnostics and tooling

26. Diagnostics must cover a missing activation, naming an extension in the dependency graph that fits; an ambiguous read, naming the extensions and how to call one by name; a receiver mismatch, including a runtime `str` at `FrozenStr`; an impure property; and a hook, field or `pub` member in an extension.
27. A published Loaf must carry its extensions and their members, so consumers can resolve them without source.
28. Completion, hover and go-to-definition must show the extension a member comes from and where it is declared. `incan inspect` must report which extensions are active in a file, and generated documentation must list an extension's members with their receivers.

### Properties on enums

29. An `enum` body must accept `property` members under RFC 046's rules, as `model` and `class` bodies do, including one that implements a property an adopted trait requires. A variant named `property` must stay valid, since `property` is contextual.

### Conformance workloads

| Workload | Required evidence |
| --- | --- |
| Activation | A member works with the import, under an alias and through a re-export, is refused without one, and is refused when only its module is imported. |
| Scope | A dependency's extension changes nothing until imported, and a caller's imports do not change a function it calls. |
| Receivers | Primitives, literals, models, enums, instantiated generics, foreign types and bounded and unbounded type parameters all work. |
| Literals | `3.days`, `2.5.percent` and `"/x".route` resolve, `-3.days` is `-(3.days)`, and a runtime `str` is refused at `FrozenStr`. |
| Purity | Bodies reading a clock, a database or a mutable global are refused, including through a callee in another Loaf. |
| Const | A const-evaluable property works in a `const`, and a failing one fails the build. |
| Resolution | An own member wins, an import wins over the prelude, a more specific receiver wins, a tie is refused, and reordering imports changes nothing. |
| By name | `extension_name.member(receiver)` reaches a shadowed member and resolves a tie. |
| Isolation | A body cannot read a private field, and an extension member does not satisfy a trait bound. |
| Refusals | A hook, a field and a `pub` member are refused with their diagnostics. |
| Mutation | `mut self` changes a mutable binding and an unshared temporary, under RFC 129. |
| Enum properties | An enum declares its own property and implements one a trait requires, and a variant named `property` still works. |

## Design details

### Why `extension` and not `vocab`

RFC 160's vocabularies add syntax: keywords, blocks and DSLs, registered with `library_vocab()` and lowered by a desugarer that runs outside the compiler. An extension adds typed members with ordinary compiled bodies, and it changes nothing in the parser. They only look alike from the consumer's side, because importing activates both. One word for two mechanisms would send a search for "how do I write a vocab" to two different answers and make a diagnostic that says "vocab" ambiguous. Prose can still call `temporal_units` a units vocabulary.

### Why properties must be pure

A method call says that something runs, and a field-shaped read does not. On a type's own property, the reader can look at the type, and that property's author is responsible for the type. An extension's property comes from an import, on a type that may be `int`, so a read gives even less of a hint. Reusing RFC 122's inferred classification means no new effect system and no annotation that can lie. The cost of being refused is two characters of `()`.

### Why `on` for properties and `self` for methods

A method has a parameter list, so its receiver goes there. RFC 046 refuses a parameter list on a property, because a property read takes no arguments, so its receiver needs its own place. `on` reads well and keeps RFC 046's rule intact. `mut self: T` needs no new syntax, and `def` needs no second keyword, since a `def` is a named callable wherever it appears.

### Why the prelude comes last

An import is a choice written in the module, and the prelude is a default. Ranking the choice first also means that adding a prelude extension member, such as a new one in `std.chain`, cannot make a working module ambiguous.

### Prior art

Rails' Active Support is where the reading style comes from, with `1.month.ago` and durations that own `ago` and `from_now` ([ActiveSupport::Duration](https://api.rubyonrails.org/classes/ActiveSupport/Duration.html)). It gets there by adding methods to `Numeric` at runtime, so loading a gem changes every caller. Rust supplies the safety model: an extension trait applies only where imported, an inherent method wins, and `Trait::method(x)` names one explicitly. Kotlin has extension properties, import scoping and members that win. Dart's `extension Name on Type` is the closest in shape. Swift and C# use the same word. None of them check that an extension property is pure, which is what this RFC adds.

### Compatibility

This RFC is additive. Code that imports no extension is unaffected, and `extension` and `on` remain usable as identifiers. Constructor forms and explicit parsers stay valid and remain the clearer choice where a value is dynamic or a call can fail.

## Alternatives considered

- **Constructor functions only:** explicit and already available, but noisy for the common cases, and it encourages raw values.
- **Extensions active once a dependency is present:** the nicest use site, but behavior would depend on the dependency graph.
- **`vocab` as the keyword:** see "Why `extension` and not `vocab`".
- **Rust-shaped `impl Trait for Type`:** a second conformance grammar, and no field-like read.
- **`method name on T (...)`:** matches `property ... on` at the cost of a keyword for what `def` already says.
- **`property days(self: int) -> TimeDelta:`:** matches `def`, but breaks RFC 046's rule and reads like a call.
- **A surrounding `for int:` block:** hides the receiver on each member, and a block about a subject often covers several types.
- **A metadata table such as `unit_vocab` with `constructor = ...`:** a parallel description of what a body already says, with room to lie about purity.
- **String-based units such as `unit(3, "days")`:** loses checked names, completion and refactoring.
- **Clock-reading properties, such as `3.days.from_now`:** it would need a second rule beside purity, and the read would hide that the value depends on when it runs.
- **Author-declared purity:** an assertion the compiler already has to verify, so inference is enough.
- **Aggregate receivers, such as `(3.days, 12.hours).from_now()`:** `3.days + 12.hours` already composes through an ordinary operator.
- **Private members inside an extension:** a block activates as a whole, and a module function already does a helper's job.

## Drawbacks

Member lookup gains steps, and the source alone does not show where a member comes from, so this relies on tooling to surface it. Behavior can scatter across imports when authors ignore who owns a type. Purity rules out effects but not poor taste: `property json on str -> Json` is deterministic and passes, even though it hides a slow parse that can fail, so documentation still has to steer authors toward a method. Because a type's own member wins silently, a dependency adding a member can take over a read that used to reach an extension; calling the extension by name is the way to pin one.

## Layers affected

- **Parser / AST:** `extension` blocks, `property ... on`, `property` in `enum` bodies, and a numeric literal followed by a member read.
- **Typechecker / Symbol resolution:** receiver checking, activation, the resolution order, specificity and the purity check.
- **IR Lowering / Emission:** static calls with no runtime member lookup, and evaluation order.
- **Standard library:** prelude activation, and RFC 109's `std.chain` as the first extension it activates.
- **Library packaging:** published extensions and their members.
- **Formatter:** `extension` blocks, formatted like other declarations.
- **LSP / Tooling:** completion, hover, go-to-definition, `incan inspect` and generated documentation.
- **Documentation:** a reference page for extensions and guidance on choosing between a member, a method and a property.

## Design Decisions

- **Keyword:** `extension`. RFC 160's vocabularies own "vocab", because they add syntax and extensions add members.
- **Member forms:** `def` with a typed `self` for methods, and `property name on Receiver -> Result:` for properties.
- **Visibility:** members take their extension's visibility, and a helper is a module function.
- **Receivers:** ordinary types, instantiated generics and type parameters with or without bounds, with no predicate language.
- **Purity:** a property must be construction-safe under RFC 122, inferred rather than annotated. A `const` read follows the ordinary const rules.
- **`from_now` and `ago`:** methods, because they read the clock: `3.days.from_now()`.
- **Literal receivers:** checked like arguments, so a string literal is a valid `FrozenStr` receiver.
- **Aggregates:** no tuple rule. `3.days + 12.hours` composes.
- **Resolution:** own member, then declared or imported extensions, then the prelude, with the most specific receiver winning inside a step and ties refused.
- **Naming a member directly:** `extension_name.member(receiver, ...)`.
- **Prelude activation:** the standard prelude can activate an extension, with no compiler knowledge of its names.
- **Enum properties:** an `enum` body accepts `property`, so an extension cannot do for an enum what its own author cannot. A type's own property keeps RFC 046's rules and is not required to be pure.
- **Rust companion crates:** none. RFC 160 removed them, and an extension is ordinary Incan.
