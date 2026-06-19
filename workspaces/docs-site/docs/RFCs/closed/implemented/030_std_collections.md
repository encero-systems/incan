# RFC 030: `std.collections` — extended collection types


- **Status:** Implemented
- **Created:** 2026-03-06
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 022 (stdlib namespacing), RFC 023 (compilable stdlib), RFC 028 (operator overloading)
- **Issue:** [#164](https://github.com/encero-systems/incan/issues/164)
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** v0.3

## Summary

Introduce `std.collections` as Incan's standard library namespace for non-builtin container types that are common, user-facing, and semantically distinct from `List`, `Dict`, `Set`, and `Tuple`. The north-star module includes queue, multiset, ordered-map, ordered-set, sorted-map, sorted-set, default-valued map, layered-map, and priority-queue surfaces: `Deque[T]`, `Counter[T]`, `DefaultDict[K, V]`, `OrderedDict[K, V]`, `OrderedSet[T]`, `SortedDict[K, V]`, `SortedSet[T]`, `ChainMap[K, V]`, and `PriorityQueue[T]`. These are ordinary stdlib types under the RFC 022 / RFC 023 model: imported explicitly, specified in Incan-facing terms, and implemented as Incan-source stdlib code rather than Rust-backed stdlib dispatch.

This RFC is not a proposal to turn every interesting Rust or Python container into a builtin or direct re-export. It is a commitment that Incan should provide a coherent user-facing collections module with first-class, batteries-included types for the collection shapes that repeatedly matter in real application, analytics, and tooling code.

## Core model

`std.collections` sits above the builtin collection floor:

- builtins remain the default, always-available containers for general-purpose code
- `std.collections` provides opt-in specialized containers with distinct semantics
- these types are library types, not parser keywords or compiler primitives
- the public contract is Pythonic at the surface where that improves DX, while the implementation dogfoods ordinary Incan-source stdlib code

The intended north-star module surface is:

- `Deque[T]`: efficient double-ended queue
- `Counter[T]`: multiset / counted occurrences
- `DefaultDict[K, V]`: mapping with default-value behavior on missing-key access
- `OrderedDict[K, V]`: insertion-ordered mapping
- `OrderedSet[T]`: insertion-ordered set
- `SortedDict[K, V]`: key-sorted mapping with deterministic order and range-oriented behavior
- `SortedSet[T]`: value-sorted set with deterministic order and range-oriented behavior
- `ChainMap[K, V]`: layered lookup across multiple maps and record-like layers
- `PriorityQueue[T]`: priority queue with explicit ordering policy

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

Python's `collections` module proves the user demand, while Rust's collection vocabulary remains useful prior art for semantics and performance expectations. Incan should not mirror either language blindly. It should standardize the collection types that actually make the language feel complete for systems, data, and library work, and the stdlib implementation should remain pure Incan unless a future RFC explicitly creates a host-backed escape hatch.

## Goals

- Provide a coherent north-star `std.collections` module rather than a narrow two-type sketch.
- Keep builtins and specialized containers clearly separated.
- Make Pythonic surfaces first-class where that improves usability.
- Dogfood pure Incan stdlib implementations instead of adding Rust-backed dispatch for this module.
- Include ordered and sorted collection families explicitly rather than forcing everything through future `Dict` redesign speculation.
- Support layered lookup as a general collection utility, including model-heavy Incan workflows.

## Non-Goals

- Making these collection types compiler builtins.
- Re-exporting Rust container APIs wholesale.
- Adding Rust-backed stdlib dispatch, Rust runtime wrappers, or host-only collection methods for `std.collections`.
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

`DefaultDict` supports both a copied default value and a zero-argument default factory. Use the default-value constructor when every missing key should start from the same cloned value; use the default-factory constructor when each missing key should be materialized from a callable.

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
- implementation must be Incan-source stdlib code, not a Rust `VecDeque<T>` wrapper

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
- implementation must be Incan-source stdlib code, not `IndexMap` / `IndexSet` wrappers or equivalent Rust-backed dispatch

#### `SortedDict[K, V]` and `SortedSet[T]`

- iteration order is sorted order
- the types support order-aware traversal and range-oriented operations
- keys/values must satisfy the language's ordering requirements for sorted collections
- implementation must be Incan-source stdlib code, not `BTreeMap` / `BTreeSet` wrappers

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
- the implementation must provide priority-queue semantics in Incan-source stdlib code rather than wrapping `BinaryHeap` or equivalent Rust runtime state
- the exact public ordering contract remains open in this draft

### Compatibility / migration

This RFC is additive. It introduces new opt-in stdlib types under a new namespace and does not change the meaning of builtin collection types.

## Design details

### Python and Rust influence

The module should be designed from Incan's point of view, not as a copy of either source ecosystem:

- Python gives the strongest user-facing intuition for `Deque`, `Counter`, `DefaultDict`, `OrderedDict`, and `ChainMap`
- Rust gives useful vocabulary for queue, sorted-map, sorted-set, and priority-queue semantics, but those names are design references rather than implementation permission
- Incan should standardize the public semantics that make sense, then implement the module in pure Incan-source stdlib code so the language dogfoods its own collection abstractions

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

### Back `std.collections` with Rust runtime dispatch

Rejected because this module is intended to prove ordinary Incan-source stdlib expressiveness. Rust-backed dispatch would hide gaps in model methods, generics, indexing traits, iteration, and module loading that this RFC is supposed to exercise directly.

### Leave all specialized collections to third-party libraries

Rejected because this is basic language completeness territory, not an exotic ecosystem extension.

## Drawbacks

- Stdlib surface area grows materially.
- Some collection families overlap conceptually with builtins, so documentation quality matters.
- `ChainMap` becomes more subtle once model/record layers are supported explicitly.
- `PriorityQueue[T]` is in scope before its ordering contract is fully settled, so the draft must stay honest about that unresolved point.

## Layers affected

- **Stdlib registry** — `std.collections` remains a registered stdlib namespace.
- **Stdlib source** — public Incan-facing declarations, docs, examples, and pure Incan-source implementations for the full type set.
- **Stdlib runtime** — no new Rust-backed dispatch or Rust wrapper state for this module; existing builtin collection primitives may be used only through ordinary Incan syntax and stdlib source.
- **Typechecker / protocol surface** — generic collection typing, iteration behavior, ordering constraints for sorted collections, and record-layer participation in `ChainMap`.
- **Serialization / docs / tooling** — deterministic-order behavior for ordered and sorted types must be documented and surfaced consistently in docs, completions, and examples.

## Implementation Plan

### RFC lifecycle and source contract

- Keep the RFC, issue, and implementation aligned on a pure Incan-source stdlib contract.
- Register `std.collections` as an ordinary stdlib namespace without extra Rust crate dependencies.
- Ensure any implementation blocker caused by missing language support is reported as a compiler or stdlib dogfooding gap, not worked around through Rust-backed dispatch.

### Stdlib collection module

- Add `std.collections` source under `crates/incan_stdlib/stdlib/collections.incn`.
- Implement the full public type set in Incan source: `Deque[T]`, `Counter[T]`, `DefaultDict[K, V]`, `OrderedDict[K, V]`, `OrderedSet[T]`, `SortedDict[K, V]`, `SortedSet[T]`, `ChainMap[K, V]`, and `PriorityQueue[T]`.
- Provide both Python-style and Rust-style `Deque` end-operation aliases as true synonyms.
- Implement `Counter[T]` with non-negative core counts; arithmetic helpers may produce intermediate negative counts only where the method contract explicitly says so.
- Implement `DefaultDict[K, V]` with both default-value and default-factory construction paths.
- Implement `PriorityQueue[T]` with a construction-time ordering policy and a min-first default.

### Compiler and tooling integration

- Extend the stdlib namespace registry so `from std.collections import ...` resolves through the normal stdlib loading path.
- Verify exported types, generic annotations, method surfaces, and doc metadata are visible through existing stdlib import, typechecker, and LSP/documentation machinery.
- Do not add parser keywords, builtin type IDs, special lowering paths, or Rust runtime wrappers for the `std.collections` types.

### Tests and docs

- Add focused import/typechecker coverage for all public `std.collections` types.
- Add compile/run or snapshot coverage for representative behavior across queue, counter, defaulting, ordered, sorted, layered, and priority queue semantics.
- Update the authored stdlib reference/docs navigation for `std.collections`.
- Add release notes and bump the active development version.

## Implementation log

### Spec / design

- [x] Resolve the implementation backing contract as pure Incan-source stdlib code.
- [x] Resolve `PriorityQueue[T]` ordering as a construction-time policy with min-first default.
- [x] Resolve `Counter[T]` as non-negative core counts with explicitly contracted arithmetic exceptions.
- [x] Resolve `DefaultDict[K, V]` construction as both default-value and default-factory.

### Stdlib registry

- [x] Register `std.collections` in `STDLIB_NAMESPACES` without extra Rust crate dependencies.
- [x] Add namespace/path tests for `std.collections`.

### Stdlib source

- [x] Add `crates/incan_stdlib/stdlib/collections.incn`.
- [x] Implement `Deque[T]` with alias method pairs.
- [x] Implement `Counter[T]` with non-negative core operations and counting helpers.
- [x] Implement `DefaultDict[K, V]` with default value and default factory.
- [x] Implement `OrderedDict[K, V]` and `OrderedSet[T]` with insertion-order behavior.
- [x] Implement `SortedDict[K, V]` and `SortedSet[T]` with deterministic sorted behavior.
- [x] Implement `ChainMap[K, V]` for mapping layers.
- [x] Add `ChainMap[K, V]` record/model field overlays through compiler-generated field snapshots.
- [x] Implement `PriorityQueue[T]` with construction-time ordering policy and min-first default.

### Compiler / tooling

- [x] Verify imported public collection types are visible to the typechecker and library export metadata.
- [x] Verify no parser, builtin, lowering, emission, or Rust runtime special case is added for `std.collections`.
- [x] Verify docs/LSP-facing metadata exposes the new module.

### Tests

- [x] Add targeted typechecker/import tests for all public collection types.
- [x] Add behavior coverage for `Deque`, `Counter`, `DefaultDict`, ordered collections, sorted collections, `ChainMap`, and `PriorityQueue`.
- [x] Add a guard that `std.collections` has no extra Rust-backed crate dependency or runtime wrapper.
- [x] Run targeted verification.
- [x] Run the repository pre-commit gate.

### Docs / release

- [x] Update stdlib reference docs and navigation for `std.collections`.
- [x] Add release notes for RFC 030.
- [x] Bump the active development version.

## Design Decisions

1. `std.collections` is a full north-star module, not a narrow `Deque + Counter` placeholder.
2. `DefaultDict`, `OrderedDict`, and `OrderedSet` are first-class public types in this RFC.
3. `SortedDict` and `SortedSet` belong in the stdlib surface and must be implemented in pure Incan-source stdlib code rather than backed by Rust sorted-tree wrappers.
4. `ChainMap` remains in scope and should align with RFC 033's precedence intuition without being defined in terms of `ctx`.
5. `ChainMap` supports both mapping layers and record/model layers; record/model layers are read-only field overlays in this RFC.
6. `PriorityQueue[T]` uses a construction-time ordering policy and defaults to min-first behavior.
7. `NamedTuple` is out of scope; Incan `model` already covers the named-record use case better.
8. `FrozenDeque` is out of scope for now; this RFC is about specialized mutable/runtime collection semantics first.
9. `Counter[T]` has non-negative core counts; arithmetic helpers may expose negative intermediate counts only where explicitly documented.
10. `DefaultDict[K, V]` supports both default-value and default-factory construction.
