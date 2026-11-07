# RFC 110: Three-way comparison with `__cmp__`

- **Status:** Planned
- **Created:** 2026-06-06
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 024 (extensible derive protocol)
    - RFC 028 (trait-based operator overloading)
    - RFC 042 (traits are always abstract)
    - RFC 068 (protocol hooks for core language syntax)
    - RFC 083 (symbol aliases)
    - RFC 088 (iterator adapter surface)
    - RFC 127 (lint catalog)
    - RFC 136 (chained comparisons)
- **Issue:** [#2660](https://github.com/encero-systems/incan/issues/2660)
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

A type defines its total order in one place: `__cmp__(self, other) -> Ordering`. All six comparison operators come from that one hook, `Ord` remains the capability that a bound asks for, and `cmp(a, b)`, also spelled `compare(a, b)`, is how code asks the question directly. `@derive(Ord)` generates the hook. This replaces the pair of `__eq__` and `__lt__` methods that ordered types must keep in agreement today, with no new trait.

## Core model

1. **One hook.** `__cmp__` answers less, equal or greater, so `==` and `<` cannot disagree.
2. **`Ordering` is a type.** The answer is `Ordering.Less`, `Ordering.Equal` or `Ordering.Greater`, never an integer.
3. **`Ord` stays the capability.** Defining `__cmp__` makes the syntax work, and adopting `Ord` is what satisfies a bound, under RFC 068.
4. **Total and partial orders stay apart.** `__cmp__` means a total order. `__lt__` and its siblings remain the hooks for `PartialOrd`, where some pairs have no answer.
5. **`cmp` is library code.** It is an ordinary function in `std.derives.comparison`, imported explicitly.

## Motivation

Custom ordering today means two methods that have to agree:

```incan
model Task with Ord:
    priority: int
    name: str

    def __eq__(self, other: Task) -> bool:
        return self.priority == other.priority and self.name == other.name

    def __lt__(self, other: Task) -> bool:
        if self.priority != other.priority:
            return self.priority < other.priority
        return self.name < other.name
```

If they ever disagree, `sorted` and `==` quietly contradict each other, and nothing can detect it. A three-way comparison answers both questions in one place. There is also no way today to ask for the answer itself: code that needs less, equal or greater has to make two comparisons.

Python 2 had `__cmp__` and `cmp()`, and Python 3 removed them because `__cmp__` returned a bare integer and overlapped with the rich comparison hooks. Returning `Ordering`, and refusing the overlap, fixes both problems.

## Goals

- Define a total order in one method.
- Give every comparison operator one source for an ordered type.
- Let code ask for the three-way answer directly.
- Keep `@derive(Ord)` and `T with Ord` as they are.
- Keep partial orders explicit through `PartialOrd`.

## Non-Goals

- A new comparison trait beside `Eq` and `Ord`.
- Comparison across different types.
- Locale-sensitive string ordering, natural sort or collation.
- Treating an unrelated method named `compare` as part of comparison.

## Guide-level explanation

`Ordering`, `cmp` and `compare` are proposed by this RFC. `Ord` is in the prelude.

### Derive it

For most types, deriving is enough. Fields compare in declaration order:

```incan
@derive(Ord)
model Version:
    major: int
    minor: int
    patch: int

def supports(candidate: Version, minimum: Version) -> bool:
    return candidate >= minimum
```

`Version(major=1, minor=4, patch=0)` is greater than `Version(major=1, minor=3, patch=9)`, because `minor` breaks the tie after equal `major` values.

### Define it

For a custom order, write one method:

```incan
from std.derives.comparison import Ordering, cmp

model Task with Ord:
    priority: int
    name: str

    def __cmp__(self, other: Task) -> Ordering:
        return cmp(self.priority, other.priority).then(cmp(self.name, other.name))
```

`==`, `!=`, `<`, `<=`, `>` and `>=` all come from it, and `sorted`, `SortedDict` and `PriorityQueue` accept `Task` because it adopts `Ord`. Tuples compare element by element, so the same order can be one comparison:

```incan
def __cmp__(self, other: Task) -> Ordering:
    return cmp((self.priority, self.name), (other.priority, other.name))
```

For a descending field, swap the arguments or use `.reversed()`:

```incan
def __cmp__(self, other: Task) -> Ordering:
    return cmp(other.priority, self.priority).then(cmp(self.name, other.name))
```

Without `with Ord`, the operators still work, because a hook is enough for syntax under RFC 068. A bound such as `T with Ord` needs the adoption, and so does `cmp`.

`compare` is an alias of `cmp`, for code that reads better spelled out:

```incan
from std.derives.comparison import Ordering, compare

match compare(left, right):
    Ordering.Less => move_left()
    Ordering.Equal => stop()
    Ordering.Greater => move_right()
```

### Range checks and clamping

A range check is a chained comparison under RFC 136, with nothing new to learn. Inverted bounds are simply false:

```incan
if minimum <= version <= maximum:
    print("supported")
```

`clamp` limits a value to a range:

```incan
volume = requested.clamp(0, 100)
```

It raises the value to the low bound and then caps it at the high bound, so it cannot fail and needs no `?`. Inverted bounds return the high bound, and a lint flags inverted literals.

### Floats

`float` is not `Ord`, because NaN has no place in an ordinary comparison. `total_cmp` uses IEEE 754's total order, in which every value has a place, so a model with a float field can still define an order:

```incan
def __cmp__(self, other: Candidate) -> Ordering:
    return self.score.total_cmp(other.score).then(cmp(self.name, other.name))
```

## Reference-level explanation

### `Ordering`

1. `std.derives.comparison` must provide `enum Ordering` with `Less`, `Equal` and `Greater` in that order, adopting `Eq`, `Ord`, `Hash`, `Copy`, `Clone` and `Debug`.
2. `Ordering` must provide `reversed(self) -> Ordering`, and `then(self, other: Ordering) -> Ordering`, which returns `self` unless it is `Equal`, and then returns `other`. `other` is evaluated before the call.

### The hook

3. `__cmp__(self, other: Self) -> Ordering` must define `==`, `!=`, `<`, `<=`, `>` and `>=` on a type that defines it, under RFC 068's structural hook resolution. Evaluating one of those operators must call it once.
4. A type defining `__cmp__` must not also define `__eq__`, `__ne__`, `__lt__`, `__le__`, `__gt__` or `__ge__`, or derive `Eq`, `PartialEq`, `Ord` or `PartialOrd`. The diagnostic must name both.
5. A `__cmp__` with another signature must be refused with a diagnostic naming the required one. An ordinary method named `compare` must have no effect on comparison.
6. `__cmp__` must define a total order: reflexive, antisymmetric and transitive. The compiler does not prove this, and documentation and the derive must treat it as the contract.

### `Ord`

7. `Ord` must require `__cmp__` and imply `Eq`, with its comparison operators provided from the hook. A type adopting `Ord` and defining `__lt__` instead of `__cmp__` must be refused, with a diagnostic pointing at `__cmp__`.
8. `Ord` must provide `clamp(self, low: Self, high: Self) -> Self`, which returns `low` if `self` is below it, then `high` if the result is above it. It must not raise, and inverted bounds must return `high`.
9. `@derive(Ord)` must generate `__cmp__`, comparing fields in declaration order and returning the first result that is not `Equal`. For an enum, it must compare variants in declaration order and then payloads. A field or payload whose type is not `Ord` must be refused, naming it.
10. Ordered built-in types must provide `__cmp__`, with results agreeing with their operators. A tuple must be `Ord` when its element types are, comparing element by element.

### `PartialOrd`

11. `PartialOrd` must be unchanged: `__lt__`, `__le__`, `__gt__` and `__ge__` remain its hooks, and a type defining only `__lt__` keeps its operators. `float` and `f32` stay `PartialOrd` and not `Ord`.
12. `float` and `f32` must provide `total_cmp(self, other: Self) -> Ordering` using IEEE 754's totalOrder predicate.

### `cmp` and `compare`

13. `std.derives.comparison` must provide `pub def cmp[T with Ord](left: T, right: T) -> Ordering`, returning `left.__cmp__(right)`, and `pub compare = alias cmp` under RFC 083. Neither must be added to the prelude.

### Diagnostics and lints

14. RFC 127's catalog must include a lint for a `clamp` whose literal bounds are inverted.
15. Hover must show the generated `__cmp__` of a derive, and the operators a hook provides.

### Conformance workloads

| Workload | Required evidence |
| --- | --- |
| One source | Every operator on a type with `__cmp__` agrees with it, and calls it once. |
| Overlap | `__cmp__` with `__lt__`, `__eq__` or a comparison derive is refused. |
| Adoption | A hook without `with Ord` gives operators but fails a `T with Ord` bound, and `with Ord` satisfies it. |
| Derive | Fields compare in declaration order and enums by variant then payload, and a field that is not `Ord` is refused. |
| Collections | `sorted`, `SortedDict`, `SortedSet` and `PriorityQueue` order by `__cmp__`. |
| Clamp | Values below, inside and above the range, and inverted bounds, behave as specified. |
| Floats | `total_cmp` orders NaN, infinities and signed zeros by IEEE 754 totalOrder. |
| Partial orders | A type with only `__lt__` keeps its operators and does not satisfy `Ord`. |

### Compatibility

A type that adopts `Ord` by hand with `__eq__` and `__lt__` must move to `__cmp__`, and the diagnostic says how. `@derive(Ord)`, `T with Ord` and types with only `__lt__` are unaffected. In this repository that means a few datetime types in the standard library and some compiler fixtures.

## Design details

### Why no `Comparable`

A second trait would need rules for how it relates to `Ord`, what happens when they disagree, and how to avoid two ordering pathways, and every collection would have to choose one. `Comparable` is a friendlier word, but putting the method on `Ord` gives one name to one thing. The pattern then matches the rest of the language: a capability, a hook and a way to use it, as `Eq`, `__eq__` and `==` are.

### Why `cmp` is imported

An ordering is defined rarely, so the names are not worth a place in every module. `Ord` is already in the prelude, and an explicit import of `Ordering` and `cmp` keeps the prelude small and keeps their origin at the top of the file.

### Why `clamp` cannot fail

A fallible `clamp` would put `?` on a call whose only failure is a programming mistake, and a raising one would hide a panic in a small helper. Defining the result keeps it simple, and it is the same answer two nested comparisons would give. The lint catches the mistake where it can be seen.

### Prior art

Rust's `Ord::cmp` returns `Ordering`, with `then` and `reverse`, and its `clamp` panics on inverted bounds. Python 2's `__cmp__` returned an integer, and Python 3 removed it in favor of rich comparisons and `functools.total_ordering`. Ruby's `Comparable` builds its operators from `<=>`, which is the ergonomic idea here without the integer. Java and Kotlin use `Comparable` with an integer `compareTo`, and Swift's `Comparable` is built on `<`. C++ `operator<=>` is three-way with distinct strong and partial orderings, the same split as `Ord` and `PartialOrd`.

## Alternatives considered

- **A new `Comparable` trait:** see "Why no `Comparable`".
- **Renaming `Ord` to `Comparable`:** a friendlier word, at the cost of a rename across the language with no change in meaning.
- **A plain `compare` method as the hook:** every other operator hook in Incan is a dunder.
- **`__compare__`:** clearer on first read, but stands out among `__eq__` and `__lt__`, and Python readers know `__cmp__`.
- **Integer results:** admit invalid values and are what Python 3 removed.
- **`cmp` in the prelude:** a name every module gets for a rare need.
- **A `between` helper:** RFC 136's chained comparison already says it.
- **A fallible `clamp`:** see "Why `clamp` cannot fail".
- **`Comparable[T]` across types:** breaks the total-order laws, and converting first is clearer.
- **A `PartialComparable` trait:** `PartialOrd` already exists.

## Drawbacks

Hand-written `__lt__` on an `Ord` type stops compiling, which is a migration even though it is small. `Ord` is a terser word than `Comparable`, and newcomers from Java or Swift will look for the other. `then` evaluates its argument even when it is not needed, so an expensive tie-break needs an `if`. Structural comparison depends on field order, so reordering fields can change an order.

## Layers affected

- **Typechecker:** the hook, its overlap rules and operator resolution.
- **Lowering and emission:** operators from `__cmp__`, and the derive.
- **Standard library:** `Ordering`, `cmp`, `compare`, `Ord` and `clamp`, written in Incan.
- **Built-in types:** `__cmp__` on ordered types, tuples as `Ord`, and `total_cmp` on floats.
- **Lints:** inverted literal bounds on `clamp`.
- **Language server:** hover for generated hooks.
- **Documentation:** the comparison reference and derive tables, with migration notes.

## Design Decisions

- **One trait:** no `Comparable`. `Ord` is the capability, `__cmp__` is the hook, and `cmp(a, b)` or `compare(a, b)` is the direct question.
- **Hook rules:** defining `__cmp__` gives the six operators. Adopting `Ord` satisfies a bound. Mixing `__cmp__` with `__eq__`, `__lt__` or a comparison derive is refused.
- **Partial orders:** `__lt__` and `PartialOrd` stay as they are. Floats get `total_cmp` rather than a new trait.
- **`cmp`:** ordinary library code, imported explicitly from `std.derives.comparison`, with `compare` as an RFC 083 alias.
- **Same type only:** `__cmp__` takes `Self`.
- **Helpers:** no `between`, because RFC 136 covers it. `clamp` cannot fail, and inverted bounds return the high bound.
