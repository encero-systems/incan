# RFC 149: Non-exhaustive public contracts

- **Status:** Planned
- **Created:** 2026-10-17
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 029 (unions and narrowing)
    - RFC 048 (checked API metadata)
    - RFC 082 (generated API documentation)
    - RFC 084 (callable presets)
    - RFC 087 (complete model composition)
    - RFC 120 (canonical identity)
    - RFC 122 (canonical model construction)
    - RFC 127 (lint catalog)
    - RFC 131 (closed error families)
    - RFC 137 (named enum payloads)
    - RFC 143 (clone with field overrides)
    - RFC 146 (typed JSON decoding)
    - RFC 147 (`except` blocks)
    - RFC 150 and RFC 151 (`@deprecated` and `@final`)
- **Issue:** [#1287](https://github.com/encero-systems/incan/issues/1287)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

`@non_exhaustive` lets a library say that a public enum, error family or model will grow. Outside the Loaf that owns it, a `match` must include a catch-all arm even when it lists every current case, and a model cannot be built directly: consumers use the author's factory, or clone an existing value. Inside the Loaf nothing changes, so the author's compiler still lists every internal site to update. Adding a case or a field is then not a breaking change.

## Core model

1. **It is a statement about the future.** A program is compiled against one version of a library, so every value holds a case that version declares. The catch-all covers source evolution across versions, not unknown values at run time.
2. **The boundary is the Loaf.** Code in the Loaf that owns the declaration, including its tests, is unaffected. Everything else is foreign.
3. **Enums stay constructible.** A known case can always be built, and only matching is affected.
4. **Models stay readable.** A foreign consumer reads fields and matches on them, and only direct construction is refused.
5. **It is a compile-time rule.** Nothing changes at run time, or in how data is decoded.

## Motivation

A library exports an enum with `Card` and `BankTransfer`, and a consumer writes a `match` listing both. The compiler accepts it as exhaustive. When the library adds `Wallet` in a minor release, every consumer stops compiling. The same happens to a model: a new field with no default breaks every consumer's `RetrySettings(...)`. Authors have no way to say which of their types are expected to grow.

## Goals

- Let a library add enum cases, error cases and model fields without breaking consumers.
- Make incomplete knowledge explicit at a consumer's `match`.
- Keep exhaustive checking inside the owning Loaf.
- Export the contract through checked metadata, diagnostics and generated documentation.

## Non-Goals

- Making public declarations open by default.
- Enum inheritance, dynamic variants or runtime schema discovery.
- Changing how JSON or other formats decode.
- A marker on a single enum case.
- Treating a catch-all as permission to ignore an error.

## Guide-level explanation

`@non_exhaustive` is proposed by this RFC.

### An enum that will grow

```incan
# In the payments Loaf
@non_exhaustive
pub enum PaymentMethod:
    Card
    BankTransfer
```

An application that depends on it has to allow for cases it does not know about:

```incan
from pub::payments import PaymentMethod

def describe(method: PaymentMethod) -> str:
    match method:
        PaymentMethod.Card => return "card"
        PaymentMethod.BankTransfer => return "bank transfer"
        _ => return "another payment method"
```

Without the `_` arm, the compiler refuses the `match` and says why. The application can still build `PaymentMethod.Card`. When the library adds `Wallet`, this code keeps compiling, and a `Wallet` value takes the `_` arm.

Inside the `payments` Loaf, including its tests, a `match` listing `Card` and `BankTransfer` stays exhaustive, and adding `Wallet` makes the compiler list every site the author needs to update.

### A model that will grow

```incan
# In the retry Loaf
@non_exhaustive
pub model RetrySettings:
    pub attempts: int
    pub delay_ms: int

pub def default_retry_settings() -> RetrySettings:
    return RetrySettings(attempts=3, delay_ms=100)
```

An application reads the fields and matches on them, but cannot build the model directly:

```incan
from pub::retry import default_retry_settings

settings = default_retry_settings()
print(settings.attempts)
faster = settings.clone(delay_ms=10)
```

`RetrySettings(attempts=3, delay_ms=100)` here is refused. `settings.clone(delay_ms=10)` is fine, because it starts from a complete value, so a future field comes along with it. Record patterns already name only the fields they care about, so `RetrySettings(attempts=3) =>` keeps matching after a field is added.

This differs from private fields, which already reserve construction but also hide the data. `@non_exhaustive` gives the missing combination: fields anyone can read, with construction reserved.

### An error family that will grow

An HTTP client's error family gains a case when it adds TLS options:

```incan
# In the http Loaf
@non_exhaustive
pub error RequestError:
    TimedOut(url: str)
    Refused(url: str)
```

A consumer's `match` needs a catch-all, and so does anything that relies on having handled every case. In an RFC 147 `except` block, listing `TimedOut` and `Refused` still leaves future cases to propagate, so the function must keep returning a `Result` that admits them, unless it has a catch-all arm.

### A case whose payload will grow

There is no marker for one enum case. A case whose payload may grow carries a `@non_exhaustive` model:

```incan
pub enum BuildOutcome:
    Finished(report: BuildReport)
    Failed(message: str)
```

External code reads `report.size`, matches `Finished(report=r)`, and gets reports from the library's functions.

### Know when a new case arrives

A catch-all also hides a new case. A program labeling each payment method falls through to its generic label after an upgrade, with no warning. An opt-in lint in RFC 127's catalog flags a known case that only a catch-all handles, so an author who wants that review can have it.

## Reference-level explanation

### Targets and boundary

1. `@non_exhaustive` must apply to `pub` enums, `pub` models and `pub` error declarations. On a declaration that is not `pub`, or on any other declaration kind, it must be refused with a targeted diagnostic.
2. A use is local when the using code belongs to the Loaf that owns the declaration, across its modules and including its tests. Every other use is foreign, including from a sibling package in a workspace, because siblings are versioned separately.
3. A re-export must not move ownership, and an import alias must change neither the annotation nor the owner.
4. Local uses must be unaffected in every respect.

### Matching

5. A foreign `match` over an annotated enum or error family must have an unconditional catch-all for it, `_` or a binding, even when every current case has an arm. A guarded catch-all must not count. The rule must apply at any depth, such as inside an `Option`, a tuple or a payload.
6. Forms that do not claim coverage, such as `if let` and RFC 134's `let ... else`, must be unaffected.
7. A composed family that includes a foreign annotated family, such as `error Combined = RequestError | LocalError`, must need the same catch-all, and so must a family that extends one.
8. An RFC 147 `except` block over a foreign annotated family must count future cases in its remainder unless it has an unguarded catch-all arm.

### Construction

9. A known case of an annotated enum or error family must remain constructible by foreign code.
10. Foreign code must not construct an annotated model directly. The refused routes are `RetrySettings(...)`, RFC 122 `new RetrySettings.method(...)` chains, RFC 084 presets of those and aliases of any of them.
11. RFC 143's `value.clone(field=x)` must remain available, subject to field visibility, because it starts from a complete value.
12. A factory the author provides must run RFC 122's normalization and validation like any other local construction.

### Reading and patterns

13. Foreign code must keep reading visible fields and matching record patterns on an annotated model. Record patterns are named-only and leave unnamed fields unconstrained, so adding a field must not affect them.
14. Field and declaration visibility must continue to apply independently.

### Composition

15. `model Target from Source:` under RFC 087 must be refused when `Source` is a foreign annotated model, because `Target` would change shape on a dependency upgrade and could collide with a local field. The diagnostic must point to reusable `field` declarations. Local composition must be unaffected.

### Decoding and schemas

16. The decorator must not change decoding. RFC 146's `unknown_fields` remains the consumer's choice for data from a newer producer, and a document naming an unknown case remains a decode failure. Each version's generated schema remains exact.

### Metadata and tooling

17. Checked metadata must record the annotation and the owning Loaf under RFC 048, and generated documentation must show the contract under RFC 082.
18. A diagnostic must name the declaration and its owning Loaf and offer the catch-all or the factory. Hover must show the annotation.
19. RFC 127's catalog must include an opt-in lint flagging a known case of a foreign annotated enum or error family that only a catch-all handles.

### Conformance workloads

| Workload | Required evidence |
| --- | --- |
| Foreign match | Two Loaves: a `match` without a catch-all is refused, with one is accepted, and a guarded catch-all is refused, at top level and nested. |
| Local match | A `match` in the owning Loaf and its tests stays exhaustive, and adding a case produces its errors. |
| Evolution | A consumer compiled against a newer library version keeps compiling after a case or field is added, and a new case takes the catch-all. |
| Enum construction | Foreign code builds a known case. |
| Model construction | Each refused route is refused, a factory works, and `clone(field=x)` works. |
| Patterns | A record pattern keeps matching after a field is added. |
| Errors | A foreign `match`, a composed family and an `except` block each require their catch-all. |
| Composition | `from` over a foreign annotated model is refused, and over a local one is accepted. |
| Boundary | A workspace sibling is foreign, and a re-export does not move ownership. |

## Design details

### Why the Loaf

A module boundary would force an author to put catch-alls on their own internal matches, which is exactly where exhaustiveness pays off most. It would also make moving code between modules change public permissions. A Loaf is what gets published and versioned, so it is the unit that can evolve independently of its consumers. Rust draws the same line at the crate.

### Why errors are included

RFC 131 already says that adding a case to a public closed family can require downstream matches to change. The annotation does not reopen the family: nobody else can add cases, each compiled version's set is closed, and `extends` still leaves the parent unchanged. It makes that existing risk an explicit contract.

### Prior art

[Rust's `#[non_exhaustive]`](https://doc.rust-lang.org/reference/attributes/type_system.html) goes on structs, enums and variants with the crate as its boundary. Its functional update `..base` is refused on foreign non-exhaustive structs, which `clone(field=x)` avoids here. Rust's `std::io::ErrorKind` uses it for the same reason errors are included here. Swift's `@unknown default` (SE-0192) is a catch-all that still warns about unhandled known cases, and Zig's `_` prong does the same, the behavior this RFC offers as a lint. Kotlin, Scala and Dart have sealed hierarchies with no opt-out, and Java 21 adds a runtime default that throws for constants added under separate compilation.

## Alternatives considered

- **Refuse foreign construction of enum cases:** protects more than the problem requires and makes known cases cumbersome to use.
- **Make every declaration non-exhaustive:** changes the language's ordinary closed-data model.
- **Module as the boundary:** see "Why the Loaf".
- **Treat workspace siblings as local:** they are versioned separately, so they can see a new case before they are updated.
- **Refuse it on error families:** leaves libraries with the same problem and no answer.
- **An open-field pattern form:** record patterns already leave unnamed fields unconstrained.
- **A marker on one enum case:** a case can already carry a `@non_exhaustive` model, so a second rule would add nothing.
- **New syntax for a checked catch-all, such as `@unknown`:** a lint gives the same review with no new syntax.
- **Letting `from` consume a growing model:** a dependency upgrade would silently change the shape of the consumer's model.
- **Decoding unknown tags into a catch-all:** a wire-format concern, which belongs with RFC 146.

## Drawbacks

Adding the annotation to an existing public type is itself a breaking change, because consumers must add catch-alls, while removing it is not. A catch-all can hide a new case that deserved its own handling, which the lint only partly answers. Consumers of an annotated model depend on the author providing a good factory. A model's field set stops being a contract that `from` can rely on across Loaves.

## Layers affected

- **Parser and declarations:** the decorator and its targets.
- **Typechecker:** foreign match coverage, including composed families and `except` remainders, construction routes and composition.
- **Checked metadata and documentation:** the annotation and its owner.
- **Lints:** the opt-in known-case lint.
- **Language server:** hovers and diagnostics.

## Design Decisions

- **Boundary:** the Loaf, including its tests. Workspace siblings are foreign, and re-exports do not move ownership.
- **Targets:** `pub` enums, models and error families. Each version's family stays closed, and the annotation makes evolution across versions explicit.
- **Matching:** foreign matches need an unguarded catch-all at any depth, as do composed families and `except` remainders. Known cases stay constructible.
- **Models:** foreign code cannot construct directly, through any route. It uses a factory or `clone(field=x)`, and keeps reading and matching. Record patterns need no new form.
- **Composition:** `from` over a foreign annotated model is refused, pointing to reusable `field` declarations.
- **Single cases:** no case-level form. A case whose payload grows carries an annotated model.
- **Decoding:** unchanged. RFC 146 owns the wire-level choice.
- **Hidden new cases:** an opt-in lint in RFC 127's catalog, with no new syntax.
