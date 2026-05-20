# Choosing collection types

Use builtin `list`, `dict`, and `set` first. Reach for `std.collections` when the collection's behavior is part of the program design rather than just a storage detail.

| Need                                                   | Use                                    |
| ------------------------------------------------------ | -------------------------------------- |
| Append and pop at both ends                            | `Deque[T]`                             |
| Count occurrences                                      | `Counter[T]`                           |
| Materialize a value for missing keys                   | `DefaultDict[K, V]`                    |
| Keep insertion order visible                           | `OrderedDict[K, V]` or `OrderedSet[T]` |
| Traverse in sorted order                               | `SortedDict[K, V]` or `SortedSet[T]`   |
| Layer overrides over defaults                          | `ChainMap[K, V]`                       |
| Always process the next highest-priority item          | `PriorityQueue[T]`                     |
| Resolve a fixed key domain to stable integer positions | `OrdinalMap[K]`                        |

## Use `Deque` for front-and-back queues

Use `Deque[T]` when both ends are active. If you only append and pop at the end, a builtin `list` is simpler.

```incan
from std.collections import Deque

def main() -> None:
    mut queue = Deque[str]()
    queue.append("normal")
    queue.appendleft("urgent")

    next = queue.popleft()
    println(next)
```

`Deque` exposes both Python-style names (`appendleft`, `popleft`) and Rust-style names (`push_front`, `pop_front`). Choose one style in a module and stay consistent.

## Use `Counter` for counted membership

Use `Counter[T]` when the count is the data model. Do not hand-roll a `dict[T, int]` unless the update rules are unusual enough that the named collection would hide the intent.

```incan
from std.collections import Counter

def main() -> None:
    counts = Counter.from_iter(["red", "blue", "red"])

    assert counts.get("red") == 2
    assert counts.total() == 3
```

## Use `DefaultDict` when missing keys should create values

Use `DefaultDict[K, V]` when reading a missing key should materialize a default. Use `get()` when you want to inspect a key without creating it.

```incan
from std.collections import DefaultDict

def zero() -> int:
    return 0

def main() -> None:
    mut counts: DefaultDict[str, int] = DefaultDict.with_factory(zero)
    counts["red"] = counts["red"] + 1

    assert counts.get("red") == Some(1)
```

Use `with_default(value)` when cloning one value is right for every missing key. Use `with_factory(factory)` when each missing key should get a fresh value.

## Use ordered collections when insertion order matters

Use `OrderedDict[K, V]` and `OrderedSet[T]` when display, serialization, or user-facing iteration must follow insertion order. Use builtin `dict` or `set` when order is not part of the contract.

```incan
from std.collections import OrderedDict, OrderedSet

def main() -> None:
    mut headers = OrderedDict[str, str]()
    headers.set("content-type", "application/json")
    headers.set("cache-control", "no-store")

    assert headers.keys() == ["content-type", "cache-control"]

    mut seen = OrderedSet[str]()
    seen.add("alice")
    seen.add("bob")
    seen.add("alice")

    assert seen.to_list() == ["alice", "bob"]
```

## Use sorted collections for deterministic sorted traversal

Use `SortedDict[K, V]` and `SortedSet[T]` when consumers rely on sorted order. The key or value type must support normal ordering.

```incan
from std.collections import SortedDict, SortedSet

def main() -> None:
    mut scores = SortedDict[str, int]()
    scores.set("bob", 7)
    scores.set("alice", 10)

    assert scores.keys() == ["alice", "bob"]

    mut ids = SortedSet[int]()
    ids.add(30)
    ids.add(10)
    ids.add(20)

    assert ids.to_list() == [10, 20, 30]
```

## Use `ChainMap` for layered configuration

Use `ChainMap[K, V]` when lookup should prefer overrides but fall back to defaults. Earlier layers win.

```incan
from std.collections import ChainMap, OrderedDict

def main() -> None:
    overrides = OrderedDict.from_items([("region", "us-east-1")])
    defaults = OrderedDict.from_items([("region", "eu-west-1"), ("retries", "3")])

    cfg = ChainMap.from_layers([overrides, defaults])

    assert cfg["region"] == "us-east-1"
    assert cfg["retries"] == "3"
```

For model or class defaults, pass a field snapshot. The snapshot is read-only inside the chain map.

```incan
from std.collections import ChainMap, OrderedDict

model RuntimeDefaults:
    host: int
    port: int

def main() -> None:
    defaults = RuntimeDefaults(host=2, port=3)
    mut cfg: ChainMap[str, int] = ChainMap.from_field_layers([defaults.__field_items__()])

    cfg.push_layer(OrderedDict.from_items([("host", 1)]))

    assert cfg["host"] == 1
    assert cfg["port"] == 3
```

## Use `PriorityQueue` for next-item scheduling

Use `PriorityQueue[T]` when every pop should return the next item by ordering. The default is min-first; use `PriorityOrder.MaxFirst` when larger values should come first.

```incan
from std.collections import PriorityOrder, PriorityQueue

def main() -> None:
    mut next_retry = PriorityQueue[int]()
    next_retry.push(30)
    next_retry.push(10)
    next_retry.push(20)

    assert next_retry.pop() == 10

    mut largest_first = PriorityQueue[int].with_order(PriorityOrder.MaxFirst)
    largest_first.push(30)
    largest_first.push(10)

    assert largest_first.pop() == 30
```

## Use `OrdinalMap` for stable key-to-position lookup

Use `OrdinalMap[K]` when the keys are known up front and each key needs a stable integer ordinal. This is a good fit for schema fields, column names, generated catalogs, dictionary-encoded scalar domains, and query-planning metadata.

Use builtin `dict[K, int]` when the map is mutable, built incrementally, or does not need deterministic serialized bytes. Use `OrderedDict[K, V]` when insertion order is the contract and values are arbitrary. Use `OrdinalMap[K]` when the value is specifically the key's integer position/code and the map should be immutable after construction.

### Build a schema index

Use `from_keys` when the input order is the ordinal contract. The first key maps to `0`, the second key maps to `1`, and so on.

```incan
from std.collections import OrdinalMap, OrdinalMapError, OrdinalMapErrorKind

def main() -> Result[None, OrdinalMapError]:
    columns = OrdinalMap.from_keys(["order_id", "customer_id", "status", "amount"])?

    assert columns.require("status")? == 2
    assert columns.get("missing") == None
    assert columns.require_many(["status", "amount"])? == [2, 3]

    return Ok(None)
```

Construction rejects duplicate keys because the key domain must be one-to-one with ordinals.

```incan
duplicate = OrdinalMap.from_keys(["status", "status"])
match duplicate:
    Ok(_) => assert False
    Err(err) => assert err.kind() == OrdinalMapErrorKind.DuplicateKey
```

### Use external ordinals

Use `from_pairs` when ordinals come from an external schema, persisted catalog, or wire contract. Pair order does not affect serialization; equivalent key/ordinal pairs produce identical bytes.

```incan
field_ids = OrdinalMap.from_pairs([
    ("order_id", 10),
    ("customer_id", 11),
    ("status", 12),
])?

same_ids = OrdinalMap.from_pairs([
    ("status", 12),
    ("order_id", 10),
    ("customer_id", 11),
])?

assert field_ids.to_bytes() == same_ids.to_bytes()
```

`from_pairs` rejects duplicate keys, negative ordinals, and duplicate ordinals. If one ordinal should point at many labels, use a different model such as `dict[int, list[str]]`.

### Choose the lookup path

Use `get` when absence is normal control flow.

```incan
match columns.get("discount_code"):
    Some(index) => println(f"column index: {index}")
    None => println("column not present")
```

Use `require` when absence should be a recoverable error value.

```incan
status_index = columns.require("status")?
```

Use indexing only when the key is expected to be present and ordinary mapping-style failure is acceptable.

```incan
status_index = columns["status"]
```

Use `get_unchecked` only when an external invariant or prior exact check has already proven that the key is present. A query planner, for example, can validate user-supplied names once and then use unchecked lookup while executing a hot loop over already-validated fields.

```incan
if "status" in columns:
    status_index = columns.get_unchecked("status")
```

### Use non-string keys

`OrdinalMap` requires `OrdinalKey` keys with deterministic canonical bytes and a stable encoding identifier. The standard key surface covers deterministic scalar keys such as `str`, `bytes`, `bool`, integers, fixed-precision decimal values, UUID values, date/time values, stable value enums, and user-defined adopters. Floats are excluded; use a deterministic domain model, integer-scaled value, decimal value, or explicit string representation for floating-point-like concepts.

```incan
from std.collections import OrdinalMap
from std.datetime import Date
from std.uuid import UUID

price: decimal[5, 2] = 19.99d
fee: decimal[5, 2] = 1.25d
amount_codes = OrdinalMap.from_pairs([(price, 1999), (fee, 125)])?

statement_days = OrdinalMap.from_keys([
    Date(year=2026, month=5, day=1),
    Date(year=2026, month=5, day=2),
])?

tenant = UUID.from_int(113059749145936325402354257176981405696)
tenant_slots = OrdinalMap.from_pairs([(tenant, 42)])?

assert statement_days.require(Date(year=2026, month=5, day=2))? == 1
assert tenant_slots.require(tenant)? == 42
assert amount_codes.require(price)? == 1999
```

### Persist an index

Use `to_bytes` when a generated catalog or dataset sidecar needs reproducible bytes. Use `from_bytes` with an explicit type annotation so the stored key encoding is checked against the key type you expect.

```incan
blob = columns.to_bytes()
restored: OrdinalMap[str] = OrdinalMap.from_bytes(blob)?

assert restored.to_bytes() == blob
assert restored.require("status")? == columns.require("status")?
```

Use `serialized_size()` when comparing persisted payload size. Use `nbytes()` or `storage_bytes()` when comparing compact payload sections in memory; those values exclude ordinary object/header overhead and runtime lookup caches.

## See also

- [std.collections reference](../reference/stdlib/collections.md)
- [Why `OrdinalMap` exists](../explanation/ordinal_map.md)
- [Collections and iteration tutorial](../tutorials/book/08_collections_and_iteration.md)
- [Collection protocols](../reference/stdlib_traits/collection_protocols.md)
