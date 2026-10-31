# RFC 149: Non-exhaustive public contracts

- **Status:** Draft
- **Created:** 2026-10-17
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 029 (unions and narrowing)
    - RFC 087 (complete model composition)
    - RFC 120 (canonical identity)
    - RFC 122 (canonical model construction)
    - RFC 131 (closed error families)
    - RFC 137 (named enum payloads)
- **Issue:** [#1287](https://github.com/encero-systems/incan/issues/1287)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Propose `@non_exhaustive` as an explicit public API evolution contract for enums and models. External enum matching must allow unknown future alternatives; direct construction of known enum cases remains possible. Model construction restrictions are a separate concern: an external caller should use an author-owned factory when future required fields must remain evolvable. The exact ownership boundary and the model pattern/composition contract remain open.

## Motivation

A library may need to add an alternative without every consumer claiming to know all alternatives forever. Adding required model fields has a related but different compatibility problem. Combining these into one blanket ban on enum construction makes common known-case construction unnecessarily difficult and obscures what each restriction protects.

## Goals

- Make incomplete external knowledge explicit at match sites.
- Preserve ordinary construction of known enum cases.
- Give model authors a deliberate evolution boundary.
- Export the contract through checked package metadata and diagnostics.

## Non-Goals

- Making all public declarations open by default.
- Adding enum inheritance, dynamic variants, or runtime schema discovery.
- Silently relaxing RFC 131's closed error-family guarantees.
- Treating a wildcard match as authorization to ignore an error.

## Guide-level explanation

The decorator below is proposed; the enum and application function are fully supplied.

```incan
@non_exhaustive
pub enum PaymentMethod:
    Card
    BankTransfer


def describe_payment(method: PaymentMethod) -> str:
    match method:
        PaymentMethod.Card => return "card"
        PaymentMethod.BankTransfer => return "bank transfer"
        _ => return "another supported payment method"
```

For a caller outside the eventual ownership boundary, the wildcard is required even if the two known cases cover today's declaration. Such a caller may still construct `PaymentMethod.Card`. The wildcard does not permit values that violate the enum's checked representation.

For a model, an author-owned factory can preserve the public entry point while the record gains fields:

```incan
@non_exhaustive
pub model RetrySettings:
    attempts: int
    delay_ms: int


pub def default_retry_settings() -> RetrySettings:
    return RetrySettings(attempts=3, delay_ms=100)
```

Under the proposed model restriction, an external caller uses `default_retry_settings()` rather than spelling every field in `RetrySettings(...)`. The factory itself runs inside the ownership boundary. This example chooses a separate named function; it does not grant `.new` a bypass of RFC 122's canonical construction rules.

## Reference-level explanation

- External exhaustive matching on an annotated enum must be refused unless an unconditional catch-all covers future cases.
- A guarded wildcard must not count as unconditional coverage.
- Annotation of the enum itself must not prohibit external construction of its known cases.
- The proposed model restriction must reject external direct construction, including aliases of the canonical constructor. It must not be bypassed by spelling `.new`.
- Author-owned factories must still perform the canonical normalization and validation obligations of RFC 122.
- Checked metadata must preserve annotation identity and the owning boundary. A source import alias must not change either.
- Visibility restrictions must continue to apply independently.
- Applying this decorator to dedicated error declarations must be refused unless a separately accepted contract reconciles it with RFC 131's closed error sets.

## Design details

Rust distinguishes [non-exhaustive enum matching from struct construction](https://doc.rust-lang.org/reference/attributes/type_system.html). This draft follows that distinction rather than the proposal issue's blanket external-construction restriction. This is a recommendation for review, not an independently settled user choice.

A module boundary makes internal refactoring affect public construction permissions; a package boundary is more stable for a published library but requires canonical package ownership. The draft recommends evaluating package ownership first. Model patterns need an explicit open-field form before this decorator can promise safe field evolution. RFC 087 composition likewise must not import an unknowable future complete contract by assuming today's fields are permanent.

## Alternatives considered

- Ban every external enum constructor: protects more than the stated evolution problem requires and makes known alternatives cumbersome to use.
- Make every declaration non-exhaustive: changes the language's ordinary closed-data model.
- Permit a public constructor alias to bypass the restriction: defeats the model evolution contract; an author-owned factory supplies a real boundary instead.

## Drawbacks

Adding the annotation later restricts existing consumers and is itself a compatibility change. Model construction, patterns, serialization, and full-contract composition need coordinated rules; a match-only implementation would be incomplete.

## Layers affected

- **Declaration metadata / identity:** annotation and canonical ownership boundary.
- **Typechecker:** match coverage, model construction, composition checks.
- **Schema / tooling:** open-contract information, diagnostics, and editor guidance.

## Unresolved questions

- Is the ownership boundary a module or a package, and how are local and foreign declarations treated?
- What open-field model pattern syntax preserves compatibility with future fields?
- Can complete-contract composition consume an annotated model, and with what evolution guarantee?
- Should variant-level non-exhaustiveness be supported separately from enum-level annotation?
- How should JSON and schema adapters represent evolution without weakening current validation?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
