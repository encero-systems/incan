# RFC 177: Comprehensions

- **Status:** Planned
- **Created:** 2026-11-10
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 006 (generators and generator expressions)
    - RFC 009 (sized integers and widening)
    - RFC 088 (iterator adapters)
    - RFC 129 (shared or copied bindings)
    - RFC 164 (explicit ownership and borrowing)
- **Issue:** [#2975](https://github.com/encero-systems/incan/issues/2975)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

A comprehension builds a list, a set or a dict from one or more `for` clauses and any number of `if` filters, eagerly, in one expression. Its clauses are the same as a generator expression's (RFC 006), which is the lazy form. The bindings a comprehension introduces are local to it. Its element type comes from the element expression and, when known, the expected type, and `?` inside it propagates out of the enclosing function as it would from a loop body.

## Motivation

List and dict comprehensions have shipped since early releases, and 0.3 made `?` work inside them, but no RFC states their rules. The language reference mentions them in passing: that their loop variable is local, and that a dict comprehension's key must be hashable. It does not say which clauses they accept, in what order the clauses run, or how the element is typed.

Their clauses also drifted from generator expressions. A generator expression accepts any sequence of `for` and `if` clauses, while list and dict comprehensions accept one `for` and at most one `if`, and there is no set comprehension. One clause grammar should serve all four forms.

## Guide-level explanation

```incan
squares = [n * n for n in range(10)]
evens = [n for n in numbers if n % 2 == 0]
by_id = {user.id: user for user in users}
tags = {tag for post in posts for tag in post.tags}
pairs = [(row, col) for row in range(3) for col in range(3) if row != col]
```

Clauses read left to right, like nested loops. In `pairs`, `row` is bound before `col`, and the filter sees both. None of `n`, `user`, `post`, `tag`, `row` or `col` exists after its comprehension.

A comprehension is eager: it builds its whole result before the next statement runs. When the next step consumes the items once, in order, a generator expression (`(n * n for n in range(10))`) avoids building the collection.

## Reference-level explanation

### Forms

| Form | Result |
| --- | --- |
| `[e clauses]` | `list[T]`, where `T` is the type of `e` |
| `{e clauses}` | `set[T]` |
| `{k: v clauses}` | `dict[K, V]` |
| `(e clauses)` | A generator expression, as RFC 006 specifies |

### Clauses

- The clauses are one `for p in source`, followed by any sequence of `for p in source` and `if condition`.
- They run as nested loops in written order. Each `for` iterates its source for every binding the clauses before it produce, and each `if` skips a binding when its condition is false.
- The element expression is evaluated once for each binding that every clause accepts, and the results keep that order in a list.
- A source is anything a `for` loop accepts, including a range, a collection, an `Iterator[T]` and a generator. The first source is evaluated once, before iteration starts. A later source is evaluated once for each binding of the clauses before it.
- `p` is any binding pattern a `for` loop accepts, such as `(key, value)`.

### Scope

- A clause's bindings are visible in the clauses after it and in the element expression, and nowhere else. They shadow outer names inside the comprehension and do not exist after it.
- The comprehension reads outer names and its sources the way a `for` loop does: under RFC 129 and RFC 164, it takes, borrows or copies items as a loop over the same source would.

### Types

- The element type is the type of the element expression. When the comprehension's expected type is known, as in `wide: list[int] = [b for b in octets]` over a `list[u8]`, each element is checked against the expected element type and may widen losslessly, as a single value does under RFC 009.
- A set element and a dict key must implement `Eq` and `Hash` (`INCAN-T0114`), as for literals.
- In a dict comprehension, a later entry with a key equal to an earlier one replaces it.
- The result is a new collection that the binding owns, so it may be bound with `mut` and changed.

### Errors and control flow

- `?` in an element expression, a filter or a source propagates out of the enclosing function, as it does from a loop body, and the comprehension's result is not produced.
- A comprehension holds expressions only, so no statement, such as `break`, `continue`, `return` or `yield`, can appear in it.
- A comprehension is not const-evaluable, so a `const` initializer cannot contain one.

### Compatibility

- Every list and dict comprehension accepted today keeps its meaning.
- Set comprehensions, more than one `for` clause in a list, set or dict comprehension, and more than one `if` are new.

## Alternatives considered

- **Keep one clause in eager comprehensions.** Generator expressions would keep a grammar the eager forms lack, and a nested build would need a loop or a lazy detour.
- **No set comprehension.** `{x for x in xs}` reads as one, and leaving it out forces `set([...])` around a list.
- **Leaking the loop variable, as Python 2 did.** A binding visible after the comprehension invites reading a value nobody meant to keep.

## Drawbacks

- Deeply nested clauses can read worse than the loops they replace.
- A comprehension builds its whole result, which costs memory a generator expression would not.

## Layers affected

- **Parser:** list, set and dict comprehensions accept the full clause sequence, and set comprehensions parse.
- **Typechecker:** clause scoping, element typing against the expected type, and hashability of set elements and dict keys.
- **Lowering / IR emission:** clauses lower to nested iteration with filters, and `?` exits the enclosing function.
- **Stdlib:** none.
- **CLI / tooling:** none.
- **LSP / formatter:** completion sees clause bindings in later clauses and the element. The formatter lays out multi-clause comprehensions.

## Inspectability and tooling surface

- **Artifact or metadata:** none.
- **Inspection command:** `incan check`.
- **Diagnostics:** an unhashable set element or dict key (`INCAN-T0114`) and an element that does not fit the expected type, each pointing at the expression.
- **Provenance:** each clause's span.
- **Not implicit:** a comprehension never becomes lazy, and a binding never leaks out of it.

## Design Decisions

- **One clause grammar for all four forms.** The lazy and eager forms differ only in when items are produced.
- **Clauses run as nested loops in written order.** A reader can rewrite any comprehension as loops without reordering anything.
- **Bindings are local.** A comprehension is an expression, so what it binds ends with it.
- **The expected type types the elements.** A comprehension behaves like the list, set or dict literal it replaces.
