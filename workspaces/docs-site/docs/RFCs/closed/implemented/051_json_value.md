# RFC 051: `JsonValue` for `std.json`

- **Status:** Implemented
- **Created:** 2026-04-06
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 024 (extensible derive protocol)
    - RFC 025 (multi-instantiation trait dispatch)
    - RFC 050 (enum methods and enum trait adoption)
- **Issue:** https://github.com/encero-systems/incan/issues/335
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** v0.3

## Summary

This RFC proposes `std.json.JsonValue` as Incan's dynamic JSON value surface for unknown or partially known JSON structures. It is intended to complement, not replace, model-based JSON handling by giving users a standard type for parse-inspect-transform workflows where the schema is not fully static.

## Core model

1. `JsonValue` represents the standard JSON value space: null, bool, number, string, array, and object.
2. `JsonValue` supports parsing from JSON text and serializing back to JSON text.
3. `JsonValue` provides typed inspection and extraction helpers so dynamic JSON code remains explicit about runtime shape checks.
4. `JsonValue` uses a hybrid public contract: users see an explicit JSON-kind model and enum-shaped inspection surface, while the underlying runtime representation remains implementation-defined.
5. `JsonValue` supports direct checked indexing for object keys and array positions without conflating missing values with JSON null.
6. Numeric JSON values classify through shared stdlib lexical helpers so JSON number parsing follows the same `is_int_like` / `is_float_like` contract available to ordinary Incan code.

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
- Provide a broad practical API for parse-inspect-transform workflows, including checked access, mutation helpers, traversal helpers, typed extraction, and pretty serialization.
- Promote reusable integer-like and float-like string classification helpers into the stdlib so JSON and CSV-like callers share one numeric lexical contract.

## Non-Goals

- Replacing typed JSON derive flows for stable schemas.
- Turning Incan into a generally dynamically typed language.
- Finalizing streaming or incremental JSON parsing in this RFC.
- Adding schema validation, JSON Schema support, JSONPath, or query-language semantics.
- Making dynamic JSON access silently coerce missing values, wrong shapes, or lossy numeric conversions.

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

### Index into dynamic payloads

```incan
from std.json import JsonValue

data = JsonValue.parse(response_body)?
user = data["user"]

if let Some(user_value) = user:
    name = user_value["name"]
    if let Some(name_value) = name:
        println(name_value.as_str()?)
```

Indexing is intentionally checked. A missing object key, an out-of-range array index, or an index kind that does not match the runtime JSON shape returns `None`. A present JSON null remains distinct: it returns `Some(JsonValue.null())`.

### Transform dynamic JSON

```incan
from std.json import JsonValue

data = JsonValue.parse(response_body)?
data.set("seen", JsonValue.bool(true))?
events = data.require_key("events")?
events.append(JsonValue.object({"kind": JsonValue.str("view")}))?
data.set("events", events)?
println(data.to_pretty_json()?)
```

The API is broad enough for ordinary parse-inspect-transform work. Users should not need to drop to ad hoc dictionaries for common object lookup, array access, mutation, traversal, and serialization.

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
- serialization back to compact or pretty JSON text;
- explicit constructors for null, bool, int, float, string, array, and object values;
- representation of null, bool, string, number, array, and object JSON values;
- type predicates or equivalent runtime-shape inspection;
- typed extraction helpers for the supported value kinds;
- checked direct indexing for object keys and array indices;
- object helpers for key lookup, insertion, removal, membership, keys, values, and items;
- array helpers for length, emptiness, append, extend, insert, removal, and iteration;
- traversal helpers for common nested paths;
- conversion support with typed model JSON workflows.

### Dynamic inspection

- Runtime shape inspection must be explicit; users must be able to tell when they are handling a string versus an array versus an object.
- Extraction helpers must not silently coerce unrelated kinds.
- `kind()` must return a `JsonKind` value whose variants cover null, bool, int, float, string, array, and object.
- Predicate helpers such as `is_null()`, `is_bool()`, `is_number()`, `is_int()`, `is_float()`, `is_str()`, `is_array()`, and `is_object()` must reflect the same runtime shape contract as `kind()`.
- Extraction helpers such as `as_bool()`, `as_int()`, `as_float()`, `as_str()`, `as_array()`, and `as_object()` must return `Result` or `Option` rather than panic or silently coerce.

### Indexing and checked access

- `JsonValue` must implement `Index[str, Option[JsonValue]]` for object-style access.
- `JsonValue` must implement `Index[int, Option[JsonValue]]` for array-style access.
- Object indexing returns `Some(value)` when the receiver is an object and the key exists, including when the stored value is JSON null.
- Object indexing returns `None` when the receiver is not an object or the key is absent.
- Array indexing returns `Some(value)` when the receiver is an array and the index is within bounds.
- Array indexing returns `None` when the receiver is not an array, the index is negative, or the index is out of bounds.
- Dedicated helpers such as `get(key)`, `at(index)`, `require_key(key)`, and `require_index(index)` must exist so callers can choose optional access or error-producing access without changing syntax.

### Numeric classification

- JSON number parsing must classify numeric lexemes with stdlib helpers that are also available to user code.
- The shared helpers must include `is_int_like(value: str) -> bool` and `is_float_like(value: str) -> bool`.
- Integer-like JSON numeric lexemes map to Incan `int`.
- Float-like JSON numeric lexemes map to Incan `float`.
- Numeric extraction must fail when the stored number cannot be represented by the requested Incan numeric type.
- The helpers must define their accepted lexical forms, including signs, decimal points, and exponent notation, rather than relying on backend parser accidents.

### Broad API surface

- Constructors must include `JsonValue.null()`, `JsonValue.bool(value)`, `JsonValue.int(value)`, `JsonValue.float(value)`, `JsonValue.str(value)`, `JsonValue.array(values)`, and `JsonValue.object(values)`.
- Serialization must include `to_json()` and `to_pretty_json()`.
- Lookup must include direct `[]`, optional helpers, and strict helpers.
- Object mutation must include `set`, `remove`, and `merge`.
- Array mutation must include `push`, `extend`, `insert`, and `remove_at`.
- Traversal must support common nested access through JSON Pointer helpers, not JSONPath syntax.
- Equality and debug/display behavior must be deterministic enough for tests and logs.
- The public API should remain Python-readable in Incan source even if some primitive operations bridge to Rust runtime helpers.

### Interoperability

- `JsonValue` should be usable as a field type in model-based JSON workflows.
- `JsonValue` should serialize and deserialize through the existing `std.serde.json` derive flow when used as a model field.
- `JsonValue` should not replace `std.serde.json`; typed models remain the preferred surface for stable schemas.
- RFC 025 supplies the multiple-key indexing machinery for `str` and `int` access.
- RFC 050 supplies the enum-method ergonomics used by `JsonKind` and any enum-shaped public surface.

## Design details

### Representation: hybrid public contract

`JsonValue` is publicly specified as a semantic JSON value with explicit shape inspection through `JsonKind`, predicates, typed extraction, and constructors. The public API should feel enum-shaped: users can ask for the kind and branch over the JSON value space without learning backend details.

The underlying runtime representation remains implementation-defined. This lets the implementation use a proven JSON runtime representation while keeping the Incan surface stable. Users must not rely on layout, backend type names, or Rust-specific behavior.

The implementation boundary is intentional: `std.json` behavior and helper APIs should be authored in Incan source, while the compiler/runtime side owns the carrier representation, parse/stringify boundary, and `@derive(json)` interop. Making the carrier itself a source-authored enum would require exposing representation controls for "serialize this enum as raw untagged JSON and deserialize arbitrary JSON back into it". That feature is plausible, but it is a language-design question, not a `JsonValue` helper implementation detail. Exposing serde-style encoder/decoder visitor mechanics directly in ordinary Incan source would also make the DX more obscure than the feature warrants. For this RFC, the appropriate line is therefore: keep the user-facing JSON API in Incan, and keep the backend representation hooks in compiler/runtime Rust.

### Indexing contract

Direct indexing is part of this RFC. It is checked and optional by default: `value["key"]` and `value[0]` return `Option[JsonValue]`. This is deliberately different from returning JSON null for missing keys, because missing and null carry different meanings in real payloads.

Strict helpers provide the error-producing path for callers that want a required shape. These helpers should use a JSON-specific error type so user code can distinguish malformed JSON, missing keys, wrong shapes, and numeric conversion failures.

### Numeric contract

JSON numeric lexemes are classified before mapping into Incan values. Integer-like lexemes become `int`; float-like lexemes become `float`. The classification helpers live in stdlib rather than inside `JsonValue` only, because CSV inference, JSON parsing, and other text-ingestion code need the same vocabulary.

The required helper names are `is_int_like` and `is_float_like`. They must accept the JSON-compatible numeric forms needed by this RFC, including signs and exponent notation. They must reject empty strings and malformed mixed forms. If a future decimal surface changes the preferred exact representation, that can extend numeric extraction without changing the basic dynamic JSON shape model.

### Interaction with existing features

- RFC 024 remains the story for typed derive-based JSON handling.
- RFC 025 is required for multiple `Index` instantiations on `JsonValue`.
- RFC 050 remains relevant because `JsonKind` should be method-friendly and pattern-matchable.

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
- A broad API increases implementation and documentation surface area; incomplete helper coverage would make the feature feel arbitrarily shaped.
- Direct indexing can look deceptively convenient, so the optional return contract must stay visible in types and examples.
- Mapping JSON numbers into Incan `int` and `float` keeps the surface simple but may need future extension for decimal or exact arbitrary-precision use cases.

## Layers affected

- **Stdlib / runtime**: must provide the `std.json.JsonValue` surface and its documented behavior. Source-authored Incan owns the public helper behavior; runtime Rust owns the implementation-defined JSON carrier and parse/stringify boundary.
- **Stdlib numeric helpers**: must expose shared `is_int_like` and `is_float_like` lexical predicates.
- **Typechecker / docs**: must surface the runtime-shape API clearly, support multiple indexing instantiations, and keep dynamic access explicit through `Option`.
- **Lowering / emission**: must preserve parse/serialize, checked indexing, mutation, and traversal semantics without leaking backend quirks.
- **Interop with derive flows**: should allow `JsonValue` to participate in otherwise typed JSON workflows without exposing serde visitor or serializer internals as ordinary user-facing Incan APIs.

## Implementation Plan

### Phase 1: RFC lifecycle and public contract

- Move the RFC to `In Progress` with the settled hybrid representation, checked indexing, stdlib numeric helpers, and broad API contract.
- Add a precise progress checklist so implementation slices can be validated against the contract.

### Phase 2: Stdlib numeric classification

- Promote the InQL-style integer-like and float-like string predicates into Incan stdlib.
- Define JSON-compatible lexical rules for signs, decimal points, and exponent notation.
- Add focused tests for accepted and rejected numeric strings.

### Phase 3: `std.json` surface and runtime bridge

- Add the `std.json` namespace and source declarations for `JsonKind`, `JsonValue`, JSON-specific errors, constructors, parse/serialize helpers, predicates, extraction helpers, object helpers, array helpers, traversal helpers, and mutation helpers.
- Back primitive operations with runtime helpers where needed while keeping the `.incn` module as the owner of the public API shape.
- Ensure the implementation remains a hybrid public contract rather than a thin untyped facade over backend-only behavior.

### Phase 4: Indexing, typechecking, lowering, and emission

- Make `JsonValue` support both `Index[str, Option[JsonValue]]` and `Index[int, Option[JsonValue]]`.
- Preserve checked optional indexing through lowering and emission.
- Add diagnostics or typechecker support needed for invalid index key types.

### Phase 5: Model interop and conversions

- Make `JsonValue` usable as a field in `std.serde.json` model workflows.
- Ensure JSON parse/serialize round trips preserve null, booleans, strings, arrays, objects, and int/float numeric classification.
- Add conversion helpers between dynamic `JsonValue` and typed model JSON entry points where the type system can express them.

### Phase 6: Tests, docs, release notes, and versioning

- Add stdlib, typechecker, codegen, and integration tests covering the broad API.
- Update user-facing docs for dynamic JSON workflows and numeric-like helpers.
- Update active release notes and bump the active development version when the implementation lands.

## Implementation log

### Spec / lifecycle

- [x] Resolve representation as a hybrid public contract.
- [x] Include checked direct indexing in the public surface.
- [x] Define numeric classification through stdlib `is_int_like` / `is_float_like`.
- [x] Commit to a broad first-class API rather than a minimal core-only surface.
- [x] Keep the RFC status and checklist in sync as implementation lands.

### Stdlib numeric helpers

- [x] Add public `is_int_like(value: str) -> bool`.
- [x] Add public `is_float_like(value: str) -> bool`.
- [x] Define and test JSON-compatible numeric lexical rules.
- [x] Reuse the helpers in JSON number classification.

### `std.json` API

- [x] Register the `std.json` namespace.
- [x] Add `JsonKind`.
- [x] Add `JsonValue`.
- [x] Add JSON-specific error types.
- [x] Add constructors for null, bool, int, float, string, array, and object.
- [x] Add parse, compact serialization, and pretty serialization.
- [x] Add predicates and typed extraction helpers.
- [x] Add object lookup, insertion, removal, membership, keys, values, and items.
- [x] Add array length, emptiness, append, extend, insert, removal, and iteration helpers.
- [x] Add traversal helpers for common nested paths.
- [x] Add deterministic equality and debug/display behavior where supported.

### Indexing / compiler pipeline

- [x] Support `JsonValue[str]` indexing as `Option[JsonValue]`.
- [x] Support `JsonValue[int]` indexing as `Option[JsonValue]`.
- [x] Reject unsupported index key types with a precise diagnostic.
- [x] Preserve checked indexing through lowering and emission.
- [x] Add typechecker and codegen coverage for both key types.

### Model interop

- [x] Allow `JsonValue` as a field in `@derive(json)` models.
- [x] Round-trip dynamic fields through `std.serde.json` serialization.
- [x] Add tests mixing typed model fields and dynamic JSON fields.

### Docs / release

- [x] Update the dynamic JSON docs or add a new user-facing page.
- [x] Update stdlib reference/navigation for `std.json`.
- [x] Document `is_int_like` and `is_float_like`.
- [x] Add active `0.3` release notes.
- [x] Bump the active `0.3.0-dev.N` version for the implementation.

### Verification

- [x] Run focused stdlib numeric helper tests.
- [x] Run focused `std.json` parse/serialize/indexing tests.
- [x] Run focused model interop tests.
- [x] Run generated language reference checks if the stdlib registry changes.
- [x] Run `make fmt`.
- [x] Run `make pre-commit`.

## Design Decisions

1. `JsonValue` uses a hybrid public contract: enum-shaped inspection and methods for users, implementation-defined runtime representation underneath.
2. Direct indexing is part of the feature. It returns `Option[JsonValue]` for both string object keys and integer array indices so missing, wrong-shape, and out-of-bounds access remain explicit.
3. JSON numbers map to Incan `int` or `float` by applying shared stdlib `is_int_like` and `is_float_like` lexical helpers to the source numeric string.
4. The RFC owns a broad practical API for dynamic JSON, including constructors, parse/serialize, inspection, extraction, checked access, mutation, traversal, and model interop. Future RFCs may add streaming, schema validation, JSONPath, or specialized query behavior.
