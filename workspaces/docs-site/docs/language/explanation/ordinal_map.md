# Why `OrdinalMap` exists

`OrdinalMap[K]` is a deterministic bridge from a stable key domain to integer positions. It exists for code that wants to stop carrying names, UUIDs, dates, or enum values through hot paths once those values have been validated.

The common shape is:

1. Build or load a fixed key domain.
2. Validate user-facing names or external values against that domain.
3. Execute later work against integer ordinals.
4. Persist the same key-to-ordinal contract as reproducible bytes.

That makes `OrdinalMap` useful for schemas, generated catalogs, dictionary-encoded scalar domains, query plans, and dataset sidecars.

## The table-index case

Imagine a table with 200 columns and billions of rows. Query text, file metadata, and user APIs naturally talk about columns by name:

```text
status, amount, customer_id
```

Execution engines do not want to compare those names repeatedly while scanning data. They want column slots:

```text
2, 3, 1
```

`OrdinalMap[str]` lets a planner resolve names once and then carry integer positions through the plan. The map is not the physical table index by itself. It is the stable name-to-slot primitive that larger table indexes, catalogs, and InQL-style planners can build on.

The same idea applies when the key is not text. A dataset may need stable ordinals for UUID tenants, civil dates, fixed enum values, or generated identifiers. `OrdinalMap` handles those through the `OrdinalKey` contract rather than forcing users to stringify precise values.

## Why not `dict` or `FrozenDict`

A builtin `dict[K, int]` is the right default when the mapping is ordinary program state: mutable, incrementally built, and not meant to define a portable byte contract.

`FrozenDict` is the right abstraction for a general immutable mapping. Its value type is arbitrary, and its job is to preserve mapping semantics.

`OrdinalMap` is narrower:

- values are ordinals;
- construction validates a complete fixed domain;
- serialization is deterministic;
- key encodings are explicit;
- safe lookup is exact;
- unchecked lookup is opt-in for proven-present hot paths;
- compact ordinal storage can choose the smallest payload width that fits the maximum ordinal.

Keeping those semantics in a separate type makes the contract visible instead of hiding it inside a general mapping.

## Determinism is the point

For generated metadata and large datasets, deterministic output matters as much as lookup speed. Two equivalent maps should serialize to identical bytes. That lets callers cache, diff, ship, and validate indexes without relying on process-local hash order or incidental construction order.

`from_keys` uses input order as the ordinal contract. `from_pairs` accepts explicit ordinals and canonicalizes serialized output so pair order does not matter. `from_bytes` validates the stored key encoding before returning a map, so an `OrdinalMap[Date]` payload cannot be silently decoded as a string-key map.

## Exact lookup stays the default

Compact immutable-map designs often expose an unchecked path because it can be much faster. The tradeoff is missing-key behavior: if the caller asks for a key outside the known domain, an unchecked structure may fail in implementation-specific ways or reconstruct an ordinal for a different stored key.

Incan keeps exact lookup as the default contract. `get`, `require`, membership, indexing, `get_many`, and `require_many` verify that the queried key matches the stored key before returning an ordinal.

Unchecked lookup remains available because it is useful after validation. A planner can resolve and validate a query's names once, then execute with `get_unchecked` or `get_many_unchecked` because the plan already proved the keys are present.

## Key types are intentionally scalar

`OrdinalKey` is the boundary for key types. A key type must provide canonical bytes for each value and a stable type-level encoding identifier for serialized maps.

The supported surface is deterministic scalar data: text, bytes, booleans, integers, fixed-precision decimals, UUIDs, civil dates and times, stable scalar value enums, and user-defined types that implement `OrdinalKey`.

Decimals are accepted because `decimal[p, s]` is precise and compiler-checked. The key bytes use the `Decimal128` runtime representation: the signed coefficient in little-endian bytes followed by the stored scale byte.

Floats are excluded because NaN payloads, negative zero, infinities, and cross-language canonicalization need their own explicit design. If a domain behaves like money, use a decimal value or scaled integer. If it behaves like a finite measurement code, model that code directly.

## Performance model

`OrdinalMap` is optimized for known key domains and repeated lookup, not for incremental mutation. Construction pays validation cost up front. Lookup then works against compact payload sections plus runtime caches.

The repository benchmark in `workspaces/benchmarks/collections/ordinal_map` compares:

- Python `dict`;
- Python `fastconstmap.ConstMap`;
- Python `fastconstmap.VerifiedConstMap`;
- Incan `OrdinalMap[str]`;
- the earlier handwritten Rust spike baseline.

Treat benchmark numbers as measured local data, not API guarantees. The latest 1,000,000-key comparison is a single directional local run rather than a median over repeated samples. In that run, `OrdinalMap[str]` exact single-key lookup was lower than Python plus `fastconstmap.VerifiedConstMap`, and unchecked single-key lookup was lower than `fastconstmap.ConstMap`. Exact batch lookup remained slower than `fastconstmap.VerifiedConstMap`; unchecked batch lookup was lower than `fastconstmap.ConstMap`. `OrdinalMap` uses more payload bytes per key and construction remains slower because construction validates and canonicalizes records before producing deterministic payload sections.

The public value of `OrdinalMap` does not depend on a benchmark claim alone. The durable contract is deterministic exact key-to-ordinal lookup over scalar key domains, with explicit serialization and a separate unchecked path for validated hot loops.

## Prior art

`OrdinalMap` is informed by compact immutable map work such as Daniel Lemire's [`constmap`](https://github.com/lemire/constmap) and related binary-fuse/xor-filter designs. Incan adapts the useful lessons: known-key construction, compact storage, checked and unchecked lookup modes, serialization, and benchmarkable large-map behavior.

The Incan contract is deliberately different from a string-only `uint64` map. It is generic over deterministic scalar keys, keeps safe lookup exact by default, and models the value as an ordinal/code rather than an arbitrary integer payload.

## See also

- [Choosing collection types](../how-to/choosing_collections.md)
- [`std.collections` reference](../reference/stdlib/collections.md)
- [RFC 101: `std.collections.OrdinalMap`](../../RFCs/closed/implemented/101_std_collections_ordinal_map.md)
