# RFC 030: `std.collections` — extended collection types


- **Status:** Draft
- **Created:** 2026-03-06
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 022 (stdlib namespacing), RFC 023 (compilable stdlib), RFC 028 (operator overloading)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/316
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

Introduce `std.collections` as Incan's standard library namespace for non-builtin container types that are common, user-facing, and semantically distinct from `List`, `Dict`, `Set`, and `Tuple`. The north-star module includes queue, multiset, ordered-map, ordered-set, sorted-map, sorted-set, default-valued map, layered-map, and priority-queue surfaces: `Deque[T]`, `Counter[T]`, `DefaultDict[K, V]`, `OrderedDict[K, V]`, `OrderedSet[T]`, `SortedDict[K, V]`, `SortedSet[T]`, `ChainMap[K, V]`, and `PriorityQueue[T]`. These are ordinary stdlib types under the RFC 022 / RFC 023 model: imported explicitly, specified in Incan-facing terms, and backed by Rust implementations where that is the practical runtime strategy.

This RFC is not a proposal to turn every interesting Rust or Python container into a builtin or direct re-export. It is a commitment that Incan should provide a coherent user-facing collections module with first-class, batteries-included types for the collection shapes that repeatedly matter in real application, analytics, and tooling code.

## Core model

`std.collections` sits above the builtin collection floor:

- builtins remain the default, always-available containers for general-purpose code
- `std.collections` provides opt-in specialized containers with distinct semantics
- these types are library types, not parser keywords or compiler primitives
- the public contract is Pythonic at the surface where that improves DX, while still taking advantage of Rust-backed implementations where they are clearly stronger

The intended north-star module surface is:

- `Deque[T]`: efficient double-ended queue
- `Counter[T]`: multiset / counted occurrences
- `DefaultDict[K, V]`: mapping with default-value behavior on missing-key access
- `OrderedDict[K, V]`: insertion-ordered mapping
- `OrderedSet[T]`: insertion-ordered set
- `SortedDict[K, V]`: key-sorted mapping with deterministic order and range-oriented behavior
- `SortedSet[T]`: value-sorted set with deterministic order and range-oriented behavior
- `ChainMap[K, V]`: layered lookup across multiple maps and record-like layers
- `PriorityQueue[T]`: heap-backed priority queue

## Motivation

Incan's builtins cover the common floor:

| Builtin            | Rust backing                    | Mutable?  |
|--------------------|---------------------------------|-----------|
| `List[T]`          | `Vec<T>`                        | Yes       |
| `Dict[K, V]`       | `HashMap<K, V>`                 | Yes       |
| `Set[T]`           | `HashSet<T>`                    | Yes       |
| `Tuple[A, B, ...]` | `(A, B, ...)`                   | Immutable |
| `FrozenList[T]`    | `Vec<T>` (immutable API)        | No        |
| `FrozenSet[T]`     | `HashSet<T>` (immutable API)    | No        |
| `FrozenDict[K, V]` | `HashMap<K, V>` (immutable API) | No        |

But many real programs need specialized collection semantics that are still too common to leave to ad hoc userland wrappers:

```incan
# Counting occurrences — today requires manual Dict bookkeeping
word_counts: Dict[str, int] = {}
for word in words:
    if word in word_counts:
        word_counts[word] += 1
    else:
        word_counts[word] = 1

# With std.collections:
from std.collections import Counter
word_counts = Counter.from_iter(words)   # Done.
```

```incan
# Queue with efficient push/pop from both ends
from std.collections import Deque

queue: Deque[str] = Deque()
queue.push_back("first")
queue.push_front("urgent")
item = queue.pop_front()   # "urgent"
```

```incan
# Layered config / overlay lookup
from std.collections import ChainMap

defaults = {"region": "eu-west-1", "retries": 3}
override = {"region": "us-east-1"}
cfg = ChainMap(override, defaults)

print(cfg["region"])   # "us-east-1"
print(cfg["retries"])  # 3
```

```incan
# Deterministic sorted keys with range-oriented traversal
from std.collections import SortedDict

prices = SortedDict({
    "apple": 2.40,
    "banana": 1.80,
    "pear": 2.10,
})

for key, value in prices.items():
    print(key, value)
```

Python's `collections` module proves the user demand, while Rust's `std::collections` and adjacent ecosystem give us stronger backing choices for ordered, sorted, and queue-like structures. Incan should not mirror either language blindly. It should standardize the collection types that actually make the language feel complete for systems, data, and library work.

## Goals

- Provide a coherent north-star `std.collections` module rather than a narrow two-type sketch.
- Keep builtins and specialized containers clearly separated.
- Make Pythonic surfaces first-class where that improves usability.
- Use Rust-backed implementations where they materially improve correctness, determinism, or performance.
- Include ordered and sorted collection families explicitly rather than forcing everything through future `Dict` redesign speculation.
- Support layered lookup as a general collection utility, including model-heavy Incan workflows.

## Non-Goals

- Making these collection types compiler builtins.
- Re-exporting Rust container APIs wholesale.
- Freezing every method and convenience helper down to the last alias in this draft.
- Solving the entire future `Dict` / `FrozenDict` redesign space here.
- Settling every comparison-policy detail for `PriorityQueue[T]` in this draft.

## Guide-level explanation (how users think about it)

### Importing collection types

Collection types in this RFC live under `std.collections`:

```incan
from std.collections import Counter, Deque, OrderedDict, SortedSet
```

### Design principle: specialized collection semantics, not near-duplicate builtins

Users should reach for `std.collections` when the semantics are the point:

- use `Deque` when both ends matter
- use `Counter` when counts are the data model
- use `DefaultDict` when missing-key defaulting is intentional
- use `OrderedDict` / `OrderedSet` when insertion order is meaningful and stable
- use `SortedDict` / `SortedSet` when sorted order and range traversal matter
- use `ChainMap` when layered lookup is the model
- use `PriorityQueue` when heap semantics are the model

The builtin collections remain the right default when none of those semantics matter.

### `Deque[T]`

Incan bridges the Python and Rust worlds. Where method naming conventions diverge sharply between the two, `std.collections` may offer both as aliases. `Deque` is the clearest case:

| Python convention     | Rust convention           | Both work in Incan                            |
|-----------------------|---------------------------|-----------------------------------------------|
| `deque.append(x)`     | `push_back(x)`            | `deque.append(x)` / `deque.push_back(x)`      |
| `deque.appendleft(x)` | `push_front(x)`           | `deque.appendleft(x)` / `deque.push_front(x)` |
| `deque.pop()`         | `pop_back()`              | `deque.pop()` / `deque.pop_back()`            |
| `deque.popleft()`     | `pop_front()`             | `deque.popleft()` / `deque.pop_front()`       |

Aliases are true synonyms. Neither spelling is deprecated. This RFC does not assume every collection type needs dual Python/Rust naming; `Deque` gets it because both ecosystems use sharply different, equally common names for the same operations.

```incan
from std.collections import Deque

tasks: Deque[str] = Deque()
tasks.append("low priority")
tasks.appendleft("urgent")

next_task = tasks.popleft()
```

### `Counter[T]`

`Counter` is the collection you use when multiplicity matters:

```incan
from std.collections import Counter

counts = Counter.from_iter(["apple", "banana", "apple"])
print(counts["apple"])        # 2
print(counts.most_common(1))  # [("apple", 2)]
```

### `DefaultDict[K, V]`

`DefaultDict` is a first-class type in Incan, not a postponed `Dict` redesign idea. It exists because missing-key default behavior is semantically meaningful and common enough to deserve its own name:

```incan
from std.collections import DefaultDict

groups = DefaultDict[List[str]](...)
groups["a"].append("x")
```

The exact constructor/default-factory contract is still open in this draft, but the type itself is not.

### `OrderedDict[K, V]` and `OrderedSet[T]`

These preserve insertion order as part of the contract:

```incan
from std.collections import OrderedDict

headers = OrderedDict()
headers["x-request-id"] = "abc"
headers["content-type"] = "application/json"
```

Stable insertion order matters for display, deterministic serialization, protocol-shaped data, and user-facing tooling. Incan should expose that explicitly instead of pretending ordinary hash maps are enough for every case.

### `SortedDict[K, V]` and `SortedSet[T]`

These are key-sorted / value-sorted collections with deterministic ordering and range-friendly behavior:

```incan
from std.collections import SortedSet

ids = SortedSet([5, 2, 9, 2])
for id in ids:
    print(id)   # 2, 5, 9
```

This is one place where Rust gives Incan a better north-star than Python's stdlib alone. Sorted collections are common enough in analytics and deterministic processing to deserve first-class support.

### `ChainMap[K, V]`

`ChainMap` is the general layered-lookup collection:

```incan
from std.collections import ChainMap

cfg = ChainMap({"region": "us-east-1"}, {"region": "eu-west-1", "retries": 3})
print(cfg["region"])   # "us-east-1"
print(cfg["retries"])  # 3
```

In Incan, `ChainMap` must also work sensibly with model-heavy code. A model layer participates through a field-overlay view:

```incan
model Defaults:
    region: str = "eu-west-1"
    retries: int = 3

cfg = ChainMap({"region": "us-east-1"}, Defaults())
```

That does not mean models become dicts. It means `ChainMap` supports both mapping layers and record-like layers intentionally.

### `PriorityQueue[T]`

`PriorityQueue` belongs in the module scope. Heap semantics are distinct enough from `List`/`Deque` to justify a first-class type. The remaining open design work is the exact ordering contract, not whether the type belongs here.

## Reference-level explanation (precise rules)

### Namespace registration

`std.collections` must remain an ordinary stdlib namespace under the RFC 022 / RFC 023 model. It is not a compiler keyword surface, and its types should be imported explicitly rather than treated as global builtins.

### Public type set

The north-star public surface standardized by this RFC is:

- `Deque[T]`
- `Counter[T]`
- `DefaultDict[K, V]`
- `OrderedDict[K, V]`
- `OrderedSet[T]`
- `SortedDict[K, V]`
- `SortedSet[T]`
- `ChainMap[K, V]`
- `PriorityQueue[T]`

Additional collection types may be added later, but the module should already read as a complete, deliberate contract rather than a two-type placeholder.

### Interaction with existing features

- **Builtins**: `std.collections` types are distinct from builtins. `List`/`Dict`/`Set` remain the always-available default containers.
- **Frozen builtins**: `FrozenList`, `FrozenDict`, and `FrozenSet` remain builtin/foundation surfaces. This RFC does not relocate them.
- **Generics**: All `std.collections` types are generic where appropriate and follow the normal builtin generic rules.
- **Iteration**: All collection types participate in ordinary `for`-loop iteration through standard collection protocols.
- **Serialization**: Ordered and sorted collections must preserve their defined order in any order-sensitive serialization or display surface; unordered collections need not.
- **Models and records**: `ChainMap` may accept record/model layers via a field-overlay view. Those layers are read-only in `ChainMap` unless a separate mutable-record contract is standardized later.

### Semantics by type

#### `Deque[T]`

- double-ended queue semantics are the point of the type
- both Python-style and Rust-style end-operation names are true aliases
- iteration order is front-to-back
- indexed access is allowed, but random access is not the motivation for the type
- Rust backing: `VecDeque<T>`

#### `Counter[T]`

- `Counter[T]` models counted membership rather than plain set membership
- missing keys read as zero
- the type supports `update`, `subtract`, `most_common`, `total`, and element expansion helpers
- arithmetic-style combination belongs naturally on the type
- the exact count-sign contract remains open in this draft

#### `DefaultDict[K, V]`

- missing-key access materializes and stores a default value according to the collection's configured defaulting rule
- `DefaultDict` is a distinct public type, not merely documentation sugar for `Dict`
- ordinary `Dict` remains non-defaulting

#### `OrderedDict[K, V]` and `OrderedSet[T]`

- insertion order is preserved by iteration and order-sensitive serialization
- reinserting an existing key/value does not create a duplicate entry
- Rust backing: `IndexMap` / `IndexSet` or equivalent ordered hash collections

#### `SortedDict[K, V]` and `SortedSet[T]`

- iteration order is sorted order
- the types support order-aware traversal and range-oriented operations
- keys/values must satisfy the language's ordering requirements for sorted collections
- Rust backing: `BTreeMap` / `BTreeSet`

#### `ChainMap[K, V]`

- lookup walks layers from first to last; earlier layers override later layers
- writes go to the first writable mapping layer by default
- mapping layers are ordinary map-like collections
- model/record layers participate through field names
- model/record layers are read-only in this RFC
- nested models are not flattened automatically
- this is a general collection utility; `ctx` may use a `ChainMap`-like overlay internally, but `ChainMap` is not defined in terms of `ctx`

#### `PriorityQueue[T]`

- heap semantics are the point of the type
- the underlying backing is heap-based (`BinaryHeap` or equivalent)
- the exact public ordering contract remains open in this draft

### Compatibility / migration

This RFC is additive. It introduces new opt-in stdlib types under a new namespace and does not change the meaning of builtin collection types.

## Design details

### Python and Rust influence

The module should be designed from Incan's point of view, not as a copy of either source ecosystem:

- Python gives the strongest user-facing intuition for `Deque`, `Counter`, `DefaultDict`, `OrderedDict`, and `ChainMap`
- Rust gives stronger backing choices and vocabulary for `VecDeque`, `BTreeMap`, `BTreeSet`, `BinaryHeap`, and the `IndexMap`/`IndexSet` family
- Incan should standardize the public semantics that make sense, then back them with Rust implementations where that is the cleanest runtime strategy

### Why separate map and set types still make sense

This RFC does not accept the idea that ordered/default/sorted map behavior must wait for a future monolithic `Dict` redesign. Those collection semantics are important enough, and common enough, that distinct first-class stdlib types are justified now. They are not near-duplicate noise; they are honest semantic distinctions:

- ordinary hash map semantics
- defaulting map semantics
- insertion-ordered map semantics
- sorted map semantics

Trying to force all of that into one future `Dict` redesign would leave the current collections story underpowered for no real gain.

### Why `ChainMap` belongs here

`ChainMap` is not just "proto-ctx". It is a general collection for layered lookup. But RFC 033 is still relevant: the precedence intuition should align. Earlier layers override later layers. The cleaner dependency direction is that `ctx` may use a `ChainMap`-like overlay internally, not that `ChainMap` is defined in terms of `ctx`.

## Alternatives considered

### Keep the RFC intentionally narrow

Keep `std.collections` limited to `Deque` and `Counter`. Rejected because it reads like a cautious implementation sketch rather than a credible north-star collections module.

### Force ordered/default behavior into future `Dict` redesign only

Rejected because it postpones clearly useful collection semantics behind a more abstract future design question. Distinct public types are justified here.

### Make all collection types builtins

Rejected because these are specialized containers, not global-language defaults.

### Re-export Rust collections directly

Rejected because that would leak Rust names and backend details into the public contract instead of giving Incan a deliberate collections story.

### Leave all specialized collections to third-party libraries

Rejected because this is basic language completeness territory, not an exotic ecosystem extension.

## Drawbacks

- Stdlib surface area grows materially.
- Some collection families overlap conceptually with builtins, so documentation quality matters.
- `ChainMap` becomes more subtle once model/record layers are supported explicitly.
- `PriorityQueue[T]` is in scope before its ordering contract is fully settled, so the draft must stay honest about that unresolved point.

## Layers affected

- **Stdlib registry** — `std.collections` remains a registered stdlib namespace.
- **Stdlib source** — public Incan-facing declarations, docs, examples, and protocol integration for the full type set.
- **Stdlib runtime** — Rust-backed implementations over `VecDeque`, `HashMap`, `IndexMap`, `IndexSet`, `BTreeMap`, `BTreeSet`, `BinaryHeap`, and equivalent runtime structures where appropriate.
- **Typechecker / protocol surface** — generic collection typing, iteration behavior, ordering constraints for sorted collections, and record-layer participation in `ChainMap`.
- **Serialization / docs / tooling** — deterministic-order behavior for ordered and sorted types must be documented and surfaced consistently in docs, completions, and examples.

## Design Decisions

1. `std.collections` is a full north-star module, not a narrow `Deque + Counter` placeholder.
2. `DefaultDict`, `OrderedDict`, and `OrderedSet` are first-class public types in this RFC.
3. `SortedDict` and `SortedSet` belong in the stdlib surface and should be backed by sorted-tree structures.
4. `ChainMap` remains in scope and should align with RFC 033's precedence intuition without being defined in terms of `ctx`.
5. `ChainMap` supports both mapping layers and record/model layers; record/model layers are read-only field overlays in this RFC.
6. `PriorityQueue[T]` remains in scope even though one core policy question is still unresolved.
7. `NamedTuple` is out of scope; Incan `model` already covers the named-record use case better.
8. `FrozenDeque` is out of scope for now; this RFC is about specialized mutable/runtime collection semantics first.

## Unresolved questions

1. What is the public ordering contract for `PriorityQueue[T]`: max-first only, min-first only, or a construction-time policy?
2. Should `Counter[T]` counts be fully signed, or should the core contract remain non-negative except where arithmetic helpers explicitly produce negatives?
3. What is the best first-class construction contract for `DefaultDict[K, V]`: default value, default factory, or both?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
