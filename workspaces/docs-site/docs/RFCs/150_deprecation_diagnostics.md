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

Propose compiler-visible deprecation metadata for declarations and fields: optional since and planned retirement text, checked replacement identity, migration message and typed diagnostic severity. Editor guidance, compiler use diagnostics, bounded runtime warnings and generated documentation consume the same checked facts. Replacements remain guidance rather than an automatic rewrite, and version text does not remove code automatically. The exact richer field spelling remains open.

## Motivation

A replacement API can exist for months while callers continue using an older name because the migration is recorded only in a changelog. A declaration-carried diagnostic connects the use directly to the reason and replacement guidance without requiring the compiler to guess a rewrite.

## Goals

- Warn at an Incan source location that refers to a deprecated declaration.
- Preserve the information through imports and compiled library metadata.
- Make editor and documentation explanations agree with compiler diagnostics.
- Distinguish human guidance from a checked, safe automatic replacement.

## Non-Goals

- Automatically removing declarations or changing their application semantics; the explicitly selected runtime warning is observability, not substitution.
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


@deprecated(since="0.7", replacement=checked_limit, message="use checked_limit and handle its Result")
pub def unchecked_limit(value: int) -> int:
    return value
```

A different module calling `unchecked_limit(-1)` receives a warning at that call, with the declaration location and message. The program still calls the old function. A compiler must not rewrite it to `checked_limit(-1)`: that would change the result type and introduce a new handling obligation.

Compiler use diagnostics inside the defining module remain quiet so the library can migrate its own implementation incrementally. This does not exempt an executed deprecated operation from the selected first-execution runtime warning. This exemption does not follow a re-export into a new module. An alias of the deprecated declaration still refers to its original identity unless a separately defined compatibility wrapper creates a new declaration.

Fields must reuse RFC 085 deprecated bool/text metadata as the compact deprecation spelling. Authored field accesses and explicit constructor bindings consume the same checked descriptor as documentation. The descriptor must also carry optional since, retire_in and checked replacement facts; its exact richer field spelling remains open. Metadata traversal and serializer wire keys must not be treated as a new authored field reference for every record.

## Reference-level explanation

- Optional since, retire_in and message must be compile-time strings. Optional replacement must be a checked declaration reference, retained as canonical identity rather than a string-dispatch key. retire_in is an advisory plan, not an automatic removal trigger.
- Editor hover and explicit deprecated imports/references must expose guidance, including a squiggle. Compiler diagnostics apply to authored consuming uses; an unused import does not by itself imply runtime execution.
- External consuming references must warn by default. Optional severity uses the compiler-recognized std.diagnostics.DeprecationSeverity cases Warning and Error. An author-declared Error rejects the use before execution. Project warning-as-error policy may strengthen Warning and must not silently weaken an author Error.
- The first executed deprecated canonical declaration/member use in one process must produce one runtime warning; later executed uses deduplicate by canonical identity. Import alone, dead code and compile-time-only metadata must not produce runtime warnings. Warnings identify symbols and safe guidance, never argument values or secrets.
- Diagnostics must identify canonical declaration identity, authored use and declaration locations, supplied message, version guidance and checked replacement when present. A local alias/re-export route must remain inspectable; generated forwarding must not duplicate one authored warning. A separately authored wrapper has its own explicit policy.
- Import aliases must not erase the warning. A re-export must not reset the defining module's identity.
- Version text must not cause automatic removal or alter callable resolution.
- Diagnostics and generated documentation must consume the same checked facts.
- An automatic replacement must not be offered solely because a message contains another symbol's spelling. A checked replacement reference must identify the target in diagnostics alongside the supplied migration message. It must not be treated as proof of equal arguments, effects or result types, and must not authorize a tool to invent error recovery.
- Field warnings must distinguish authored field access from unrelated runtime mapping keys and wire names.

## Design details

Rust's [deprecation attribute](https://doc.rust-lang.org/reference/attributes/diagnostics.html) is useful prior art for a compiler-visible warning with explanatory metadata. Incan should additionally expose this information as structured diagnostics so an agent can locate and assess the migration rather than parse prose alone.

Report the consuming authored reference once when elaboration creates several internal accesses. Editor import guidance, compiler use diagnostics and first-execution runtime warnings describe different events and must remain distinguishable in structured output. Reflection, generated adapters and schema metadata traversal must not manufacture source-use warnings.

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

## Design Decisions

- Optional since, retire_in, message and checked replacement metadata share one descriptor. Version guidance is advisory; neither a retirement version nor a replacement identity automatically changes or removes code.
- Existing field deprecated bool/text metadata drives authored source-use guidance and documentation. The descriptor carries richer facts without a second metadata authority; exact richer field spelling remains for review below.
- Editor import/reference guidance, compiler consuming-use warnings and one runtime warning per executed canonical identity per process are distinct. Imports alone, dead code and compile-time-only metadata do not execute deprecated code. Diagnostics must preserve secrecy.
- std.diagnostics.DeprecationSeverity.Warning is the default; Error rejects the use at compilation. Project policy may strengthen warnings without weakening an author-declared Error.
- Aliases and re-exports retain canonical policy and an inspectable authored route. Generated references deduplicate at their responsible source operation; a real wrapper has its own declaration policy.

- A checked replacement and migration message must be shown together; neither authorizes an automatic rewrite when call contracts differ.

## Unresolved questions

- What exact richer field metadata spelling carries since, retire_in and checked replacement alongside the existing deprecated bool/text compact form? The shared descriptor and authored-use behavior are settled.

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
