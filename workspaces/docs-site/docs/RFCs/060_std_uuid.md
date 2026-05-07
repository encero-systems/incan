# RFC 060: `std.uuid` — UUID parsing, generation, and formatting

- **Status:** Draft
- **Created:** 2026-04-14
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 009 (sized numeric types and `u128`)
    - RFC 022 (namespaced stdlib modules and compiler handoff)
    - RFC 023 (compilable stdlib and Rust module binding)
    - RFC 056 (`std.io` in-memory byte streams and binary parsing helpers)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/338
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC proposes `std.uuid` as Incan's standard library module for working with UUIDs, using RFC 9562 as the normative baseline for canonical formatting and generation while remaining compatible with binary-first interoperability surfaces such as Substrait. It provides a first-class `UUID` type together with parsing, formatting, byte and `u128` conversion, version and variant inspection, standard constants, and generation helpers for the RFC-defined UUID versions that have portable generation rules. The goal is to treat UUIDs as real values rather than as loose strings, ad hoc byte arrays, or database-specific conventions.

## Motivation

UUIDs show up everywhere: APIs, databases, event streams, object storage keys, distributed systems, and migration tooling. Without a dedicated stdlib type, users pass them around as strings, bytes, or loosely documented conventions, which makes validation, formatting, and version-aware behavior harder than it should be.

This matters for several reasons:

- typed UUID values are clearer than free-form strings;
- parsing and formatting should be standardized rather than reimplemented in every codebase;
- binary formats and network protocols often need byte-level UUID access;
- analytics and data systems often benefit from lossless numeric UUID handling via `u128`;
- generation helpers are a common enough need to deserve a standard path.

## Goals

- Provide a first-class `UUID` type in `std.uuid`.
- Follow RFC 9562 as the normative UUID baseline.
- Support parsing from canonical textual forms and common interoperable textual forms.
- Support conversion to and from the 16-byte binary representation and lossless `u128` representation.
- Remain compatible with Substrait-style binary UUID interchange.
- Standardize UUID constants, version and variant inspection, and generation helpers.
- Make the standard library UUID story stronger than ecosystems that only treat UUIDs as strings or opaque framework helpers.

## Non-Goals

- Standardizing every UUID-adjacent identifier format in this RFC.
- Turning UUID generation into a cryptography RFC.
- Standardizing lossy UUID-to-`BIGINT` surrogate hashing for database-specific performance workarounds.
- Solving database-specific ID generation or ordering strategies beyond what UUID versions already provide.
- Replacing application-specific identifier types where stronger domain modeling is warranted.

## Guide-level explanation

Authors should be able to parse, compare, format, inspect, and generate UUIDs directly.

```incan
from std.uuid import UUID

user_id = UUID.parse("550e8400-e29b-41d4-a716-446655440000")?
println(str(user_id))
println(user_id.version)
```

```incan
from std.uuid import UUID

request_id = UUID.v7()?
raw = request_id.to_bytes()
round_tripped = UUID.from_bytes(raw)?
number = request_id.to_int()
same_id = UUID.from_int(number)?
```

```incan
from std.uuid import UUID

report_id = UUID.v5(UUID.NAMESPACE_URL, "https://example.com/reports/2026/04/14")?
println(report_id.to_urn())
```

## Reference-level explanation

### Module scope

`std.uuid` should provide:

- a `UUID` value type;
- parse and format helpers;
- binary and numeric conversion helpers;
- constants for Nil, Max, and the standard namespace UUIDs;
- version and variant inspection;
- generation helpers for the RFC 9562 UUID versions with portable generation rules.

### Expected capability areas

The eventual contract should cover:

- parsing RFC 9562 UUID values across supported textual forms;
- accepting arbitrary 128-bit UUID payloads through bytes and `u128` conversion, even when the bit pattern is not an RFC 9562-generated value;
- formatting in canonical lowercase hyphenated form;
- equality and ordering semantics;
- conversion to and from 16-byte binary representations;
- conversion to and from `u128`;
- helpers to inspect the UUID version and variant;
- generation helpers for UUID versions 1, 3, 4, 5, 6, 7, and 8;
- explicit handling of version 2 as parseable and inspectable but not generatable in core stdlib because RFC 9562 leaves its definition outside the scope of the specification.

### API direction

The module should treat UUIDs as structured identifier values, not just as strings with validation. The public contract should be explicit about:

- canonical string formatting;
- exact byte ordering for `to_bytes()` and `from_bytes(...)`;
- lossless `u128` conversion via `to_int()` and `from_int(...)`;
- how ordering works;
- which UUID versions can be generated directly;
- which UUID versions are supported only for parsing and inspection.

The module should remain simple. It does not need to expose raw, version-specific wire fields as part of the core standard-library surface.

The module should remain simple. It should not turn UUIDs into a protocol-forensics API with version-specific raw field extraction as part of the core standard-library surface.

### Public API surface

The north-star surface should include:

- `UUID.parse(text: str) -> Result[UUID, E]`
- `UUID.from_bytes(raw: bytes) -> Result[UUID, E]`
- `UUID.from_int(value: u128) -> Result[UUID, E]`
- `UUID.v1() -> Result[UUID, E]`
- `UUID.v3(namespace: UUID, name: str | bytes) -> Result[UUID, E]`
- `UUID.v4() -> Result[UUID, E]`
- `UUID.v5(namespace: UUID, name: str | bytes) -> Result[UUID, E]`
- `UUID.v6() -> Result[UUID, E]`
- `UUID.v7() -> Result[UUID, E]`
- `UUID.v8(raw: bytes) -> Result[UUID, E]`
- `uuid.to_string() -> str`
- `uuid.to_hex() -> str`
- `uuid.to_urn() -> str`
- `uuid.to_bytes() -> bytes`
- `uuid.to_int() -> u128`

The type should also expose read-only information and constants:

- `uuid.version`
- `uuid.variant`
- `UUID.NIL`
- `UUID.MAX`
- `UUID.NAMESPACE_DNS`
- `UUID.NAMESPACE_URL`
- `UUID.NAMESPACE_OID`
- `UUID.NAMESPACE_X500`

## Design details

### Why UUID deserves a first-class type

Using `str` for UUIDs works until it doesn't. Parsing, validation, formatting, version inspection, and byte conversion all become repeated ad hoc logic. A dedicated type makes the API clearer, allows version-aware helpers, and gives the language a consistent interoperability story across text, bytes, and `u128`.

### RFC 9562 baseline

RFC 9562 is the normative baseline for this module. In practice, that means:

- UUIDs are 128-bit values.
- Binary conversion uses the RFC/network byte order.
- Nil and Max UUIDs are standard constants.
- versions 1, 3, 4, 5, 6, 7, and 8 are real RFC-defined UUID versions;
- version 2 is reserved for DCE Security UUIDs, but the RFC leaves its definition outside the scope of the specification.

Following RFC 9562 does not mean every UUID version should be treated as equally good for new application design. The stdlib should support the full RFC 9562 value space while still documenting preferred usage:

- prefer `v7` for time-sortable UUID generation;
- prefer `v4` for simple random identifiers;
- prefer `v5` over `v3` for deterministic namespace-based identifiers;
- treat `v8` as advanced or vendor-specific.

### Substrait compatibility and arbitrary 128-bit values

Substrait UUID values are looser than RFC 9562 text-format UUIDs: the binary type is a 128-bit UUID payload and does not require every value to be a canonical RFC-generated UUID. To stay compatible with that model:

- `UUID.from_bytes(...)` must accept any 16-byte payload;
- `UUID.from_int(...)` must accept any `u128` value;
- `UUID.parse(...)` remains the textual RFC-oriented parser for standard UUID string forms.

This means `std.uuid` is RFC 9562-aware rather than RFC 9562-validity-gated. The type supports canonical RFC generation and formatting, but binary and numeric interchange remain fully 128-bit-compatible.

### Parsing and formatting

Canonical formatting is the lowercase hyphenated form:

- `550e8400-e29b-41d4-a716-446655440000`

Parsing should accept the common interoperable forms as well:

- lowercase or uppercase hexadecimal digits;
- canonical hyphenated form;
- bare 32-character hexadecimal form;
- surrounding braces;
- `urn:uuid:` prefixed form.

Regardless of input form, canonical stringification should render lowercase hyphenated output.

When a `UUID` is converted to `str`, it should produce the same canonical lowercase hyphenated form.

When a `UUID` is converted to `str`, it should produce the same canonical lowercase hyphenated form.

### Bytes and `u128`

`UUID.to_bytes()` and `UUID.from_bytes(...)` must use the RFC 9562 byte ordering only. The standard library should not standardize platform- or vendor-specific alternate byte layouts such as COM/GUID little-endian field permutations.

`UUID.to_int()` and `UUID.from_int(...)` should be part of the core contract. This gives Incan a lossless numeric UUID representation that is stronger than ecosystems which fall back to strings or narrower integer surrogates when UUID columns are inconvenient to store natively.

The public numeric representation is `u128`, not `i128`. UUIDs are 128-bit identifier values, not signed numbers. The implementation may store the bits however it likes internally, but the public contract should standardize `u128` as the lossless numeric view.

The public numeric representation is `u128`, not `i128`. UUIDs are 128-bit identifier values, not signed numbers. The implementation may store the bits however it likes internally, but the public contract should standardize `u128` as the lossless numeric view.

### Comparison and ordering

UUID values should support equality and ordering by raw UUID value, equivalent to comparison by their `u128` representation or RFC/network-order bytes.

This gives the language a single deterministic total ordering for all UUID values. The RFC should still state clearly that only some versions have meaningful chronological ordering semantics:

- `v6` and `v7` are designed to sort well as raw bytes;
- RFC 9562 explicitly recommends `v7` over `v1` and `v6` where possible;
- ordering all UUIDs is useful as a language and data-structure property, but it should not be confused with domain-specific creation-time ordering for every UUID version.

### Generation surface

Core stdlib generation should follow RFC 9562 directly:

- `UUID.v1()` for Gregorian time-based UUIDs;
- `UUID.v3(namespace, name)` for MD5 namespace-based UUIDs;
- `UUID.v4()` for random UUIDs;
- `UUID.v5(namespace, name)` for SHA-1 namespace-based UUIDs;
- `UUID.v6()` for reordered Gregorian time-based UUIDs;
- `UUID.v7()` for Unix-epoch time-based UUIDs;
- `UUID.v8(raw)` for vendor-specific UUID construction from a 16-byte payload, with the RFC 9562 version and variant bits applied by the constructor.

Version 2 is the exception. RFC 9562 identifies it as DCE Security UUIDs but leaves its definition outside the scope of the RFC, so `std.uuid` should parse and inspect version 2 UUID values but should not promise a `UUID.v2()` generator in the core standard library contract.

### Namespaces and constants

Because name-based UUIDs are part of the committed surface, the standard namespace identifiers should be exposed directly:

- `UUID.NAMESPACE_DNS`
- `UUID.NAMESPACE_URL`
- `UUID.NAMESPACE_OID`
- `UUID.NAMESPACE_X500`

The special constants should also be public:

- `UUID.NIL`
- `UUID.MAX`

### Simple inspection model

Core stdlib inspection should stop at:

- `uuid.version`
- `uuid.variant`

That is enough for ordinary application, analytics, and interoperability use cases. The standard library does not need to expose every internal field of time-based UUID layouts just because those fields exist in the wire format.

For non-RFC bit patterns introduced through `from_bytes(...)` or `from_int(...)`, these inspection views should reflect the bits when they map cleanly onto known UUID variant/version classifications and otherwise indicate an unknown or non-standard value.

### Simple inspection model

Core stdlib inspection should stop at:

- `uuid.version`
- `uuid.variant`

That is enough for most application, analytics, and interoperability use cases. The standard library does not need to expose every version-specific internal field of time-based UUID layouts just because those fields exist in the wire format.

### Interaction with existing features

- `std.io` can support reading and writing UUID bytes once a canonical byte-ordering contract exists.
- future JSON and database-related code can serialize UUIDs via their standard text form or `u128` form where that is more appropriate;
- future Substrait support can expose UUID values through their standard 16-byte binary representation without changing the `UUID` model;
- Rust interop remains the escape hatch for specialized UUID features not standardized here.

### Compatibility / migration

This feature is additive. Existing string-based code keeps working, but new APIs should prefer `UUID` over loose string conventions where an identifier is semantically a UUID. Existing legacy UUIDs remain interoperable because the module parses and inspects the full RFC 9562 UUID value space, including older versions.

## Alternatives considered

1. **Strings only**
   - Too weak and too easy to misuse.

2. **Bytes only**
   - Too low-level for common API and storage use cases.

3. **Rust interop only**
   - Too implementation-shaped for ordinary application code.

4. **Expose only a narrow generator subset**
   - Cleaner on paper, but too restrictive for interoperability-heavy application and migration work that still encounters legacy UUID versions.

## Drawbacks

- A complete UUID module needs careful decisions about version support, text permissiveness, and binary/text canonicalization.
- A first-class UUID type adds one more value kind for users to learn.
- Supporting legacy and vendor-specific UUID generation in core stdlib increases the size of the API surface.
- `v8` in particular is easy to misuse if the documentation does not make its vendor-specific nature explicit.

## Layers affected

- **Stdlib / runtime**: must provide the UUID type, parsing, formatting, and generation semantics.
- **Language surface**: the module and value type must be available as specified.
- **Execution handoff**: implementations must preserve parsing, formatting, and byte-conversion behavior without backend leakage.
- **Docs / tooling**: should standardize UUID examples and canonical formatting expectations.
- **Numeric/runtime interop**: must align `UUID.to_int()` and `UUID.from_int(...)` with RFC 009 `u128` semantics.

## Design Decisions

- `std.uuid` follows RFC 9562 as its normative UUID baseline.
- `UUID` is a first-class value type with parsing, formatting, binary conversion, and `u128` conversion.
- `UUID.from_bytes(...)` and `UUID.from_int(...)` accept arbitrary 128-bit UUID payloads for compatibility with binary-first systems such as Substrait.
- Parsing accepts canonical hyphenated text plus common interoperable forms such as bare hex, braces, and URN-prefixed UUID text.
- Canonical formatting is lowercase hyphenated text.
- `UUID.to_bytes()` and `UUID.from_bytes(...)` use RFC/network byte order only.
- `UUID.to_int()` and `UUID.from_int(...)` are part of the core contract and depend on RFC 009 `u128`.
- `str(uuid)` renders the canonical lowercase hyphenated text form.
- `UUID` values support equality and total ordering by raw UUID value.
- `UUID.NIL`, `UUID.MAX`, and the four standard namespace constants are part of the public surface.
- Core stdlib generation includes `v1`, `v3`, `v4`, `v5`, `v6`, `v7`, and `v8`.
- Core stdlib does not promise `UUID.v2()` generation, because RFC 9562 leaves version 2 outside the scope of the specification.
- Core inspection is intentionally small and stops at `version` and `variant`.
- `UUID.parse(...) -> Result[...]` is the parsing entry point; there is no separate `try_parse(...)` alias.
- The public numeric UUID representation is `u128`; internal storage is not part of the public contract.
- The module should remain compatible with Substrait-style 16-byte UUID interchange.
- Documentation should recommend `v7` for ordered IDs, `v4` for simple random IDs, and `v5` over `v3` for deterministic namespace-based IDs.
