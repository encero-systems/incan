# RFC 030: `std.collections` — Extended Collection Types


- **Status:** Draft
- **Created:** 2026-03-06
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 022 (stdlib namespacing), RFC 023 (compilable stdlib), RFC 028 (operator overloading)
- **Target version:** v0.2

## Summary

Introduce a `std.collections` stdlib namespace providing extended collection types beyond the builtin `List`, `Dict`, `Set`, and `Tuple`. These are ordinary stdlib types under the RFC 022 / RFC 023 model: imported explicitly, defined in Incan-first terms, and backed by Rust implementations where that is the practical runtime strategy. They are not compiler primitives, not vocabulary registrations, and not raw Rust re-exports.

The initial scope of this RFC is intentionally narrow:

- `Deque[T]` for efficient double-ended queue semantics
- `Counter[T]` for multiset / counting semantics

Dict-shaped extensions such as default-valued maps and ordered maps are deliberately **not** standardized as separate types in this RFC. Those concerns fit better as a future redesign of `Dict` / `FrozenDict` configuration and sugar than as a parallel hierarchy of near-duplicate mapping types.

## Motivation

Incan's builtins cover the most common collection needs:

| Builtin            | Rust backing                    | Mutable?  |
|--------------------|---------------------------------|-----------|
| `List[T]`          | `Vec<T>`                        | Yes       |
| `Dict[K, V]`       | `HashMap<K, V>`                 | Yes       |
| `Set[T]`           | `HashSet<T>`                    | Yes       |
| `Tuple[A, B, ...]` | `(A, B, ...)`                   | Immutable |
| `FrozenList[T]`    | `Vec<T>` (immutable API)        | No        |
| `FrozenSet[T]`     | `HashSet<T>` (immutable API)    | No        |
| `FrozenDict[K, V]` | `HashMap<K, V>` (immutable API) | No        |

But many programs need specialized data structures that don't warrant being compiler builtins:

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

Python's `collections` module shows the general shape of what users want from a batteries-included language: specialized container types that are common enough to deserve stdlib support but niche enough that they should not all become builtins.

For this first RFC, `deque` and `Counter` are the clearest wins. `defaultdict` and `OrderedDict` are useful too, but in Incan they collide with a more fundamental design question: should map defaults and order be properties of `Dict` itself, rather than entirely separate types? This RFC keeps that question open by not hard-coding the wrong shape too early.

## Guide-level explanation (how users think about it)

### Importing collection types

The initial collection types in this RFC live under `std.collections`:

```incan
from std.collections import Counter, Deque
```

### Design principle: dual naming where it adds real value

Incan bridges the Python and Rust worlds. Where method naming conventions diverge sharply between the two, `std.collections` may offer both as aliases. `Deque` is the clearest case:

| Python convention     | Rust convention           | Both work in Incan                            |
|-----------------------|---------------------------|-----------------------------------------------|
| `deque.append(x)`     | `vec_deque.push_back(x)`  | `deque.append(x)` / `deque.push_back(x)`      |
| `deque.appendleft(x)` | `vec_deque.push_front(x)` | `deque.appendleft(x)` / `deque.push_front(x)` |
| `deque.pop()`         | `vec_deque.pop_back()`    | `deque.pop()` / `deque.pop_back()`            |
| `deque.popleft()`     | `vec_deque.pop_front()`   | `deque.popleft()` / `deque.pop_front()`       |

Aliases are true synonyms. Neither spelling is deprecated; the choice is stylistic. This RFC does **not** assume every future collection type needs Python/Rust dual naming. `Deque` gets it because the two ecosystems use noticeably different, equally common names for the same operations.

## `Deque[T]` — Double-ended queue

**Rust backing:** `VecDeque<T>` (`std::collections`)  
**Serde:** Serializes as a JSON array (same as `List[T]`)

Efficient `O(1)` push/pop from both ends. Offers both Python-style (`append`/`appendleft`/`pop`/`popleft`) and Rust-style (`push_back`/`push_front`/`pop_back`/`pop_front`) method names — they are aliases for the same operations:

```incan
from std.collections import Deque

tasks: Deque[str] = Deque()

# Python-style naming
tasks.append("low priority")     # same as tasks.push_back("low priority")
tasks.appendleft("urgent")       # same as tasks.push_front("urgent")

next_task = tasks.popleft()      # "urgent"  (same as tasks.pop_front())
last_task = tasks.pop()          # "low priority"  (same as tasks.pop_back())

# Rust-style naming works too
tasks.push_back("task A")
tasks.push_front("task B")

next_task = tasks.pop_front()    # "task B" (same as tasks.popleft())
last_task = tasks.pop_back()     # "task A" (same as tasks.pop())

print(len(tasks))                # 4
```

**Method surface**:

Methods with dual Python/Rust names are aliases for the same operation:

| Method        | Alias        | Signature                    | Description                     |
|---------------|--------------|------------------------------|---------------------------------|
| `append`      | `push_back`  | `(self, item: T)`            | Append to back                  |
| `appendleft`  | `push_front` | `(self, item: T)`            | Prepend to front                |
| `pop`         | `pop_back`   | `(self) -> Option[T]`        | Remove from back                |
| `popleft`     | `pop_front`  | `(self) -> Option[T]`        | Remove from front               |
| `extend`      | —            | `(self, items: Iterable[T])` | Append multiple items to back   |
| `extendleft`  | —            | `(self, items: Iterable[T])` | Prepend multiple items to front |
| `__len__`     | —            | `(self) -> int`              | Element count                   |
| `is_empty`    | —            | `(self) -> bool`             | Whether empty                   |
| `__iter__`    | —            | iteration support            | Front-to-back iteration         |
| `__getitem__` | —            | `(self, index: int) -> T`    | Index access                    |

## `Counter[T]` — Counting / multiset

**Rust backing:** Newtype over `HashMap<T, usize>` (`std`)  
**Serde:** Serializes as a JSON object (`{"apple": 3, "banana": 2}`)

Counts occurrences of elements:

```incan
from std.collections import Counter

words = ["apple", "banana", "apple", "cherry", "banana", "apple"]
counts = Counter.from_iter(words)

print(counts["apple"])           # 3
print(counts.most_common(2))     # [("apple", 3), ("banana", 2)]
```

**Method surface**:

> Note: `Iterable[T]` is a stdlib trait provided through the collection-related derives surface. It is the protocol a type satisfies when it supports `for x in collection` via `__iter__`.

| Method        | Signature                               | Description                          |
|---------------|-----------------------------------------|--------------------------------------|
| `from_iter`   | `(items: Iterable[T]) -> Counter[T]`    | Construct a counter from an iterable |
| `__getitem__` | `(self, key: T) -> int`                 | Get count (0 for missing)            |
| `most_common` | `(self, n: int) -> List[Tuple[T, int]]` | Top N elements                       |
| `total`       | `(self) -> int`                         | Sum of all counts                    |
| `elements`    | `(self) -> List[T]`                     | Flat list with repetitions           |
| `update`      | `(self, items: Iterable[T])`            | Add counts                           |
| `__iter__`    | iteration support                       | Iterate unique elements              |

## Dict-shaped extensions deliberately deferred

This RFC does **not** standardize `DefaultDict` or `OrderedDict` as separate first-class types.

That is an intentional design decision, not an omission. Both are fundamentally variants of "map behavior":

- missing-key behavior (`default_factory`-style semantics)
- iteration / serialization order behavior (`remember_order`-style semantics)

Those concerns fit more naturally into the future design space of `Dict` and `FrozenDict` than into a parallel family of near-duplicate mapping types. If Incan eventually wants sugar names like `DefaultDict` or `OrderedDict`, those should layer on top of the core map design rather than lock us into separate base abstractions too early.

## Reference-level explanation (precise rules)

### Namespace registration

`std.collections` is registered in `STDLIB_NAMESPACES` in `crates/incan_core/src/lang/stdlib.rs`:

```rust
StdlibNamespace {
    name: "collections",
    impl_mode: StdlibImplMode::IncanSource,
    feature: None,
    extra_crate_deps: &[],
    submodules: &[],
},
```

This RFC does not require extra third-party Rust dependencies. `Deque` can rely on `std::collections::VecDeque`, and `Counter` can rely on `HashMap`.

### Interaction with existing features

- **Builtins**: `std.collections` types are distinct from builtins. `List` is always available without import; `Deque` requires `from std.collections import Deque`.
- **FrozenList / FrozenDict / FrozenSet**: These remain compiler builtins (already registered in `CollectionTypeId`). They are not moved to `std.collections` — they represent immutable views of the builtin mutable types.
- **Generics**: All `std.collections` types are generic. Their type parameters follow the same rules as the existing builtin generic types (`List[T]`, `Dict[K, V]`, etc.).
- **Pattern matching**: `match` on collection types works via standard method dispatch (e.g., matching on `Deque` elements after popping).
- **Loops / iteration**: All collection types support `for x in collection` via `__iter__`.
- **Dict evolution**: Ordered-map behavior and default-valued map behavior are intentionally left with `Dict` / `FrozenDict` design space rather than being frozen here as separate peer types.

### Compatibility / migration

Non-breaking. This is purely additive — new types in a new namespace.

## Alternatives considered

### Make all collection types builtins

Add `Deque`, `Counter`, etc. as compiler-known types like `List` and `Dict`. Rejected because these are specialized types that most programs don't need — polluting the global namespace with them would be un-Pythonic and inconsistent with the stdlib namespace model established by RFC 022.

### Use Rust crate re-exports directly

Let users write `import rust::std::collections::VecDeque as Deque` via RFC 005 Rust interop. Rejected because it exposes Rust naming conventions, requires users to know Rust types, and doesn't provide Python-familiar APIs (e.g., `Counter` doesn't exist in Rust's stdlib).

### Standardize `DefaultDict` / `OrderedDict` as separate types now

Rejected for this RFC. Both features are really about the behavior of maps, not about brand-new collection concepts. It is cleaner to settle the long-term design of `Dict` / `FrozenDict` first and only then decide whether sugar names like `DefaultDict` or `OrderedDict` are still warranted.

### Defer to third-party libraries

Wait for the Incan library system and let community libraries provide these types. Rejected because these are fundamental data structures that every language stdlib should provide, and they're needed before the library ecosystem exists.

## Drawbacks

- **Stdlib surface growth**: Each new type adds API surface to maintain and document.
- **Scope restraint**: By deferring ordered/default-valued map design, this RFC does not immediately cover every Python `collections` favorite. That is deliberate, but some users will still ask for those features next.
- **Overlap with builtins**: Some users may still ask when to use `List` vs `Deque`, or a plain `Dict[str, int]` vs `Counter[str]`. Clear docs and examples matter.

If a future ordered-map design still wants a distinct runtime type, using `indexmap` would be a reasonable option: it is a mature, widely used ordered-map crate in Rust. This RFC avoids taking that dependency only because it avoids standardizing `OrderedDict` at this stage.

## Layers affected

- **Stdlib registry** (`crates/incan_core/src/lang/stdlib.rs`) — `std.collections` must be registered as a new `StdlibNamespace` with `IncanSource` impl mode and no extra crate dependencies.
- **Stdlib source** (`crates/incan_stdlib/stdlib/`) — Incan-first type declarations and doc stubs for `Deque[T]` and `Counter[T]`, consistent with how existing stdlib modules are authored.
- **Stdlib runtime** (`crates/incan_stdlib/src/`) — Rust backing implementations for `Deque` (over `VecDeque<T>`) and `Counter` (newtype over `HashMap<T, usize>`).
- **Compiler frontend/backend** — No special parser, typechecker, or codegen handling is required beyond what the standard stdlib type-binding path already provides. These are ordinary stdlib types.
- **Tooling** (LSP, formatter) — No new syntax or special cases needed; stdlib loading and completion machinery already handles new namespaces.

## Design decisions and deferrals

1. **`ChainMap`**: Deferred indefinitely. The main layered-lookup use case is better served by the planned `ctx` direction than by adding another map type now.

2. **No frozen deque in this RFC**: `FrozenDeque` is not justified. For ordered-map immutability, the more important future question is whether `FrozenDict` itself should eventually grow ordered behavior.

3. **`Counter` arithmetic**: Supported in principle. Python's `Counter` arithmetic is a good fit for RFC 028-style operator overloading and should be designed on top of that operator protocol rather than as ad hoc special methods.

4. **No callable-factory `DefaultDict` design in this RFC**: If Incan adds default-valued map behavior later, the cleaner direction is explicit/default-based map semantics rather than Python's "any callable factory" model.

5. **`NamedTuple`**: Deferred. Incan `model` already covers the main named-field use case. If tuple-style positional indexing on models is desired, that should be a separate feature discussion.

6. **Additional collection types**: Future `std.collections` expansion should be driven by real demand and by whether a proposed type is genuinely distinct from existing builtins. Based on the Python/Rust ecosystems, `deque` and `Counter` are strong first-wave candidates; map variants deserve a separate design pass rather than being bundled in here by momentum.
