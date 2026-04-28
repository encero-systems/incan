# RFC 012: `JsonValue`, enum methods, and enum trait adoption

- **Status:** Superseded
- **Created:** 2025-11-15
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 050 (enum methods and enum trait adoption), RFC 051 (`JsonValue`)
- **Issue:** [#80](https://github.com/dannys-code-corner/incan/issues/80)
- **RFC PR:** —
- **Written against:** v0.1
- **Shipped in:** —

## Summary

This RFC originally bundled two separable concerns: a general language feature for enum methods and enum trait adoption, and a specific stdlib proposal for `JsonValue`. That coupling proved counterproductive because the language feature and the library type have different design maturity and should be reviewable independently.

## Superseded by

- RFC 050 — Enum methods and enum trait adoption
- RFC 051 — `JsonValue` for `std.json`

## Reason for supersession

The language capability and the dynamic JSON type can now move on separate tracks:

- RFC 050 covers the general enum-language feature.
- RFC 051 covers the dedicated dynamic JSON proposal.

Keeping the old combined proposal active would blur that boundary and make future references harder to interpret.
