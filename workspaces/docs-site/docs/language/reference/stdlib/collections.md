# std.collections reference

`std.collections` provides specialized collection types for cases where builtin `list`, `dict`, and `set` are too broad. Use these types when the collection semantics are part of the program contract.

```incan
from std.collections import ChainMap, Counter, DefaultDict, Deque, OrderedDict, OrderedSet, OrdinalMap, PriorityQueue, SortedDict, SortedSet
```

The module is imported explicitly and has no feature gate.

## Types

| Type | Purpose |
| --- | --- |
| `Deque[T]` | Double-ended queue with efficient front and back operations. |
| `Counter[T]` | Multiset that stores occurrence counts. |
| `DefaultDict[K, V]` | Mapping that materializes a configured default value for missing keys. |
| `OrderedDict[K, V]` | Mapping whose iteration order follows insertion order. |
| `OrderedSet[T]` | Set whose iteration order follows insertion order. |
| `SortedDict[K, V]` | Mapping whose iteration order follows sorted key order. |
| `SortedSet[T]` | Set whose iteration order follows sorted value order. |
| `ChainMap[K, V]` | Layered lookup across mapping and record-shaped layers. |
| `PriorityQueue[T]` | Heap-shaped queue for priority ordering. |
| `OrdinalMap[K]` | Immutable deterministic key-to-ordinal lookup for stable scalar keys. |

Builtin collections remain the default for ordinary data. Reach for `std.collections` when the named behavior matters: front-of-queue work, counted membership, missing-key defaulting, insertion-order stability, sorted traversal, layered configuration, or priority scheduling.

For task-oriented guidance, see [Choosing collection types](../../how-to/choosing_collections.md).

## OrdinalMap

`OrdinalMap[K]` maps each unique key to a non-negative integer ordinal. Use it when a stable key domain needs stable integer positions: schema field indexes, column catalogs, generated metadata, dictionary-encoded values, or cached lookup tables whose bytes must be reproducible.

`OrdinalMap` is immutable after construction. It is not a replacement for mutable `dict`, insertion-ordered `OrderedDict`, or general-purpose `FrozenDict`. Its public contract is deterministic construction, exact safe lookup, deterministic serialization, and compact ordinal storage.

Keys must implement the `OrdinalKey` trait. `OrdinalKey` provides deterministic canonical bytes for each key value and a stable key-encoding identifier for serialized maps. For task-oriented examples, see [Choosing collection types](../../how-to/choosing_collections.md). For the design model, see [Why `OrdinalMap` exists](../../explanation/ordinal_map.md).

### API summary

| API | Returns | Description |
| --- | --- | --- |
| `OrdinalMap.from_keys(keys: list[K])` | `Result[OrdinalMap[K], OrdinalMapError]` | Construct a map where each key's ordinal is its zero-based position in `keys`. Rejects duplicate keys. |
| `OrdinalMap.from_pairs(entries: list[tuple[K, int]])` | `Result[OrdinalMap[K], OrdinalMapError]` | Construct a map from explicit ordinals. Rejects duplicate keys, negative ordinals, and duplicate ordinals. |
| `map.get(key)` | `Option[int]` | Exact safe lookup; returns `None` when the key is absent. |
| `map.require(key)` | `Result[int, OrdinalMapError]` | Exact safe lookup; returns an error when the key is absent. |
| `map[key]` | `int` | Exact indexing lookup using Incan's ordinary missing-key behavior. Prefer `get` or `require` when absence is part of normal control flow. |
| `key in map` | `bool` | Exact membership check. |
| `map.get_many(keys: list[K])` | `list[Option[int]]` | Batch exact lookup that preserves input order. |
| `map.require_many(keys: list[K])` | `Result[list[int], OrdinalMapError]` | Batch exact lookup that fails if any key is absent. |
| `map.get_unchecked(key)` | `int` | Explicit non-default lookup for callers that have already proven the key is present. Missing-key behavior is implementation-specific. |
| `map.get_many_unchecked(keys: list[K])` | `list[int]` | Batch unchecked lookup. Preserves input order. |
| `map.keys()` | `list[K]` | Return stored keys in deterministic canonical order. |
| `map.keys_list()` | `list[K]` | Compatibility alias for `keys()`. |
| `map.key_count()` | `int` | Return the number of keys. Equivalent to `len(map)`. |
| `map.max_ordinal()` | `int` | Return the highest stored ordinal, or `-1` for an empty map. |
| `map.ordinal_width_bytes()` | `int` | Return the compact cell width selected for stored ordinals. |
| `map.slot_count()` | `int` | Return the number of open-addressing lookup slots. |
| `map.storage_bytes()` | `int` | Return compact payload bytes retained by key, offset, ordinal, lookup, and metadata sections. |
| `map.to_bytes()` | `bytes` | Deterministic serialized representation. |
| `OrdinalMap[K].from_bytes(data: bytes)` | `Result[OrdinalMap[K], OrdinalMapError]` | Parse and validate a serialized ordinal map. |
| `map.nbytes()` | `int` | Bytes retained by the compact ordinal-map payload sections, excluding ordinary object/header overhead and runtime lookup caches. |
| `map.serialized_size()` | `int` | Number of bytes `to_bytes()` would produce. |

### Semantics

`from_keys` assigns ordinals from the input order. `from_pairs` keeps caller-supplied non-negative ordinals. Both constructors reject duplicate keys; `from_pairs` also rejects duplicate ordinals.

Safe lookup is exact. `get`, `require`, membership, indexing, `get_many`, and `require_many` verify that the queried key's canonical bytes match the stored key before returning an ordinal.

Unchecked lookup skips exact key-byte verification after the hash probe. It is for hot paths where key presence has already been established by a planner, schema validator, or prior exact lookup. Missing-key behavior is implementation-specific.

### Supported key types

| Key family | Examples | Notes |
| --- | --- | --- |
| Text and bytes | `str`, `bytes` | Strings use UTF-8 canonical bytes; bytes use the byte payload directly. |
| Booleans | `bool` | Encoded as a stable one-byte domain. |
| Integers | `int`, `i8`, `i16`, `i32`, `i64`, `i128`, `u8`, `u16`, `u32`, `u64`, `u128`, and exact-width aliases | Ordinary `int` uses Incan's stable `i64` encoding. Pointer-sized `isize` and `usize` are not accepted because their width is target-dependent. |
| Decimals | `decimal[p, s]`, `numeric[p, s]`, `decimal128[p, s]` | Encoded through the deterministic `Decimal128` runtime representation: little-endian signed coefficient plus stored scale. Binary floating-point types are not accepted. |
| UUIDs | `std.uuid.UUID` | Uses the UUID's stable 128-bit value encoding. |
| Civil temporal values | `Date`, `Time`, `DateTime`, `DateTimeOffset` | Date/time values are precise and deterministic, so users do not need to convert them to strings. |
| Value enums | `enum Status(str): ...` or `enum Code(int): ...` | Stable scalar value enums are accepted. Payload enums are not accepted as ordinal keys. |
| User-defined keys | `model Key with OrdinalKey: ...` | Implement `ordinal_encoding`, `from_ordinal_bytes`, and `ordinal_bytes`. |

Floats are outside the `OrdinalKey` surface. Use a deterministic domain model, integer-scaled value, decimal value, or explicit string representation when a floating-point-like concept needs indexing.

### OrdinalKey

```incan
pub trait OrdinalKey:
    @staticmethod
    def ordinal_encoding() -> str: ...

    @staticmethod
    def from_ordinal_bytes(data: bytes) -> Result[Self, OrdinalMapError]: ...

    def ordinal_bytes(self) -> bytes: ...

    def ordinal_hash(self) -> int: ...

    def ordinal_bytes_equal(self, data: bytes) -> bool: ...
```

`ordinal_bytes(self)` returns canonical bytes for a key value. `ordinal_encoding()` returns the stable type-level encoding identifier stored in serialized maps. `from_ordinal_bytes(data)` decodes one stored key record for `OrdinalMap[K].from_bytes(data)`.

The `ordinal_encoding()` string is part of the serialized type contract. Change it when the byte layout changes so old payloads fail clearly instead of decoding with the wrong shape.

User-defined keys usually implement only `ordinal_encoding`, `from_ordinal_bytes`, and `ordinal_bytes`. The default `ordinal_hash` and `ordinal_bytes_equal` methods derive from `ordinal_bytes`; builtin key implementations override them so hot lookups can hash and compare without materializing a fresh byte vector.

User-defined key implementations should keep `ordinal_encoding()` stable for a given byte layout and should change it when stored bytes are no longer compatible.

### Serialization and size

The serialized container uses the `INCAN_ORDMAP` magic value and records the format version, key encoding identifier, key count, ordinal width, lookup algorithm identifier, exact-verification mode, construction metadata, and lookup/value payload. Serialization is deterministic: equivalent canonical input under the same format version produces identical bytes.

`from_bytes` validates the container before returning a map. It rejects malformed payloads, unsupported versions, unsupported lookup modes, non-canonical ordinal widths, key encoding mismatches, and lookup payloads that do not match the stored keys and ordinals.

Compact ordinal width is selected from the maximum ordinal. Maps whose maximum ordinal fits in `u8`, `u16`, `u32`, or `u64` use that width internally and in the serialized payload. Public lookup still returns ordinary `int`.

`nbytes()` and `storage_bytes()` report compact payload sections: key bytes, key offsets, ordinal cells, lookup slots, and metadata. They do not report total process heap, ordinary object/header overhead, or runtime lookup caches used by the implementation. Use `serialized_size()` when comparing persisted payload size.

### OrdinalMapError

`OrdinalMapError` exposes typed and text accessors for branching and diagnostics:

| API | Meaning |
| --- | --- |
| `err.kind()` | `OrdinalMapErrorKind` category for source-level branching. |
| `err.kind_name()` | Stable category string for diagnostics and text compatibility. |
| `err.message()` | Human-readable explanation. |
| `err.index()` | Input or batch position when there is a meaningful position, otherwise `-1`. |

Use `err.kind()` when branching in new code and `err.kind_name()` when a stable string is needed.

Common construction and lookup error kinds:

| Kind | When it happens |
| --- | --- |
| `duplicate_key` | Two input keys have the same canonical key bytes. |
| `duplicate_ordinal` | Two `from_pairs` entries use the same ordinal. |
| `negative_ordinal` | A `from_pairs` entry uses a negative ordinal. |
| `hash_collision` | Two stored keys collide in the stable lookup hash domain. |
| `missing_key` | `require` or `require_many` cannot find a key. |

Common decoding error kinds:

| Kind | When it happens |
| --- | --- |
| `truncated` or `invalid_length` | The byte payload ends early or declares an invalid section length. |
| `invalid_magic` | The byte payload is not an `INCAN_ORDMAP` container. |
| `unsupported_version` | The container format version is not supported by this runtime. |
| `key_encoding_mismatch` | The payload was built for a different key type or encoding version. |
| `invalid_lookup_payload` | The stored lookup section does not match the stored keys and ordinals. |

## Deque

`Deque[T]` stores values in front-to-back order and supports both Python-style and Rust-style method names for the same end operations.

| API | Returns | Description |
| --- | --- | --- |
| `Deque[T]()` | `Deque[T]` | Construct an empty deque. |
| `len(deque)` | `int` | Number of elements. |
| `deque.push_back(value)` / `deque.append(value)` | `None` | Add to the back. |
| `deque.push_front(value)` / `deque.appendleft(value)` | `None` | Add to the front. |
| `deque.pop_back()` / `deque.pop()` | `T` | Remove from the back. Empty queues fail through builtin list indexing behavior. |
| `deque.pop_front()` / `deque.popleft()` | `T` | Remove from the front. Empty queues fail through builtin list indexing behavior. |
| `deque.to_list()` | `list[T]` | Snapshot values in front-to-back order. |
| `deque.clear()` | `None` | Remove all values. |

## Counter

`Counter[T]` stores counts by element. Missing elements read as zero.

| API | Returns | Description |
| --- | --- | --- |
| `Counter[T]()` | `Counter[T]` | Construct an empty counter. |
| `Counter.from_iter(values: list[T])` | `Counter[T]` | Count values from an iterable collection. |
| `counter.get(value)` | `int` | Count for one value, or zero when absent. |
| `counter.set(value, count: int)` | `None` | Set the count for one value. |
| `counter.update(values: list[T])` | `None` | Add one count for each value. |
| `counter.subtract(values: list[T])` | `None` | Subtract one count for each value. |
| `counter.total()` | `int` | Sum of counts. |
| `counter.most_common(n: int = -1)` | `list[tuple[T, int]]` | Highest-count elements, or all elements when `n` is negative. |
| `counter.elements()` | `list[T]` | Expand positive counts back into repeated elements. |

## DefaultDict

`DefaultDict[K, V]` makes missing-key defaulting explicit. Use it when default creation is part of the data model rather than a local convenience branch around an ordinary `dict`.

| API | Returns | Description |
| --- | --- | --- |
| `DefaultDict.with_default(value: V)` | `DefaultDict[K, V]` | Construct a map that clones `value` for missing keys. |
| `DefaultDict.with_factory(factory: () -> V)` | `DefaultDict[K, V]` | Construct a map that calls `factory` for missing keys. |
| `default_dict[key]` | `V` | Return the existing value or materialize the default. |
| `default_dict.get(key)` | `Option[V]` | Return the existing value without materializing. |
| `default_dict.set(key, value)` | `None` | Store a value. |
| `key in default_dict` | `bool` | Whether the key is present. |
| `default_dict.keys()` | `list[K]` | Current keys. |
| `default_dict.values()` | `list[V]` | Current values. |
| `default_dict.items()` | `list[tuple[K, V]]` | Current key/value pairs. |

## Ordered Collections

`OrderedDict[K, V]` and `OrderedSet[T]` preserve insertion order for iteration, display, and order-sensitive serialization.

| API | Returns | Description |
| --- | --- | --- |
| `OrderedDict[K, V]()` | `OrderedDict[K, V]` | Construct an empty ordered map. |
| `ordered_dict.set(key, value)` | `None` | Insert or replace a key without duplicating its position. |
| `ordered_dict.get(key)` | `Option[V]` | Lookup by key. |
| `ordered_dict.remove(key)` | `V` | Remove one key. Missing keys fail through builtin list indexing behavior. |
| `ordered_dict.keys()` | `list[K]` | Keys in insertion order. |
| `ordered_dict.values()` | `list[V]` | Values in key insertion order. |
| `ordered_dict.items()` | `list[tuple[K, V]]` | Items in key insertion order. |
| `OrderedSet[T]()` | `OrderedSet[T]` | Construct an empty ordered set. |
| `ordered_set.add(value)` | `None` | Add a value if it is absent. |
| `ordered_set.remove(value)` | `None` | Remove a value when present. |
| `ordered_set.contains(value)` | `bool` | Membership check. |
| `ordered_set.to_list()` | `list[T]` | Values in insertion order. |

## Sorted Collections

`SortedDict[K, V]` and `SortedSet[T]` preserve sorted order for iteration and range-oriented work. Keys or values must support the ordinary ordering protocol.

| API | Returns | Description |
| --- | --- | --- |
| `SortedDict[K, V]()` | `SortedDict[K, V]` | Construct an empty sorted map. |
| `sorted_dict.set(key, value)` | `None` | Insert or replace a key. |
| `sorted_dict.get(key)` | `Option[V]` | Lookup by key. |
| `sorted_dict.remove(key)` | `V` | Remove one key. Missing keys fail through builtin list indexing behavior. |
| `sorted_dict.keys()` | `list[K]` | Keys in sorted order. |
| `sorted_dict.values()` | `list[V]` | Values in sorted-key order. |
| `sorted_dict.items()` | `list[tuple[K, V]]` | Items in sorted-key order. |
| `SortedSet[T]()` | `SortedSet[T]` | Construct an empty sorted set. |
| `sorted_set.add(value)` | `None` | Add a value if it is absent. |
| `sorted_set.remove(value)` | `None` | Remove a value when present. |
| `sorted_set.contains(value)` | `bool` | Membership check. |
| `sorted_set.range(start, end)` | `list[T]` | Values where `start <= value < end`. |
| `sorted_set.to_list()` | `list[T]` | Values in sorted order. |

## ChainMap

`ChainMap[K, V]` performs layered lookup from first layer to last. Earlier layers override later layers.

| API | Returns | Description |
| --- | --- | --- |
| `ChainMap.from_layers(layers: list[OrderedDict[K, V]])` | `ChainMap[K, V]` | Construct a layered map. |
| `chain[key]` | `V` | Return the first matching value. Missing keys fail through builtin list indexing behavior. |
| `chain.set(key, value)` | `None` | Write to the first writable mapping layer. |
| `chain.contains(key)` | `bool` | Whether any layer contains the key. |
| `chain.keys()` | `list[K]` | Distinct visible keys. |
| `chain.items()` | `list[tuple[K, V]]` | Distinct visible key/value pairs. |
| `chain.push_layer(layer)` | `None` | Add a highest-precedence mapping layer. |
| `ChainMap.from_field_layers(layers: list[list[tuple[K, V]]])` | `ChainMap[K, V]` | Construct from read-only field snapshots such as `model.__field_items__()`. |
| `chain.push_field_layer(layer)` | `None` | Add a highest-precedence read-only field snapshot. |
| `chain.pop_layer()` | `OrderedDict[K, V]` | Remove the highest-precedence mapping layer. |

Model and class field overlays use compiler-generated `__field_value__(name: str) -> Option[T]` and `__field_items__() -> list[tuple[str, T]]` views, where `T` is either the common field type or a union of the exposed field types. `ChainMap` accepts `__field_items__()` snapshots through `from_field_layers()` and `push_field_layer()`; these layers are read-only snapshots and preserve the field ordering reported by `__fields__()`.

## PriorityQueue

`PriorityQueue[T]` stores values according to priority order.

| API | Returns | Description |
| --- | --- | --- |
| `PriorityQueue[T]()` | `PriorityQueue[T]` | Construct an empty priority queue. |
| `PriorityQueue.with_order(order)` | `PriorityQueue[T]` | Construct an empty queue with an explicit ordering policy. |
| `PriorityQueue.max_first()` | `PriorityQueue[T]` | Construct an empty max-first queue. |
| `PriorityQueue.from_iter(values, order)` | `PriorityQueue[T]` | Construct a queue from values and an ordering policy. |
| `queue.push(value)` | `None` | Add a value. |
| `queue.peek()` | `Option[T]` | Read the next value without removing it. |
| `queue.pop()` | `T` | Remove and return the next value. Empty queues fail through builtin list indexing behavior. |
| `len(queue)` | `int` | Number of queued values. |
| `queue.is_empty()` | `bool` | Whether the queue is empty. |
| `queue.to_list()` | `list[T]` | Queued values in pop order. |

Values must support the ordering protocol used by the queue.
