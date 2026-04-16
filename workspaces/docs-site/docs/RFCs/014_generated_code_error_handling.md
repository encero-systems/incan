# RFC 014: user-facing runtime error behavior for generated code

- **Status:** Rejected
- **Created:** 2025-12-01
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 013 (Rust crate dependencies)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/81
- **RFC PR:** —
- **Written against:** v0.1
- **Shipped in:** —

## Summary

This document is no longer tracked as an active RFC. The underlying problem is real, but it does not belong in the RFC track: it is primarily compiler/runtime implementation work about how generated programs surface errors, not a standalone language or stdlib governance proposal.

## Motivation

The repo's RFC process is for user-facing language, tooling, runtime-surface, and documentation commitments. RFC 014 drifted into a compiler-maintenance and code-generation design note. Keeping it active as an RFC would weaken the boundary between governance documents and implementation work.

The actual work remains important:

- compiled Incan programs should not surface raw backend panic text for ordinary user-facing failures;
- strict operations should continue to have strict semantics;
- intentionally fallible companion APIs should remain explicit and typed.

That work is now tracked directly in issue `#81`.

## Guide-level explanation (how users think about it)

There is no longer an active RFC-level language proposal here. If future work on runtime error behavior grows into a distinct user-facing contract that genuinely needs governance, it should return as a new RFC with a narrower and more clearly user-facing scope.

## Reference-level explanation (precise rules)

RFC 014 is rejected as an RFC artifact. The implementation and design work for this topic lives in issue `#81`.

## Alternatives considered

### Keep RFC 014 active

Rejected. The topic matters, but the document is mostly about generated-code behavior, backend error surfacing, and compiler/runtime cleanup rather than a durable user-facing standards commitment.

### Supersede RFC 014 with another RFC

Rejected for now. There is not yet a separate replacement RFC because the work is better tracked as a normal implementation issue.

## Drawbacks

- Some historical design discussion moved out of the RFC corpus and into issue tracking.
- If a future user-facing runtime error model needs a real standards decision, a new RFC will need to be written from scratch rather than reviving this one.

## Layers affected

- **RFC process / docs** — RFC 014 remains as a historical record but is no longer part of the active RFC set.
- **Issue tracking** — issue `#81` is the authoritative home for the remaining work and design notes.
