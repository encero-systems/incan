# RFC 101: `std.collections.OrdinalMap` — deterministic compact key-to-ordinal lookup

- **Status:** Implemented
- **Created:** 2026-05-19
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 009 (sized numeric types)
    - RFC 022 (namespaced stdlib modules and compiler handoff)
    - RFC 023 (compilable stdlib and Rust module binding)
    - RFC 030 (`std.collections` extended collection types)
    - RFC 052 (module static storage)
    - RFC 065 (`std.hash` stable hashing primitives)
    - RFC 086 (schema descriptors and adapters)
    - RFC 098 (native associated types for traits)
    - RFC 099 (generic trait-targeted methods)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/595
- **Follow-up:** https://github.com/dannys-code-corner/incan/issues/596 (v0.5 trait-system bridge removal)
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** v0.3

## Summary

This RFC proposes `std.collections.OrdinalMap`, an immutable map from deterministically encodable keys to compact integer ordinals. The type gives schemas, catalogs, generated metadata, and immutable dataset dictionaries a reproducible way to turn names, IDs, dates, UUIDs, and other stable scalar values into integer codes, while keeping safe lookup exact, deterministic, serializable, and separate from mutable `dict` or general `FrozenDict` semantics.

## North star

`OrdinalMap` is not a convenience wrapper around `dict[str, int]`. The desired end-state is a stdlib collection whose output is deterministic enough for persisted metadata and dataset indexes, and whose performance profile is credible against specialized immutable-map libraries.

The target use case is large, static, query-many lookup: schemas with hundreds of columns, catalogs with stable IDs, dictionary-encoded segment values, and planning-time name resolution for systems such as InQL. A planner should be able to resolve stable keys to ordinals once, cache or ship that representation, and then execute against compact integer positions instead of repeatedly comparing source keys.

## Core model

1. **Ordinal maps are immutable key-to-integer maps:** an `OrdinalMap[K]` maps each unique key of type `K` to a non-negative integer ordinal.
2. **Keys are deterministic encodings, not arbitrary hashables:** supported keys must implement a stable key-encoding contract so equal keys produce identical canonical bytes across platforms.
3. **Construction is deterministic:** the same canonical input must produce the same ordinals, lookup structure, and serialized bytes for a given Incan version and format version.
4. **Safe lookup is exact:** default lookup APIs must not silently return an ordinal for a missing key.
5. **Storage is compact:** ordinal cells should use the smallest supported unsigned storage width that can represent the maximum ordinal.
6. **Unchecked lookup is explicit:** a proven-present fast path may exist, but it must be visibly unsafe in the API and must not be the default behavior.
7. **The public contract is portable:** observable behavior must be defined by deterministic key encoding, exact safe lookup, compact ordinal storage, and deterministic serialization rather than backend-specific map behavior.

## Motivation

Large systems frequently need to resolve stable names or scalar values into integer positions before doing real work. Column names become column ordinals, field names become field IDs, table names become catalog IDs, enum-like values become compact codes, and string or scalar dictionary values become physical segment codes. Once a query, transformation, or scan has resolved a key to an ordinal, the hot path can use integer slots instead of repeated string comparisons.

Builtin `dict` is the right tool for mutable, general-purpose lookup. It is not the right public abstraction for immutable, reproducible, serialized lookup tables that should be rebuilt byte-for-byte across machines. `FrozenDict` is the right general frozen dictionary shape, but it is intentionally broad and should not be overloaded with specialized minimal-perfect-hash or ordinal-specific behavior.

The motivating requirement is not merely speed. Determinism matters because generated metadata, persisted catalogs, and immutable dataset segments must be reproducible. If the same schema or dictionary domain produces different index bytes depending on process randomness, host platform, or insertion-order accidents, downstream artifacts become harder to diff, cache, test, verify, and distribute.

Purpose-built data libraries also need a primitive below full physical indexing. A table with hundreds of columns and billions of rows should not resolve column names inside the row loop. The plan should resolve names to ordinals once, then execute against integer slots. For dictionary-encoded scalar columns, the segment dictionary should resolve a value such as `"paid"` or `Status.Paid` to an integer code once, then scan or compare packed integer codes.

This RFC is directly informed by Daniel Lemire's [`constmap`](https://github.com/lemire/constmap) work and by Thomas Mueller Graf and Daniel Lemire's [binary fuse](https://arxiv.org/abs/2201.01174) and [xor filter](https://arxiv.org/abs/1912.08258) papers. That work demonstrates the practical value of immutable compact maps built from a known key set, including fast lookup, compact storage, verified and unchecked lookup modes, and serialization. `OrdinalMap` adapts those lessons to Incan's stdlib goals: deterministic construction, exact safe lookup, generic deterministic scalar keys, and ordinal/code-oriented semantics rather than a string-only `uint64` map.

## Goals

- Add `OrdinalMap[K]` to `std.collections` as an immutable, deterministic key-to-ordinal lookup type.
- Define `OrdinalKey` as the stable key-encoding contract for key types that can be used with `OrdinalMap`.
- Support stable scalar keys beyond `str` and `bytes`, including booleans, ordinary `int`, fixed-width integers, fixed-precision decimals, UUIDs, civil dates, civil times, civil datetimes, fixed-offset datetimes, value enums with stable scalar encodings, and user-defined key types that implement the contract.
- Support construction from ordered keys where ordinals are input positions.
- Support construction from explicit key/ordinal pairs where ordinals are caller-provided.
- Require exact safe lookup semantics for default APIs.
- Define deterministic construction and deterministic serialization requirements.
- Select compact ordinal storage width from the maximum ordinal.
- Provide batch lookup APIs for resolving many keys without forcing callers into repeated boilerplate.
- Provide explicit unchecked lookup APIs for proven-present hot paths and benchmarkable batch lookup, without making unchecked lookup the default.
- Provide byte-size introspection and deterministic serialization/deserialization APIs suitable for persisted metadata.
- Match the practical feature surface that made `constmap` and `fastconstmap` worth studying: immutable construction from known keys and ordinals, fast lookup, batch lookup, checked and unchecked modes, compact storage, serialization, deserialization, and benchmarks against ordinary maps.

## Non-Goals

- This RFC does not replace builtin `dict`.
- This RFC does not replace `FrozenDict`.
- This RFC does not define a mutable ordered map.
- This RFC does not define a row-level database index, B-tree, bitmap index, inverted index, vector index, or range index.
- This RFC does not accept arbitrary objects without a deterministic `OrdinalKey` implementation.
- This RFC does not define floating-point key semantics. Float support needs a separate policy for NaN payloads, signed zero, and canonicalization before it can satisfy this RFC's determinism requirement.
- This RFC does not standardize probabilistic missing-key behavior as the default safe API.
- This RFC does not expose backend hash-table, minimal-perfect-hash, or fuse-filter internals as part of the public contract.

## Guide-level explanation

Use `OrdinalMap` when a stable set of keys needs stable integer positions.

```incan
from std.collections import OrdinalMap

columns = OrdinalMap.from_keys(["order_id", "customer_id", "status", "amount"])?

assert columns["status"] == 2
assert columns.get("missing").is_none()
```

The simplest construction form takes an ordered key list. The ordinal is the key's position in that list.

```incan
statuses = OrdinalMap.from_keys(["pending", "paid", "cancelled"])?

assert statuses["pending"] == 0
assert statuses["paid"] == 1
assert statuses["cancelled"] == 2
```

Explicit ordinals are useful when ordinals come from a file format, schema registry, generated catalog, or physical storage layout.

```incan
field_ids = OrdinalMap.from_pairs([
    ("order_id", 10),
    ("customer_id", 12),
    ("status", 18),
])?

assert field_ids["status"] == 18
```

Ordinal maps are not limited to strings. They can map any key with a stable canonical byte encoding.

```incan
first_id: u64 = 1001
second_id: u64 = 1002
third_id: u64 = 1003
customer_ids = OrdinalMap.from_keys([first_id, second_id, third_id])?
assert customer_ids[second_id] == 1
```

UUID and date-like values are ordinary use cases when their stdlib representation has a fixed canonical encoding.

```incan
from std.datetime import Date
from std.uuid import UUID

statement_days = OrdinalMap.from_keys([
    Date.fromisoformat("2026-05-01")?,
    Date.fromisoformat("2026-06-01")?,
])?

tenant_ids = OrdinalMap.from_keys([
    UUID.parse("018f2f26-4b7e-7a1a-9f32-59f1ab02a001")?,
    UUID.parse("018f2f26-4b7e-7a1a-9f32-59f1ab02a002")?,
])?
```

For dictionary-encoded data, an ordinal map can translate values into compact codes before execution enters a hot row loop.

```incan
status_codes = OrdinalMap.from_keys(["cancelled", "paid", "pending"])?
paid_code = status_codes.require("paid")?

# A scan can compare stored integer codes to `paid_code` instead of comparing strings for every row.
```

Batch lookup resolves many names or values at once.

```incan
wanted = columns.require_many(["status", "amount"])?
assert wanted == [2, 3]
```

Persisted metadata can serialize an ordinal map. Deserialization must validate the format, key encoding, construction metadata, and exact-lookup data before returning a usable map.

```incan
blob = columns.to_bytes()
round_trip = OrdinalMap[str].from_bytes(blob)?
assert round_trip["amount"] == 3
```

## Reference-level explanation

### Module and type

`std.collections` must expose `OrdinalMap[K]` as a public immutable collection type. `OrdinalMap[K]` maps keys of type `K` to non-negative integer ordinals.

`OrdinalMap[K]` must require `K` to satisfy `OrdinalKey`. A key type that lacks a stable canonical encoding must not be accepted by `OrdinalMap`.

The type must be immutable after construction. APIs that appear to add, remove, or reassign entries must return a new map or be absent.

### Ordinal key contract

The stdlib must define an ordinal-key contract for key types that can be used with `OrdinalMap`. The contract must provide canonical bytes for a key value and must identify the key encoding used by serialized maps.

The trait name is `OrdinalKey`.

```incan
pub trait OrdinalKey:
    @staticmethod
    def ordinal_encoding() -> str: ...

    @staticmethod
    def from_ordinal_bytes(data: bytes) -> Result[Self, OrdinalMapError]: ...

    def ordinal_bytes(self) -> bytes: ...
```

`ordinal_bytes(self)` returns the canonical bytes for a concrete key value. `ordinal_encoding()` returns a stable type-level encoding identifier, such as `str:utf8`, `uuid:rfc9562-bytes`, or another accepted identifier. `from_ordinal_bytes(data)` decodes one stored canonical key record for the target key type and is required so `OrdinalMap[K].from_bytes(data)` can reconstruct exact keys without forcing callers through string conversion. Serialized maps store the encoding identifier and must reject data whose key encoding does not match the target `OrdinalMap[K]`.

The key encoding must be deterministic across supported platforms. It must not depend on locale, process-randomized hash seeds, pointer identity, memory layout, map iteration order, host endianness, timezone database versions, or implementation-defined object identity.

The required key set includes `str`, `bytes`, `bool`, ordinary `int`, fixed-width signed and unsigned integers, `decimal[p, s]` / `numeric[p, s]` / `decimal128[p, s]`, `UUID`, `Date`, `Time`, `DateTime`, `DateTimeOffset`, value enums whose values have stable scalar encodings, and user-defined types that implement `OrdinalKey` with a versioned encoding identifier.

`str` keys must encode their exact Unicode string value as UTF-8 bytes without implicit case folding, locale handling, or normalization. If callers need normalized keys, they must normalize before construction.

`bytes` keys must encode as their raw byte contents.

Integer keys must encode deterministically. Ordinary `int` uses Incan's stable `i64` encoding. Fixed-width integer keys use little-endian two's-complement representation for signed fixed-width integers and little-endian representation for unsigned fixed-width integers.

Decimal keys must encode deterministically through the `Decimal128` runtime representation: little-endian signed coefficient bytes followed by the stored scale byte. Binary floating-point keys remain excluded from `OrdinalKey`.

`UUID` keys must encode as their 16 RFC/network-order bytes. `Date` keys must encode the proleptic Gregorian year, month, and day fields. `Time` keys must encode hour, minute, second, and nanosecond fields. `DateTime` keys must encode the date fields followed by the time fields. `DateTimeOffset` keys must encode the local datetime fields plus the fixed offset in seconds. These encodings must not route through locale-formatted strings.

Floating-point keys are excluded because NaN payloads, negative zero, and cross-language canonicalization rules need a separate policy before floats can satisfy this RFC's deterministic-key contract.

### Construction from keys

`OrdinalMap.from_keys(keys: list[K]) -> Result[OrdinalMap[K], OrdinalMapError]` must construct a map where each key's ordinal is its zero-based position in the input list.

Construction from keys must reject duplicate keys. Duplicate detection must use the key's equality semantics and canonical ordinal bytes consistently. If two values compare equal, they must not appear as distinct keys. If two non-equal values produce the same canonical bytes, construction must fail with an encoding-collision error because the key contract has been violated.

For a fixed ordered input list, construction must be deterministic. The implementation may use hashing or retry-based placement internally, but all seeds, retries, placement choices, and serialized output must be derived deterministically from the canonical input and format version.

### Construction from pairs

`OrdinalMap.from_pairs(entries: list[Tuple[K, int]]) -> Result[OrdinalMap[K], OrdinalMapError]` must construct a map where each key maps to the explicit non-negative ordinal supplied by the caller.

Construction from pairs must reject duplicate keys, duplicate ordinals, and negative ordinals. Ordinal maps are position/code maps, not many-to-one labels. If many-to-one labels are needed later, they must use a separate type or an explicit constructor with a different name.

Construction from pairs must be deterministic regardless of the input pair order when explicit ordinals are the same. The serialized representation should use a canonical order derived from key bytes or ordinal order so byte output is reproducible.

### Lookup APIs

`OrdinalMap[K].get(key: K) -> Option[int]` must return `Some(ordinal)` when `key` is present and `None` when it is absent.

`OrdinalMap[K].require(key: K) -> Result[int, OrdinalMapError]` must return the ordinal when present and a missing-key error when absent.

`OrdinalMap[K].__getitem__(key: K) -> int` must return the ordinal when present and raise a `KeyError`-style runtime error when missing, matching Incan's mapping indexing convention. Examples that handle absent keys should use `get` or `require`; indexing is the ergonomic present-key path.

`OrdinalMap[K].__contains__(key: K) -> bool` must return whether `key` is present.

`OrdinalMap[K].get_many(keys: list[K]) -> list[Option[int]]` must perform batch lookup while preserving input order.

`OrdinalMap[K].require_many(keys: list[K]) -> Result[list[int], OrdinalMapError]` must perform batch lookup and return a missing-key error if any key is absent. The error should identify at least one missing key position and should identify all missing key positions when practical.

Safe lookup APIs must be exact. They must not return an ordinal for an absent key due only to hash or fingerprint collision. If an implementation uses fingerprints internally, it must verify enough key material to reject absent keys exactly before returning from safe APIs.

### Unchecked lookup

Unchecked lookup APIs must be provided for callers that can prove a key is present through external invariants or prior exact validation. The APIs are `get_unchecked(key: K) -> int` and `get_many_unchecked(keys: list[K]) -> list[int]`.

Unchecked lookup must not be used by `get`, `require`, `__getitem__`, `__contains__`, `get_many`, or `require_many`.

Unchecked lookup must document its missing-key behavior as undefined or implementation-specific. It must not silently become the default surface because the primary `OrdinalMap` contract is exact lookup. Benchmarks should include both safe and unchecked paths so Incan owns the performance story without weakening the default API.

### Deterministic storage and compact ordinals

A conforming implementation must store ordinals using the smallest unsigned integer width that can represent the maximum ordinal in the map. The required width choices are `u8`, `u16`, `u32`, and `u64`.

A map whose maximum ordinal is at most `255` must use `u8` ordinal cells. A map whose maximum ordinal is at most `65535` must use `u16` ordinal cells. A map whose maximum ordinal is at most `4294967295` must use `u32` ordinal cells. Larger supported ordinals must use `u64` ordinal cells.

The public API returns ordinary Incan `int` ordinals unless a narrower return type is explicitly requested by a future RFC. Compact storage is an implementation and serialization property, not a narrowing of the public value.

### Serialization

`OrdinalMap[K].to_bytes() -> bytes` must return a deterministic byte representation for the map.

`OrdinalMap[K].from_bytes(data: bytes) -> Result[OrdinalMap[K], OrdinalMapError]` must parse and validate a serialized map before returning it.

Serialized ordinal maps must use the following container contract:

- magic bytes: `b"INCAN_ORDMAP\0"`;
- format version: little-endian `u16`, with this RFC defining format version `1`;
- flags: little-endian `u16`;
- key encoding identifier: little-endian `u16` byte length followed by UTF-8 identifier bytes;
- key count: little-endian `u64`;
- ordinal width: one byte with allowed values `1`, `2`, `4`, and `8`;
- lookup algorithm identifier: little-endian `u16` byte length followed by UTF-8 identifier bytes;
- verification mode: one byte, where the required safe mode means exact key verification;
- length-delimited sections for key records, ordinal cells, lookup payload, and algorithm metadata.

Serialization must use little-endian byte order for numeric fields.

Two maps constructed from equivalent canonical input under the same format version must serialize to identical bytes.

Deserialization must reject unknown required format versions, unsupported key encodings, malformed payloads, duplicate keys in stored exact-verification data, duplicate ordinals, invalid ordinal widths, invalid counts, truncated payloads, and inconsistent payload lengths.

### Size introspection

`OrdinalMap[K].nbytes() -> int` should return the number of bytes retained by the ordinal-map payload, excluding ordinary object/header overhead that cannot be made portable across backends.

`OrdinalMap[K].serialized_size() -> int` must return the number of bytes `to_bytes()` would produce without requiring callers to materialize the serialized bytes when practical.

The documentation must distinguish payload size from total process heap usage.

### Error model

The stdlib must define `OrdinalMapError` or an equivalent stable error type. It must expose a stable category string and a human-readable message.

Error categories must include duplicate key, duplicate ordinal, negative ordinal, stored-key hash collision, missing key, malformed serialized data, unsupported serialized version or flags, invalid key encoding, key-encoding mismatch, unsupported lookup or verification mode, unsupported ordinal width, non-canonical serialized payloads, and unsupported metadata. Unsupported key types are compile-time `OrdinalKey` bound diagnostics, not runtime `OrdinalMapError` values.

### Relationship to `dict` and `FrozenDict`

`OrdinalMap` must not change builtin `dict` semantics.

`OrdinalMap` must not change `FrozenDict` semantics.

`OrdinalMap` must live in `std.collections` as a specialized collection for deterministic immutable ordinal lookup. It may share ordinary collection protocols such as `len`, membership, iteration, and indexing where those protocols fit the exact lookup contract.

## Design details

### Lookup algorithm contract

This RFC does not standardize a specific minimal-perfect-hash or fuse-filter algorithm. The public contract is deterministic construction, exact safe lookup, compact ordinal storage, unchecked proven-present lookup, and deterministic serialization.

The implementation must include a deterministic compact lookup representation intended for large maps. It may use a binary-fuse-style structure, a minimal perfect hash family, or another deterministic layout as long as the public requirements hold. It may also choose simpler representations for very small maps if that is faster or smaller after overhead, but a small-map fallback alone does not satisfy the performance goal.

Feature parity with the motivating libraries is a requirement at the capability level, not at the exact type-signature level. `OrdinalMap` must cover construction, lookup, batch lookup, checked lookup, unchecked lookup, serialization, deserialization, and benchmarkable large-map behavior. It deliberately extends the key model beyond strings and bytes through `OrdinalKey`, and it deliberately keeps safe lookup exact rather than making unchecked behavior the ergonomic default.

### Prior art and acknowledgement

Daniel Lemire's [`constmap`](https://github.com/lemire/constmap) project is the immediate practical inspiration for this RFC. The project describes a compact immutable string-to-`uint64` map using the binary fuse filter construction, with lookup shaped around one hash computation, three array accesses, and XOR reconstruction. Its README also calls out the trade-off between unchecked lookup and verified missing-key detection, plus serialization for reuse. `OrdinalMap` should acknowledge that prior art while deliberately choosing a different public contract: deterministic generic ordinal keys, exact safe lookup, and persistent metadata suitability.

The algorithmic family comes from Thomas Mueller Graf and Daniel Lemire's ["Binary Fuse Filters: Fast and Smaller Than Xor Filters"](https://arxiv.org/abs/2201.01174) and the earlier ["Xor Filters: Faster and Smaller Than Bloom and Cuckoo Filters"](https://arxiv.org/abs/1912.08258). This RFC does not require Incan to standardize those algorithms, but they are important prior art for the space/time trade-offs motivating the feature.

### Deterministic retry policy

If the implementation uses retry-based construction, the retry sequence must be deterministic. A conforming strategy is to derive a base seed from the canonical key encodings, map format version, key encoding identifier, and algorithm identifier, then derive attempt seeds from that base seed and a monotonically increasing attempt number.

The implementation must define a deterministic failure mode if construction cannot find a valid layout within its supported attempt budget. Failure must return `OrdinalMapError`; it must not loop indefinitely.

### Exact verification

Safe lookup requires exact missing-key rejection. A compact implementation may use fingerprints as an early reject, but it must be able to verify the queried key exactly before returning an ordinal from safe APIs.

Every serialized map must carry enough exact-verification data to reconstruct the complete set of canonical key bytes and ordinals. Safe lookup must compare the queried key's canonical bytes against exact stored membership data before returning an ordinal. The exact data may be organized as canonical key records, a sorted side table, collision buckets, or another deterministic structure, but it must participate in `to_bytes()` and `from_bytes()` so persisted maps remain exact.

A separate future RFC may define an explicitly probabilistic mode for callers that accept false-positive risk. That mode must not be named or documented as the ordinary safe `OrdinalMap` contract.

### Key encoding and schema evolution

Serialized maps are tied to a key encoding identifier. If a type's canonical encoding changes in a backward-incompatible way, maps serialized with the old encoding must either continue to load through a compatibility path or fail with an unsupported key encoding error.

User-defined key encodings must use a non-empty, versioned `ordinal_encoding()` identifier if they can be persisted. Deserialization must reject persisted maps whose encoding identifier is unsupported, ambiguous, or unversioned for the target key type.

### Dataset and metadata usage

For schema-like data, `OrdinalMap[str]` resolves field names or column names to integer positions. Query and transformation systems can resolve names once during planning, then execute against integer slots.

For immutable dictionary-encoded data, `OrdinalMap[K]` resolves a distinct scalar domain value to a compact code. A scan can compare stored integer codes rather than comparing full key values repeatedly.

For catalogs and generated metadata, `OrdinalMap` gives deterministic bytes that can be cached, diffed, shipped, and validated. This is more important than raw lookup speed alone because metadata reproducibility affects build systems, storage snapshots, and tests.

### API aliases

A future RFC may add aliases for common specializations. For example, `NameIndex` could be an alias for `OrdinalMap[str]` if schema/name lookup needs a clearer user-facing name.

Aliases must not obscure the generic contract. `OrdinalMap[K]` remains the normative type proposed by this RFC.

## Alternatives considered

### String-only map

A string-only map is simpler and matches many schema and catalog use cases. It is too narrow for immutable scalar dictionaries, enum-coded columns, numeric identifiers, UUID-like values, and other deterministic scalar domains. The better boundary is a stable key-encoding contract.

### General frozen dictionary optimization

Optimizing `FrozenDict` would hide a specialized ordinal/code lookup contract inside a general mapping type. That would make missing-key semantics, compact ordinal storage, serialization, and unchecked lookup harder to explain. `FrozenDict` should remain the general frozen dictionary, while `OrdinalMap` owns the specialized deterministic ordinal use case.

### Builtin `dict` with conventions

Users can build a `dict[str, int]`, but that does not give deterministic serialized layout, compact ordinal storage, exact persisted lookup data, or a reproducible construction contract. `dict` remains appropriate for dynamic in-memory maps.

### Fingerprint-only verified map

Fingerprint-only maps can be compact and fast, but they are not exact unless the fingerprint width is treated as part of an explicitly probabilistic contract. Persisted metadata and dataset dictionaries should not silently accept false positives. Fingerprints may be an optimization, but safe lookup must verify exact key membership.

### Full database index abstraction

A database index abstraction would need row identifiers, duplicate keys, range scans, null semantics, updates, segment visibility, and physical planning integration. `OrdinalMap` is smaller: it maps stable keys to integer ordinals. More complete physical indexes can build on ordinal maps where appropriate.

## Drawbacks

`OrdinalMap` adds a specialized collection to the stdlib, and specialized collections carry documentation and teaching cost. The type must justify itself through real uses in schemas, metadata, catalogs, and immutable dictionaries rather than through benchmark novelty.

Exact safe lookup costs more memory than fingerprint-only lookup. This is the right trade-off for persisted and correctness-sensitive data, but it means the most compact benchmark shape cannot be the default safe contract.

Generic key support requires a stable encoding story. If the key contract is too broad, serialized maps become fragile. If it is too narrow, the type becomes a string-only convenience. The required key set must stay tied to deterministic canonical encodings.

Construction can be more expensive than building a `dict`, especially for large maps. The type is intended for build-once, query-many workloads; it should not be promoted for one-off small maps.

Unchecked lookup is useful for proven-present hot paths, but it is easy to misuse. The API must make unsafety visible and examples should prefer exact lookup unless a caller has already validated presence.

## Implementation architecture

This section describes the expected implementation architecture. Algorithm choices may evolve during implementation, but the public contract in this RFC must remain stable.

The recommended shape is an implementation in `std.collections` backed by typed lists, stable hash helpers from `std.hash`, byte serialization helpers from `std.io` and `std.encoding`, and ordinary model/enum definitions for the public error and representation choices.

The implementation must own the full contract: key encoding, validation, construction, lookup, exact verification, unchecked lookup, serialization, deserialization, and size introspection. A small-map representation may be better for tens of keys, while a minimal-perfect or fuse-style representation may be better for thousands or millions of keys. Both can live behind the same public contract when benchmarks justify the split.

Compiler support may recognize hot `OrdinalMap` paths and specialize them when doing so preserves the same public construction semantics and deterministic byte output. In v0.3, `OrdinalKey` support for language scalar families uses a deliberately narrow Rust emission bridge because RFC 098/099 have not yet shipped the full source-level trait-owned capability-family model. That bridge is technical debt, not incomplete RFC 101 scope: the trait contract remains authored in `std.collections`, construction and serialization remain ordinary Incan source, and the bridge should collapse into RFC 098/099 conformance metadata once those RFCs land. The v0.5 cleanup is tracked by issue #596.

## Layers affected

- **Typechecker / Symbol resolution**: validates generic `OrdinalMap[K]` construction against the ordinal-key contract and reports unsupported key types.
- **IR Lowering**: may need to preserve enough static construction information for compile-time baking.
- **Emission**: may emit pre-baked deterministic ordinal-map payloads for module-static or const-compatible construction.
- **Stdlib / Runtime (`incan_stdlib`)**: adds the public `OrdinalMap` type, key-encoding contract, error type, constructors, lookup APIs, and serialization APIs.
- **LSP / Tooling**: should surface completions and hover documentation for `OrdinalMap`, supported key types, missing-key APIs, and unchecked lookup warnings.

## Implementation Plan

### Phase 1: RFC lifecycle, capability intake, and benchmarks

- Capture the settled design decisions in this RFC and move the RFC to active implementation status.
- Inspect existing `.incn` stdlib patterns for generic collections, traits, stable hashing, byte I/O, UUID, and datetime support before choosing representation details.
- Establish benchmark fixtures against builtin `dict`, Python `fastconstmap`, and the prior spike so performance claims are measured rather than assumed.

### Phase 2: `OrdinalKey` contract and deterministic scalar encodings

- Add the public `OrdinalKey` trait in stdlib source.
- Implement deterministic encodings for `str`, `bytes`, `bool`, ordinary `int`, fixed-width signed and unsigned integers, fixed-precision decimals, `UUID`, `Date`, `Time`, `DateTime`, `DateTimeOffset`, stable scalar value enums, and user-defined `OrdinalKey` adopters.
- Add diagnostics or typed errors for unsupported key types and invalid user-defined encoding identifiers.

### Phase 3: `OrdinalMap` construction and lookup

- Add the `OrdinalMap[K]` type, `OrdinalMapError`, and constructors from ordered keys and explicit key/ordinal pairs.
- Reject duplicate keys, duplicate ordinals, negative ordinals, and key-encoding collisions.
- Implement exact safe lookup APIs, batch lookup APIs, membership, length, and mapping-style indexing with `KeyError` behavior.
- Implement explicit unchecked single and batch lookup APIs without routing safe APIs through unchecked behavior.

### Phase 4: Compact storage, deterministic layout, and serialization

- Select `u8`, `u16`, `u32`, or `u64` ordinal storage from the maximum ordinal.
- Implement a deterministic compact lookup layout suitable for large maps, with deterministic retry and failure behavior.
- Implement the standardized byte container, exact verification payload, `to_bytes`, `from_bytes`, `nbytes`, and `serialized_size`.
- Validate deserialization errors for malformed payloads, unsupported versions, unsupported key encodings, duplicate keys, duplicate ordinals, and inconsistent lengths.

### Phase 5: Compiler and tooling support

- Add typechecker support needed for `OrdinalMap[K]` bounds and clear unsupported-key diagnostics.
- Add lowering/emission fixes exposed by the implementation.
- Add static construction or const-baking support if benchmarks show that ordinary module-static `OrdinalMap.from_keys(...)` needs compiler help to meet the feature's performance goal.
- Update LSP hover/completion surfaces for `OrdinalMap`, `OrdinalKey`, safe lookup, and unchecked lookup warnings.

### Phase 6: Docs, release notes, and quality gates

- Update authored stdlib reference docs and collection-selection guidance.
- Add release notes for the target dev release.
- Run focused tests, benchmark verification, and the repository pre-commit gate.

## Implementation log

### Spec / design

- [x] Settle public name as `OrdinalMap`.
- [x] Settle `OrdinalKey` as the stable deterministic key contract.
- [x] Settle non-string key scope, including UUID and civil date/time values.
- [x] Settle duplicate-ordinal rejection for `from_pairs`.
- [x] Settle exact safe lookup and explicit unchecked lookup requirements.
- [x] Settle deterministic serialization container fields.
- [x] Exclude probabilistic fingerprint-only lookup from stdlib safe semantics.

### Stdlib / runtime

- [x] Add `OrdinalKey`.
- [x] Add deterministic key encodings for required builtin scalar types.
- [x] Add deterministic key encodings for UUID and datetime scalar types.
- [x] Add `OrdinalMapError`.
- [x] Add `OrdinalMap[K]` constructors and validation.
- [x] Add exact safe lookup and batch lookup APIs.
- [x] Add unchecked lookup and batch lookup APIs.
- [x] Add compact ordinal storage width selection.
- [x] Add deterministic compact lookup layout for large maps.
- [x] Add deterministic serialization, deserialization, and size introspection.

### Compiler / tooling

- [x] Add or fix typechecker support for `OrdinalKey` bounds and unsupported-key diagnostics.
- [x] Add or fix lowering/emission support exposed by the implementation.
- [x] Evaluate compiler support against benchmark results. The measured lookup gap is in the generated ownership/borrow hot path: exact lookup and batch lookup still clone string/list values at current Incan call boundaries, while unchecked single-key lookup is already below `fastconstmap.ConstMap` in the committed local benchmark. Further borrow-path cleanup is tracked through the RFC 098/099 v0.5 chore instead of changing the RFC 101 public contract.
- [x] Mark the v0.3 scalar `OrdinalKey` Rust bridge as technical debt pending RFC 098/099 trait-owned capability families and track the v0.5 cleanup in issue #596.
- [x] Update LSP metadata for public APIs and unchecked warnings.

### Tests / benchmarks

- [x] Add unit and integration tests for constructors, duplicate handling, lookup, batch lookup, membership, indexing, and missing-key behavior.
- [x] Add serialization round-trip and malformed-payload tests.
- [x] Add deterministic-output tests across equivalent inputs.
- [x] Add tests for required non-string key types.
- [x] Add benchmark coverage against builtin `dict`, Python `fastconstmap`, and the prior spike baseline.

### Docs / release

- [x] Update `std.collections` reference docs.
- [x] Update collection-choice guidance.
- [x] Add release notes entry for the target dev release.
- [x] Run the repository pre-commit gate.

## Design Decisions

1. The public type name is `OrdinalMap`. Aliases such as `NameIndex` may be added later for common specializations, but they must not replace or obscure the generic `OrdinalMap[K]` contract.
2. The key contract is `OrdinalKey`: a type can be used as a key when it can produce deterministic canonical bytes for each value and a stable type-level encoding identifier for serialized maps.
3. `OrdinalMap` is not string-only. Required deterministic keys include `str`, `bytes`, `bool`, ordinary `int`, fixed-width signed and unsigned integers, fixed-precision decimals, UUIDs, civil dates, civil times, civil datetimes, fixed-offset datetimes, stable scalar value enums, and user-defined types that implement `OrdinalKey` with versioned encodings.
4. Floating-point keys are excluded until a separate policy defines canonical NaN, signed-zero, and payload behavior.
5. `from_pairs` rejects duplicate ordinals. Many-to-one labels are a different abstraction and should not be hidden inside `OrdinalMap`.
6. `get` returns `Option[int]`, `require` returns `Result[int, OrdinalMapError]`, and indexing returns an `int` for present keys while raising a `KeyError`-style runtime error for missing keys.
7. Unchecked lookup ships as an explicit non-default API for proven-present hot paths and benchmark ownership. Safe APIs must remain exact and must not depend on unchecked behavior.
8. Serialized maps use the standardized `INCAN_ORDMAP` container fields in this RFC, little-endian numeric fields, stable key encoding identifiers, compact ordinal width metadata, algorithm metadata, and exact verification data.
9. Safe lookup must be exact. Probabilistic fingerprint-only behavior is outside the stdlib safe contract.
10. Compiler optimizations may precompute or specialize `OrdinalMap`, but they must preserve the public construction semantics and deterministic serialized output.
11. The v0.3 implementation may use a narrow Rust bridge for scalar `OrdinalKey` families until RFC 098/099 can express trait-owned capability families in source; that bridge must remain explicit and removable, with removal tracked as v0.5 chore #596.
