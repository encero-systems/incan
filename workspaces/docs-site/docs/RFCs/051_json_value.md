# RFC 051: `JsonValue` for `std.json`

- **Status:** Draft
- **Created:** 2026-04-06
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 024 (extensible derive protocol)
    - RFC 025 (multi-instantiation trait dispatch)
    - RFC 050 (enum methods and enum trait adoption)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/335
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC proposes `std.json.JsonValue` as Incan's dynamic JSON value surface for unknown or partially known JSON structures. It is intended to complement, not replace, model-based JSON handling by giving users a standard type for parse-inspect-transform workflows where the schema is not fully static.

## Core model

1. `JsonValue` represents the standard JSON value space: null, bool, number, string, array, and object.
2. `JsonValue` supports parsing from JSON text and serializing back to JSON text.
3. `JsonValue` provides typed inspection and extraction helpers so dynamic JSON code remains explicit about runtime shape checks.
4. The exact representation strategy and direct indexing contract remain open design questions in this Draft.

## Motivation

Model-driven JSON handling is good when the schema is known. It is not enough for several real cases:

- dynamic APIs that return different shapes depending on context;
- exploration and prototyping before a schema is stable;
- partial parsing where only a few fields matter;
- mixed static/dynamic payloads where some fields are well-typed and others are intentionally open.

Without a dedicated dynamic JSON type, users either over-model fluid payloads or fall back to ad hoc dictionaries and unclear conventions.

## Goals

- Provide a dedicated `JsonValue` type under `std.json`.
- Support parse/serialize and explicit runtime inspection of dynamic JSON values.
- Coexist cleanly with typed model-based JSON workflows rather than displacing them.
- Leave enough room to choose the right underlying representation without overcommitting too early.

## Non-Goals

- Replacing typed JSON derive flows for stable schemas.
- Turning Incan into a generally dynamically typed language.
- Finalizing streaming or incremental JSON parsing in this RFC.
- Settling every possible convenience surface in the first Draft.

## Guide-level explanation (how users think about it)

### Parse unknown JSON

```incan
from std.json import JsonValue

data = JsonValue.parse(response_body)?
```

### Inspect the runtime shape

```incan
match data.kind():
    JsonKind.Object => println("got an object")
    JsonKind.Array => println("got an array")
    _ => println("got some other JSON value")
```

### Mix typed and dynamic

```incan
from std.json import JsonValue
from std.serde import json

@derive(json)
model ApiResponse:
    status: int
    message: str
    data: JsonValue
```

This is the niche `JsonValue` is meant to fill: keep the stable parts typed while allowing one part of the payload to remain dynamic.

## Reference-level explanation (precise rules)

### Surface requirements

`JsonValue` must support:

- parsing from JSON text;
- serialization back to JSON text;
- representation of null, bool, string, number, array, and object JSON values;
- type predicates or equivalent runtime-shape inspection;
- typed extraction helpers for the supported value kinds.

### Dynamic inspection

- Runtime shape inspection must be explicit; users must be able to tell when they are handling a string versus an array versus an object.
- Extraction helpers must not silently coerce unrelated kinds.
- If the surface offers direct indexing for objects or arrays, the missing-key and out-of-bounds contract must be specified precisely rather than left to backend behavior.

### Interoperability

- `JsonValue` should be usable as a field type in model-based JSON workflows.
- If direct indexing is part of the final design, RFC 025 is the natural path for supporting multiple key types cleanly.
- If the final representation uses enum-backed methods, RFC 050 provides the needed language support.

## Design details

### Candidate representation: enum-backed

One strong design direction is to make `JsonValue` an enum with variants for each JSON kind. That would fit naturally with pattern matching and would compose well with RFC 050.

### Candidate representation: opaque wrapper

Another direction is a model-like wrapper over an implementation-defined runtime JSON representation. That would reduce the amount of language coupling, but it gives up some of the pattern-matching story and makes the public API more method-centric.

### Interaction with existing features

- RFC 024 remains the story for typed derive-based JSON handling.
- RFC 025 becomes relevant if `JsonValue` adopts multiple indexing traits.
- RFC 050 becomes relevant if the chosen public representation is enum-backed and method-rich.

### Compatibility / migration

This feature is additive. Existing typed JSON code keeps its meaning.

## Alternatives considered

1. **Only typed models**
   - Too rigid for exploratory or mixed-schema JSON work.

2. **Ad hoc `Dict[str, Any]`-style handling**
   - Too loose. It loses the benefit of having one explicit dynamic JSON contract.

3. **No stdlib dynamic JSON surface**
   - Forces each library or codebase to invent its own conventions for the same problem.

## Drawbacks

- A dynamic JSON type introduces runtime shape inspection into a language that otherwise prefers static structure.
- If the final API is too ergonomic in the wrong ways, users may reach for `JsonValue` where a typed model would be clearer.
- The representation choice materially affects ergonomics, so the Draft still carries real design risk.

## Layers affected

- **Stdlib / runtime**: must provide the `std.json.JsonValue` surface and its documented behavior.
- **Typechecker / docs**: must surface the runtime-shape API clearly and keep dynamic access explicit.
- **Lowering / emission**: must preserve the chosen parse/serialize and access semantics without leaking backend quirks.
- **Interop with derive flows**: should allow `JsonValue` to participate in otherwise typed JSON workflows.

## Unresolved questions

1. Should `JsonValue` be an enum-backed public type, an opaque wrapper type, or something hybrid?
2. Should direct `[]` access be part of the public surface, and if so, what is the contract for missing keys and out-of-bounds indices?
3. If numeric JSON values split into integer and float cases, what exact parsing and extraction rules should apply?
4. How much convenience API should `JsonValue` carry directly versus leaving to helper functions or follow-on RFCs?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
