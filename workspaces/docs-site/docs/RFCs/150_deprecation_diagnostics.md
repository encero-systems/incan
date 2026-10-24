# RFC 150: Deprecation diagnostics

- **Status:** Draft
- **Created:** 2026-10-17
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 024 (symbol aliases)
    - RFC 036 (declaration decorators)
    - RFC 082 (checked API documentation)
    - RFC 085 (field metadata)
    - RFC 120 (canonical source identity)
- **Issue:** [#1288](https://github.com/encero-systems/incan/issues/1288)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Propose `@deprecated(since=..., message=...)` for declarations, with a warning at external use sites and structured deprecation information for tools and generated documentation. Both arguments are optional. Field deprecation must share checked field metadata rather than introduce an unrelated annotation system. Structured replacement and per-declaration error escalation remain design questions.

## Motivation

A replacement API can exist for months while callers continue using an older name because the migration is recorded only in a changelog. A declaration-carried diagnostic connects the use directly to the reason and replacement guidance without requiring the compiler to guess a rewrite.

## Goals

- Warn at an Incan source location that refers to a deprecated declaration.
- Preserve the information through imports and compiled library metadata.
- Make editor and documentation explanations agree with compiler diagnostics.
- Distinguish human guidance from a checked, safe automatic replacement.

## Non-Goals

- Removing the declaration or changing its runtime behavior.
- Treating version text as an automatic removal deadline.
- Applying unchecked textual replacement as an editor fix.
- Introducing a second field-metadata authority.

## Guide-level explanation

The decorator is proposed. Both functions below are application-defined and fully specified.

```incan
pub def checked_limit(value: int) -> Result[int, str]:
    if value < 0:
        return Err("limit must be non-negative")
    return Ok(value)


@deprecated(since="0.7", message="use checked_limit and handle its Result")
pub def unchecked_limit(value: int) -> int:
    return value
```

A different module calling `unchecked_limit(-1)` receives a warning at that call, with the declaration location and message. The program still calls the old function. A compiler must not rewrite it to `checked_limit(-1)`: that would change the result type and introduce a new handling obligation.

References inside the defining module are proposed to remain quiet so the library can migrate its own implementation incrementally. This exemption does not follow a re-export into a new module. An alias of the deprecated declaration still refers to its original identity unless a separately defined compatibility wrapper creates a new declaration.

For fields, RFC 085 already has `deprecated` metadata for data/schema descriptions. Whether that metadata also drives source-use warnings, or requires an additional checked language facet, remains open. This draft deliberately does not present an undecided field decorator as accepted syntax.

## Reference-level explanation

- `since` and `message`, when supplied, must be compile-time strings.
- External references to an annotated declaration must produce a warning by default, subject to the accepted diagnostic policy.
- The warning must identify canonical declaration identity, use location, declaration location, and supplied guidance.
- Import aliases must not erase the warning. A re-export must not reset the defining module's identity.
- Version text must not cause automatic removal or alter callable resolution.
- Diagnostics and generated documentation must consume the same checked facts.
- An automatic replacement must not be offered solely because a message contains another symbol's spelling.
- Field warnings must distinguish authored field access from unrelated runtime mapping keys and wire names.

## Design details

Rust's [deprecation attribute](https://doc.rust-lang.org/reference/attributes/diagnostics.html) is useful prior art for a compiler-visible warning with explanatory metadata. Incan should additionally expose this information as structured diagnostics so an agent can locate and assess the migration rather than parse prose alone.

Recommended behavior is to report the consuming reference, with deduplication where one source reference is elaborated into several internal accesses. Whether an unused import alone counts as a use needs a clear rule. Warnings for reflection, generated adapter accesses, and schema consumers should not be invented by treating every internal metadata traversal as a source reference.

## Alternatives considered

- Changelog-only deprecation: useful context, but disconnected from the actual caller.
- Free-form replacement strings with automatic application: unsafe when argument binding, effects, or return types differ.
- Immediate removal: sometimes deliberate, but offers no migration interval.

## Drawbacks

Warnings can become noisy across heavily used compatibility APIs. Defining-module suppression can hide internal migration debt. Field deprecation spans language references and data-schema expectations, which need an explicit shared contract.

## Layers affected

- **Declaration / field metadata:** stable deprecation facts.
- **Name resolution / diagnostics:** external use checks and deduplication.
- **LSP / documentation:** visible guidance and checked replacement affordances if adopted.

## Unresolved questions

- What is the exact field spelling, and does RFC 085's schema metadata also request source-use diagnostics?
- Does an import alone warn, or only a subsequent consuming reference?
- Should authors be able to request an error for a specific deprecated item?
- Is a checked replacement expression part of this RFC or a separate tooling extension?
- How should generated references and aliases with their own compatibility policy be reported?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
