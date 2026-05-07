# std.collections reference

`std.collections` provides specialized collection types for cases where builtin `list`, `dict`, and `set` are too broad. Use these types when the collection semantics are part of the program contract.

```incan
from std.collections import ChainMap, Counter, DefaultDict, Deque, OrderedDict, OrderedSet, PriorityQueue, SortedDict, SortedSet
```

The module is an ordinary Incan stdlib source module. It is imported explicitly, has no feature gate, and does not use Rust-backed stdlib dispatch.

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

Builtin collections remain the default for ordinary data. Reach for `std.collections` when the named behavior matters: front-of-queue work, counted membership, missing-key defaulting, insertion-order stability, sorted traversal, layered configuration, or priority scheduling.

For task-oriented guidance, see [Choosing collection types](../../how-to/choosing_collections.md).

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

```incan
from std.collections import Deque

mut queue = Deque[str]()
queue.append("normal")
queue.appendleft("urgent")

next = queue.popleft()
```

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

```incan
from std.collections import Counter

counts = Counter.from_iter(["apple", "banana", "apple"])
assert counts.get("apple") == 2
```

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

```incan
from std.collections import ChainMap

defaults = OrderedDict.from_items([("region", "eu-west-1"), ("retries", "3")])
override = OrderedDict.from_items([("region", "us-east-1")])
cfg = ChainMap.from_layers([override, defaults])

assert cfg["region"] == "us-east-1"
assert cfg["retries"] == "3"
```

```incan
model Defaults:
    region: str
    retries: str

cfg = ChainMap.from_field_layers([Defaults(region="eu-west-1", retries="3").__field_items__()])
cfg.push_layer(OrderedDict.from_items([("region", "us-east-1")]))

assert cfg["region"] == "us-east-1"
assert cfg["retries"] == "3"
```

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
