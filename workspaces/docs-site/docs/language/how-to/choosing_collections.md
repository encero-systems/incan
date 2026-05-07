# Choosing collection types

Use builtin `list`, `dict`, and `set` first. Reach for `std.collections` when the collection's behavior is part of the program design rather than just a storage detail.

| Need | Use |
| --- | --- |
| Append and pop at both ends | `Deque[T]` |
| Count occurrences | `Counter[T]` |
| Materialize a value for missing keys | `DefaultDict[K, V]` |
| Keep insertion order visible | `OrderedDict[K, V]` or `OrderedSet[T]` |
| Traverse in sorted order | `SortedDict[K, V]` or `SortedSet[T]` |
| Layer overrides over defaults | `ChainMap[K, V]` |
| Always process the next highest-priority item | `PriorityQueue[T]` |

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

## See also

- [std.collections reference](../reference/stdlib/collections.md)
- [Collections and iteration tutorial](../tutorials/book/08_collections_and_iteration.md)
- [Collection protocols](../reference/stdlib_traits/collection_protocols.md)
