# RFC 151: Final classes

- **Status:** Draft
- **Created:** 2026-10-17
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 036 (declaration decorators)
    - RFC 099 (class behavior reuse and trait contracts)
    - RFC 120 (canonical source identity)
    - RFC 132 (frozen classes)
- **Issue:** [#1289](https://github.com/encero-systems/incan/issues/1289)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Propose `@final` on class declarations, making any attempt to extend the annotated class a compile-time error. Trait adoption and ordinary construction remain available. This is a restriction on behavior reuse and overriding, independently of frozen instance state and independently of subtyping.

## Motivation

A class may maintain invariants that rely on its methods operating together. Its author should be able to state that those behaviors are not extension points. Incan's `extends` supports behavior reuse rather than subtype substitution; the restriction should describe that actual model rather than import a polymorphic-inheritance rationale from another language.

## Goals

- Give class authors an enforceable no-extension contract.
- Diagnose extension at the source declaration that requests it.
- Preserve the rule across aliases and package boundaries.

## Non-Goals

- Adding subtyping through `extends`.
- Freezing fields or preventing instance mutation.
- Restricting trait adoption or composition through fields.
- Adding per-method finality or sealed-by-default classes.
- Adding model inheritance.

## Guide-level explanation

The decorator is proposed. The trait and class are application declarations, supplied here rather than imported from an invented package.

```incan
trait Counted:
    def count(self) -> int: ...


@final
class Counter with Counted:
    value: int

    def count(self) -> int:
        return self.value

    def advance(mut self) -> None:
        self.value += 1


class SpecialCounter extends Counter:
    pass  # Rejected: Counter explicitly prohibits extension.
```

The class can implement Counted and mutate its own value through `mut self`. `@final` does not mean `@derive(Frozen)`. A separate class may hold a Counter as a field and delegate to its methods without extending it.

## Reference-level explanation

- The decorator must be accepted on class declarations only.
- An `extends` target resolving to a final class must be rejected, including through aliases or imported metadata.
- The diagnostic must show the extending declaration and the final class's declaration or exported contract.
- Construction, visibility, method calls, and trait adoption must retain their ordinary rules.
- A class that extends an extensible base may itself be final.
- Finality must not imply a new subtype relationship or make an existing one disappear.
- Applying this decorator to a model, enum, function, or trait must produce a target-specific diagnostic rather than silently doing nothing.

## Design details

Finality is a checked declaration property, not a backend-only attribute. Adding it to an existing public base intentionally breaks consumers that extend that base; documentation should state that compatibility consequence. It does not justify changing an unannotated class's extension policy.

An annotated class may adopt traits with required fields or constants under RFC 144. Those adopted contracts remain checked normally. A final class's methods can still be trait implementations; preventing class extension must not erase that capability.

## Alternatives considered

- Documentation only: cannot enforce the invariant at an extension site.
- Sealed-by-default classes: a broader language policy change than the explicit opt-in requested here.
- Frozen classes: controls instance reassignment, not behavioral extension.

## Drawbacks

Authors can overuse finality and make legitimate reuse harder. Adding the marker later is a compatibility change for subclassers. Composition remains available but can require explicit forwarding methods.

## Layers affected

- **Declaration metadata:** finality across checked library boundaries.
- **Typechecker / resolution:** extension-target checks.
- **LSP / documentation:** extension diagnostics and class-contract display.

## Design Decisions

- This draft proposes an explicit class-level opt-in; unannotated classes keep their existing extension rules.
- Finality and frozen instance state are independent contracts.
- Trait adoption and composition through fields remain available.
- Foreign interop types keep their existing extension rules; this RFC does not introduce foreign class extension.
