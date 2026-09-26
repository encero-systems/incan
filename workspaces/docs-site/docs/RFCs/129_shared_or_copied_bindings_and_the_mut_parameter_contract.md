# RFC 129: Shared or copied bindings and the `mut` parameter contract

- **Status:** Draft
- **Created:** 2026-09-26
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 006 (Python-style generators)
    - RFC 016 (`loop` and `break <value>`)
    - RFC 023 (compilable stdlib; §6 implicit ownership and borrowing)
    - RFC 128 (one `Callable[(A, B) -> R]` trait; callable types carry the `mut` parameter marker)
    - #121 (implicit ownership inference)
    - #1609 and #1611 (duckborrowing across units, and liveness)
    - #1773 (`mut` parameter fixes, `INCAN-T0117`, and the transitional refusal)
    - #1790 (the `mut` marker in function types)
- **Issue:** [#1846](https://github.com/encero-systems/incan/issues/1846)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

This RFC specifies the `mut` parameter contract, which the language reference states but no RFC does, and decides what a new binding made from an existing collection, model, or class value means: whether `mut other = items` shares the value or copies it, for ordinary locals and `mut` parameters alike. Left to the generated code, the answer depends on how the binding is spelled, which breaks RFC 023's rule that ownership decisions never change what a program does. The RFC sets out the two coherent answers, Python-style views and value semantics, with the rules each needs, recommends value semantics with a rule that refuses any copy a Python reader would misread, and records the transitional rule, implemented by the #1773 fixes, under which neither answer changes the meaning of a program that compiles.

## Core model

1. **The `mut` parameter contract.** A `mut` parameter whose type is not `int`, `float`, or `bool` is *caller-visible*: the callee's in-place changes to it reach the caller, it cannot be rebound (`INCAN-T0001`), and an argument for it that the call changes or may change must be a place the caller may change (`INCAN-T0117`). An `int`, `float`, or `bool` parameter is the callee's own copy.
2. **Storage owns its value.** A field, an element, a collection or tuple literal, a return value, a `yield` value, and an argument for a parameter that is not caller-visible always hold an independent value, whichever answer this RFC settles on.
3. **One meaning per binding.** Whether a binding shares or copies depends only on what it is made from, never on a type annotation, on tuple unpacking, or on whether the value arrives through a `match` arm, an `if` branch, or a `break`.
4. **The open choice.** A new binding made from an existing collection, model, or class value is either a view of that value (Option A) or an independent value (Option B).
5. **Explicit copies are spelled.** `list(items)`, `dict(d)`, `set(s)`, `str(s)`, `.clone()`, or a newly constructed value make an independent copy under either option.
6. **The transition changes no meaning.** Until the choice is made, a caller-visible parameter is used only directly: binding it to another name or holding it in another value is refused.

## Motivation

### The `mut` parameter contract has no RFC

The language reference documents what `mut` on a parameter means, and the #1773 fixes (PR #1814) make the compiler follow it: which parameters show their changes to the caller, that an `int`, `float`, or `bool` parameter never does, that a caller-visible parameter cannot be rebound, and which arguments `INCAN-T0117` refuses, including the calls that count as "may change" because the body that runs is not known at the call. These rules decide what programs mean, yet they exist only as reference text and checker behavior. A design record should own them.

### The same binding shares or copies depending on spelling

The generated code represents a caller-visible parameter as a reference to the caller's value. A binding without a type annotation takes the representation of its value, so it takes the reference; a binding with an annotation, a reassignment, and a return take an owned value, so they copy. For `def f(mut items: list[int])`, without the transitional refusal:

| Spelling                                        | What the generated code does                                |
| ----------------------------------------------- | ----------------------------------------------------------- |
| `mut other = items`                             | shares: a change through `other` reaches the caller         |
| `mut other: list[int] = items`                  | copies                                                      |
| `other = items`, reassigning a `mut` binding    | copies                                                      |
| `return items`                                  | copies                                                      |
| a `match` arm value, `break items`              | shares, but the checker does not count it as a change       |
| `for xs in [items]:`                            | shares, but the checker does not count it as a change       |

Three consequences follow. Adding a type annotation changes what the function does. In the last two rows the checker does not see the change, so an immutable argument for a parameter it believes unchanged is handed over as a copy and the change is silently lost. And after `mut other = items`, any later use of `items` fails in the generated code, because the caller's reference has moved into `other`; the failure comes from the host compiler, after `incan check` accepted the program.

Ordinary locals are consistent: `mut b = a` copies whenever `a` is read again or is itself `mut`, and moves otherwise, where the move cannot be observed. Ordinary locals already have value semantics.

### RFC 023's invariant is broken

RFC 023 §6 makes ownership implicit: users do not annotate ownership, ambiguous cases fall back to cloning, and "emitted decisions must not change user-visible behavior." A binding that shares under one spelling and copies under another is an emitted decision that changes user-visible behavior. Either sharing is a meaning the language owns, in which case the language must say so and set that invariant aside for those bindings, or it is not, and the bindings must copy.

### Python readers expect sharing

In Python, `other = items` never copies: both names refer to one list, and a change through either is visible through both. Incan's surface invites that reading, and ordinary locals contradict it silently. Whichever answer the language takes, a reader must be able to tell from the source what a binding means, and ideally a program whose meaning depends on the difference should not compile without saying which it wants.

## Goals

- Specify the `mut` parameter contract: which parameters are caller-visible, which changes reach the caller, the scalar rule, the rebinding refusal, and `INCAN-T0117` with its "changes or may change" analysis.
- Give every spelling of a new binding from the same source one meaning, for ordinary locals and parameters alike, and state what storage (fields, elements, literals, returns) holds.
- Set out the two coherent meanings, views and values, with the complete rules each needs, and a recommendation.
- Record the transitional rule under which neither outcome changes the meaning of a program that compiles.
- State exactly which part of RFC 023 §6 the view option would supersede.
- Make it possible to replace a caller-visible collection's contents in place, which the rebinding refusal otherwise prevents: `clear()` on `list`, `dict`, and `set`.

## Non-Goals

- New syntax: reference types, borrow annotations, or a `ref` keyword. #121 rejected user-facing ownership syntax, and #1790 removes the one place ordinary code still writes `&`.
- Runtime sharing (reference counting) as the representation of collections, models, or classes.
- Identity across storage or a return. In both options a value that is stored or returned is independent of the binding it came from.
- Syntax for changing elements through a `for` target, such as `for mut row in rows`.
- The spelling of the `mut` marker in function types (#1790) and callable bounds (RFC 128); this RFC relies on them.
- Publishing the `mut` markers of a compiled library's methods in its checked metadata.
- Sharing values between concurrent tasks.

## Guide-level explanation

### `mut` parameters

A parameter declared `mut` can be changed in the function body. For a collection, a model, a class, or any other type except `int`, `float`, and `bool`, the change reaches the caller, and the caller passes a binding it declared `mut`:

```incan
model Order:
    id: int
    lines: list[str]


def add_line(mut order: Order, line: str) -> None:
    order.lines.append(line)


def tag(mut tags: list[str], label: str) -> None:
    if label not in tags:
        tags.append(label)


def countdown(mut remaining: int) -> int:
    mut steps = 0
    while remaining > 0:
        remaining -= 1
        steps += 1
    return steps


def main() -> None:
    mut order = Order(id=7, lines=[])
    add_line(order, "2 x widget")
    println(len(order.lines))       # 1: the change reached `order`

    mut tags = ["urgent"]
    tag(tags, "draft")
    println(len(tags))              # 2

    remaining = 3
    println(countdown(remaining))   # 3
    println(remaining)              # 3: an `int` parameter is the callee's own copy
```

The caller sees changes made to the value it passed, not a new value bound to the parameter's name, so rebinding a caller-visible parameter is refused:

```incan
def reset(mut tags: list[str]) -> None:
    tags = []    # INCAN-T0001: cannot rebind the 'mut' parameter 'tags'
```

A call that changes the parameter needs a place the caller may change. An immutable binding, a list or dict element, and a static are refused with `INCAN-T0117`; a literal or a call result is accepted:

```incan
def main() -> None:
    tags = ["urgent"]
    tag(tags, "draft")        # INCAN-T0117: declare 'tags' with 'mut'
    tag(["urgent"], "draft")  # accepted: nothing but `tag` holds the list
```

### Changing the caller's value in place

Every change the caller should see is made through the parameter itself: a method that changes it, an element or field assignment, or a call that passes it to another `mut` parameter. Replacing the whole contents clears the value and fills it again:

```incan
def replace_all(mut items: list[int], fresh: list[int]) -> None:
    items.clear()
    items.extend(fresh)


def bump_first(mut items: list[int]) -> None:
    items[0] = items[0] + 1


def drain_into(mut target: list[int], mut source: list[int]) -> None:
    while len(source) > 0:
        target.append(source.pop())
```

`clear()` is new with this RFC; `list`, `dict`, and `set` have no method that empties them today.

### A new binding from an existing value

This RFC decides what the following means:

```incan
class Inbox:
    pending: list[str]
    archived: list[str]

    def archive_all(mut self) -> None:
        mut queue = self.pending
        while len(queue) > 0:
            self.archived.append(queue.pop())
```

A Python reader expects `queue` and `self.pending` to be one list, so `archive_all` empties `pending`. The two options answer differently.

**Option A, views.** `mut queue = self.pending` makes `queue` a *write view* of `self.pending`: it is the same list under another name, so `archive_all` empties `pending`, as in Python. `let` makes a *read view* instead. A view is a borrow in the generated code, so the language adds one rule: while a view is still used later, its source cannot be used in a way that conflicts with it.

```incan
def tag_twice(mut tags: list[str]) -> None:
    mut view = tags
    view.append("a")
    tags.append("b")    # refused: 'view' is a write view of 'tags' and is used again below
    view.append("c")
```

Moving `tags.append("b")` after the last use of `view` makes the function compile. An independent copy is written explicitly; Option A adds Python's `copy()` for it, beside `list(tags)`.

**Option B, values.** `mut queue = self.pending` makes `queue` an independent list, as ordinary locals behave today. `archive_all` moves the copied messages into `archived` and leaves `pending` full, which is not what a Python reader expects. Option B therefore comes with an *explicit-copy rule* (Unresolved question 2): a binding whose copy behaves differently from sharing is refused until the copy is written out. For `archive_all` the refusal reads (wording illustrative):

```text
'queue' is a copy of 'self.pending', and changes to it do not reach 'self.pending', which the caller sees
  hint: change 'self.pending' directly, or write list(self.pending) for an independent copy
```

The fix is to change `self.pending` directly (`self.archived.append(self.pending.pop())`), or to write `list(self.pending)` when a copy is really wanted. Under that rule every program Option B accepts means the same thing whether its bindings are read as sharing or as copying.

Both options refuse a read of an old value after its source changes, for different reasons:

```incan
def summarize(mut readings: list[int]) -> int:
    let before = readings
    readings.append(0)      # Option A: refused, 'before' is a read view still used below
    return len(before)      # Option B: refused by the explicit-copy rule; write list(readings)
```

### During the transition

Until this RFC is decided, a caller-visible parameter is used only directly. Binding it to another name, holding it in a tuple, list, set, or dict literal, a comprehension, or a field, producing it as a `match`, `if`, or `break` value, binding it in a `match` arm, and changing it inside a closure are refused under `INCAN-T0001`:

```incan
def normalize(mut scores: list[int]) -> None:
    mut working = scores
    working.append(0)
```

```text
INCAN-T0001: The 'mut' parameter 'scores' cannot be bound to another name or held in another value
  hint: Change 'scores' directly, or pass it to a function that takes a 'mut' parameter; for an independent copy, write list(scores)
```

`return scores` stays accepted and returns an independent copy, as it does under both options.

## Reference-level explanation

### Terms

- **Caller-visible parameter.** An ordinary parameter, not `*args` or `**kwargs`, declared `mut`, whose type with type aliases expanded is not `int`, `float`, or `bool` and does not name an imported Rust type. `self` in a method declared `mut self` is caller-visible in the same way.
- **Place.** A binding name; a field access `p.f` or an element access `p[k]` whose base `p` is a place; or a parenthesized place. `items[1:]`, a call, a literal, an operator result, and a static are not places. The **root** of a place is its binding name.
- **Overlap.** Two places overlap when they have the same root and the field path of one is a prefix of the other's. Element accesses into the same collection overlap each other.
- **Shareable type.** A type whose values can be changed in place and are not copied implicitly: every type except the numeric types, `bool`, `str`, `bytes`, tuples, the frozen collections, `None`, a type that derives `Copy`, and an imported Rust type. Collections, models, and classes are the main cases.
- **New binding.** `let n = e`, `mut n = e`, and a plain `n = e` that introduces `n`, each with or without a type annotation; each name bound by tuple unpacking; a name a `match` arm pattern binds to the whole matched value; and the target of a `for` statement whose iterated collection is a place, made from each of its elements in turn.
- **Storage.** A field set by a constructor or by field assignment; an element of a collection or tuple literal or of a comprehension, or one set by element assignment or added by a method such as `append` or `insert`; a return value; a `yield` value; and an argument for a parameter that is not caller-visible.

### The `mut` parameter contract

1. A parameter declared `mut` is a mutable binding in its function's body.
2. A caller-visible parameter shares the caller's argument for the duration of the call. Every in-place change the body makes to it is visible to the caller after the call: a method call that changes it, an element or field assignment through it, and passing it to another caller-visible parameter or to a Rust or C parameter that takes it exclusively.
3. A `mut` parameter that is not caller-visible is the function's own value. The body may change and rebind it, and the caller never sees either.
4. Rebinding a caller-visible parameter, by assignment or compound assignment to its name, must be refused under `INCAN-T0001`, with a hint to change it in place.
5. An argument for a caller-visible parameter that the call changes or may change must be a place the caller may change: a `mut` binding, a caller-visible parameter of the calling function, `self` in a `mut self` method, or a field of one of those. An immutable binding or a field of one, a collection element, and a static must be refused under `INCAN-T0117`. A temporary, such as a literal or a call result, must be accepted. An argument for a caller-visible parameter the call never changes may be any value, and the callee receives the argument's value.
6. A call *changes* a caller-visible parameter when the body that runs changes it as rule 2 describes. A method call counts as a change unless the method is known only to read its receiver: a reading method of a standard collection, or a source method none of whose overloads takes `mut self`. A method the check cannot resolve counts as a change.
7. A call *may change* a caller-visible parameter when the body that runs is not known where the call is checked: a method reached through a type parameter's bound, on `self` in a trait default, or on a trait-typed value; a callee known only by a callable type that marks the parameter `mut` (#1790); a function whose body the check does not read, such as a compiled library's; and a call through a function-valued local that is reassigned anywhere in the module. A call through a function-valued local that is never reassigned is checked as a call of that function.
8. A closure may use a caller-visible parameter only while the call is running. A closure that uses one must not be returned, stored, or yielded.

### Rules common to both options

- **One meaning per binding.** Whether a new binding or a reassignment shares or copies must depend only on its source. A type annotation, tuple unpacking, and a value that arrives through a `match` arm, an `if` branch, or a `break` must not change it.
- **Storage owns.** A value placed in storage must be independent of every binding. When the stored expression is a place, storage receives a copy of the place's current value, except that the value may be moved when the place is a local binding that nothing uses afterward. `return items` on a caller-visible parameter returns a copy. Whether other storage made from a caller-visible parameter must spell its copy is Unresolved question 3.
- **Explicit copies.** An independent copy is written `list(x)`, `dict(x)`, `set(x)`, or `str(x)` for those types, `x.clone()` for a list and for a model or class that has `Clone` (the language reference lists `Clone` among the derives the compiler adds to every model and class), or by constructing a new value.
- **Clearing.** `list`, `dict`, and `set` must offer `clear()`, which removes every element in place.
- **Generic copies.** Where a rule copies a value of a generic type, the copy requires `Clone` of that type, which bound inference supplies (RFC 023, "clone implies `Clone` bound").
- **No shape decides sharing.** Whether a binding shares must follow from the chosen option's rules, never from how the generated code represents its source.

### Option A: views

- **A1. Views.** A new binding whose value is a place of shareable type, or a `match`, `if`, or `loop` expression each of whose results is such a place, is a *view* of that place, its *source*. `let`, a plain introducing assignment, a `match` arm binding, and a `for` target make a *read view*; `mut` makes a *write view*. A type annotation must name the source's type and does not change the rule.
- **A2. Meaning.** Reading through a view reads its source. An in-place change through a write view changes its source. A read view cannot be changed through. A view made from a view has the outer view's source.
- **A3. Changeable sources.** A write view's source must be rooted at a `mut` binding, a caller-visible parameter, or `self` in a `mut self` method. When it is rooted at an immutable local that nothing uses after the view is made, the value moves into the new binding, which is then an ordinary binding. Any other write view must be refused, with a hint to declare the root `mut` or to bind a copy.
- **A4. Liveness.** A view is live from its binding to its last use. A use through a view made from it, a call of a closure that uses it, and a use in a later iteration of an enclosing `for`, `while`, or `loop` body are uses.
- **A5. Conflicts.** While a read view is live, a place that overlaps its source must not be changed. While a write view is live, a place that overlaps its source must not be used at all except through the view. Passing an overlapping place to a caller-visible parameter, or calling a method on it that may change it, is a change. A conflicting use must be refused under a new stable code.
- **A6. Suspension.** A view must not be live across an `await` or a `yield`. Such a program must be refused under the same code, since the generated code cannot hold the view across the suspension and a copy would change the program's meaning.
- **A7. Reassignment.** Reassigning a `mut` view to a place of the same type makes it a view of that place, subject to A3; it never writes through the view. Reassigning a view to a value that is not a place must be refused, with a hint to bind the value to a new name. Reassigning a binding that is not a view to a place of shareable type must be refused, with a hint to write a copy or to introduce a view with a new binding, unless the place is a local that nothing uses afterward, in which case the value moves.
- **A8. Storage.** A view placed in storage follows "Storage owns": storage receives a copy of the source's current value, never a view (Unresolved question 3).
- **A9. Closures.** A closure may use a view while the view is live, and extends the view's liveness to the closure's last call. A closure that uses a view must not be returned, stored, or yielded.
- **A10. Parameters.** A caller-visible parameter is a write view of the caller's argument for the duration of the call. A parameter that is not caller-visible is the function's own value, and a view made from it is a view of that value.
- **A11. Change analysis.** An in-place change through a view whose source is rooted at a caller-visible parameter is a change to that parameter, for rule 6 of the contract and for `INCAN-T0117`.
- **A12. `copy()`.** `list`, `dict`, and `set` must offer `copy()`, returning an independent value equal to the receiver. No `copy()` exists today. Because storage owns its value, a copied list's elements are copies too, where Python's `copy()` shares them.
- **A13. Scope.** These rules apply to every new binding in every function, ordinary locals and parameters alike.

### Option B: values

- **B1. Values.** A new binding or a reassignment whose value is a place, directly or through a `match` arm, an `if` branch, or a `break`, holds an independent value: a copy of the place's value at that point. A `for` target over a place holds a copy of each element. The generated code may move or borrow instead only when no program behavior can tell the difference.
- **B2. Changes reach a caller only through the parameter.** A caller sees a change only when it is made through a caller-visible parameter itself, including its fields and elements, or when the parameter is passed to another caller-visible parameter or to a Rust or C parameter that takes it exclusively. A binding made from a caller-visible parameter is not the parameter, and changing it is not a change to the parameter.
- **B3. Explicit-copy rule** (Unresolved question 2). A new binding or a reassignment whose value is a place must be refused when its copy can be told apart from sharing: when the binding is changed afterward and a place that overlaps the source is used after that change, or the source is rooted at a caller-visible parameter or at `self` in a `mut self` method, where the caller observes it; or when a place that overlaps the source is changed while the binding is still read afterward. The diagnostic must name the binding, its source, and the use that makes the copy observable, and offer the explicit copy spelling and, where the binding is changed, changing the source directly.
- **B4. Scope.** These rules apply to every new binding in every function, ordinary locals and parameters alike.

### The transitional rule

This rule is in force until this RFC leaves Draft, and is what the compiler applies with the #1773 fixes.

- **T1.** A caller-visible parameter must be used only directly. It must be refused, under `INCAN-T0001`, as the value of an assignment that binds or reassigns a name (`let other = items`, `mut other = items`, `mut other: list[int] = items`, `other = items`, and a tuple unpacking whose value it is); as an element of a tuple, list, set, or dict literal, or of a comprehension or generator expression; as the value of a field or element assignment (`holder.items = items`, `rows[0] = items`); as the value of a `match` arm, an `if` branch, or a `break`; as the scrutinee of a `match` with an arm that binds the whole value to a name; and inside a closure that changes it or passes it to a parameter that may change it.
- **T2.** The refusal's hint must name changing the parameter directly and passing it to a function that takes a `mut` parameter, and must spell an independent copy: `list(items)`, `dict(items)`, `set(items)`, or `str(items)` for those types, and a new value built from the parameter otherwise.
- **T3.** `return items` must be accepted and return an independent copy. An argument for a caller-visible parameter passes the parameter on; an argument for any other parameter hands over its value.
- **T4.** The rule should also cover `self` in a `mut self` method, which is caller-visible in the same way; the compiler does not refuse it yet.
- **T5.** When this RFC leaves Draft, the chosen option's rules replace T1 to T4.

Every spelling T1 refuses either shares in the generated code today or means something different under the two options, and T3's shapes mean the same under both. No program that compiles under T1 to T4 changes meaning under either option.

### Diagnostics

- The transitional refusal uses `INCAN-T0001`, as described under T1 and T2.
- Under Option A, a view conflict, including a view live across an `await` or a `yield`, must be refused under a new stable `INCAN-T` code with a catalog entry and `incan explain` text. The message must name the view, the conflicting use, and the later use of the view that keeps it live. The refusals under A3 and A7 may share that code or take their own.
- Under Option B with B3, an observable copy must be refused under a new stable `INCAN-T` code with a catalog entry and `incan explain` text.
- `INCAN-T0001` for rebinding and `INCAN-T0117` keep their meanings.

## Design details

### Syntax

No new syntax. `clear()` and, under Option A, `copy()` are methods.

### Summary by spelling

For `def f(mut items: list[int])` and ordinary locals `a` and `b`:

| Spelling                                   | Without the refusal          | Transition | Option A                       | Option B                          |
| ------------------------------------------ | ---------------------------- | ---------- | ------------------------------ | --------------------------------- |
| `mut other = items`                        | shares                       | refused    | write view                     | copy; B3 refuses it if changed    |
| `mut other: list[int] = items`             | copies                       | refused    | write view                     | copy; as above                    |
| `let other = items`                        | shares; `items` then unusable | refused   | read view                      | copy; B3 refuses a stale read     |
| `other = items`, reassigning               | copies                       | refused    | re-targets a view (A7)         | copy                              |
| `match` arm, `if` branch, `break` value    | shares, change unseen        | refused    | view of the selected place     | copy                              |
| `match items: xs => ...`                   | shares                       | refused    | read view                      | copy                              |
| `[items]`, `(items, 1)`, `{"k": items}`    | shares in `for xs in [items]` | refused   | copy, or refused (question 3)  | copy, or refused (question 3)     |
| `holder.items = items`, `rows[0] = items`  | copies                       | refused    | copy, or refused (question 3)  | copy, or refused (question 3)     |
| `return items`                             | copies                       | copy       | copy (storage)                 | copy (storage)                    |
| a closure that changes `items`             | shares                       | refused    | uses the parameter (rule 8)    | uses the parameter (rule 8)       |
| local `mut b = a`                          | copies if `a` is read again  | unchanged  | write view                     | copy; B3 refuses it if observable |
| `mut xs = self.items` in `mut self`        | copies                       | unchanged  | write view                     | copy; B3 refuses it if changed    |

### Interaction with existing features

- **Closures.** Rule 8 of the contract applies under both options. Under Option A, a closure that uses a view keeps it live (A9). Closure captures are not new bindings: a closure that reassigns or changes an outer binding changes that binding, as today.
- **`for`, `while`, and `loop` bodies.** Under Option A, liveness spans iterations (A4). A `for` target over a place is a read view of each element under Option A and a copy of each element under Option B, so Python's `for row in rows: row.append(x)` is refused under Option A (the target cannot be changed through) and under B3 (the copy's change is observable); a target that changes elements needs syntax this RFC does not add.
- **`match` and `if` expressions, `break` values (RFC 016).** A value selected by an arm, a branch, or a `break` follows the same rule as the place written directly.
- **Generators (RFC 006) and `await`.** Under Option A, no view may be live across a `yield` or an `await` (A6). Under Option B, bindings are values and nothing changes.
- **Traits and generic dispatch.** A method reached through a bound, a trait default's `self`, or a trait-typed value may change a caller-visible parameter (contract rule 7), under both options.
- **Callable types (RFC 128, #1790).** A function type marks its caller-visible parameters; a callee known only by the type may change each of them. `Callable[(mut T) -> R]` inherits the marker.
- **Rust and C interop.** Passing a caller-visible parameter to a Rust or C parameter that takes it exclusively is a change. A `mut` parameter of an imported Rust type is handed over whole and is not caller-visible.
- **Static storage.** A static is not a place for this RFC. A binding made from a static holds a copy under both options, and a static is refused as an argument for a changed caller-visible parameter (`INCAN-T0117`).
- **Duckborrowing.** Under Option B the ownership planner keeps its freedom to move and borrow wherever the difference cannot be observed, and RFC 023 §6 is unchanged. Under Option A the planner must represent a view as a borrow and never copy it.

### Supersession of RFC 023 §6 (Option A only)

Closed RFCs are not edited. If Option A is chosen, this RFC replaces the following parts of RFC 023 §6, for view bindings only:

- the invariant that "emitted decisions must not change user-visible behavior": whether a view shares is the program's meaning, not an emitted decision, and the generated code must preserve it;
- the fallback that ambiguous cases "fall back to cloning", and "when in doubt, clone" for borrows across `await`: a view must never be copied, and where the generated code cannot keep a view, the checker refuses the program (A5, A6).

The rest of §6 stays in force: users write no ownership, borrowing, or lifetime syntax; read-only, mutated, and consumed parameters, primitives, and return values keep their strategies; and every decision the planner makes for a value that is not a view keeps the invariant and the clone fallback. Under Option A, `let` and `mut` on a view choose between a shared and an exclusive borrow, so RFC 023's promise that users do not annotate borrowing holds in syntax but not in effect for view bindings.

Option B supersedes nothing. It removes the spelling-dependent sharing that breaks the invariant.

### Compatibility and migration

- **The transitional rule** refuses programs that compiled before the #1773 fixes and used a refused spelling. Each fix is to change the parameter directly or to write an explicit copy. A program that used `items` after `mut other = items` already failed in the generated code.
- **Option A** changes the meaning of ordinary locals where `mut b = a` is followed by a change through `b` and a later read of `a`, and of bindings made from fields and elements (`mut xs = self.items`): these copy today and would share. It also refuses programs that compile today: conflicts (A5), views across a suspension (A6), and the reassignments of A7. Adopting it therefore needs a release in which the compiler warns at each binding whose meaning would change, before the meaning changes.
- **Option B** changes the meaning of no program that compiles under the transitional rule. With B3, programs whose copy is observable are refused, and each fix is a mechanical explicit copy or a direct change of the source.
- **`clear()`**, and `copy()` under Option A, are additions.

## Alternatives considered

- **Runtime sharing.** Collections, models, and classes become reference-counted, with borrow checks at run time. This matches Python exactly, including identity across storage and returns. But every access pays for a count or a borrow flag, an aliasing mistake becomes a run-time panic instead of a compile-time refusal, reference cycles leak, and values reachable from several tasks need atomic counts and locks. It gives up the performance premise of compiling to Rust.
- **Sharing for parameters only.** Views for bindings made from caller-visible parameters, copies for everything else. The same spelling then has two meanings depending on where its source came from, so moving code from a function body into a helper that takes a `mut` parameter changes what it does.
- **Keeping the generated code's answer.** Spelling-dependent, loses changes the checker cannot see, and fails in generated code; it is the problem this RFC exists to solve.
- **Explicit borrow syntax.** `&` and `&mut` types or `ref` bindings would let authors choose sharing directly. #121 rejected user-facing ownership syntax, and #1790 is removing the last place ordinary code writes `&`.
- **Copy-on-write collections.** Values whose storage is shared until the first change would make Option B's copies cheap without changing its meaning. This is a representation Option B could adopt later, not a third meaning.

## Drawbacks

- The transitional rule refuses programs that compiled before it.
- **Option A** makes borrow conflicts language rules, reported as Incan diagnostics: the rules #121 set out to keep out of the source. Its Python fidelity is partial: storage and returns copy, a source cannot even be read while a write view of it is live, which Python allows, and views cannot cross an `await` or a `yield`. `mut` gains a second meaning on a view, write access to the source. It supersedes part of RFC 023 §6. Its conflict check needs liveness across branches, repeated bodies, and closures, which the compiler does not have yet (#1611), and until it does the check refuses what it cannot prove. It changes the meaning of existing programs.
- **Option B** contradicts a Python reader's expectation that `mut other = items` shares; without B3 the difference is silent. Copies of large values cost time and memory where the planner cannot prove a move or a borrow unobservable. With B3, some programs that compile today are refused until an explicit copy is written.
- **Both options** lose identity at storage and returns: a list stored in a field or returned is never the list its binding named. After `rows.append(row)`, a change to `row` does not reach `rows`, and no rule in this RFC reports it for an ordinary local.

## Implementation architecture

This section is non-normative.

- **The source of the inconsistency.** The generated code represents a caller-visible parameter as a reference, and an unannotated binding takes the representation of its value. Every option starts from the same correction: a binding's representation follows the option's rule, never the representation of its source.
- **Liveness.** A5, A6, and B3 need the last use of each binding across branches, repeated bodies, and closures. The planner's last-use facts today are per-block read counts; #1611's liveness step replaces them. Both Option A's conflict check and B3 should be built on that one analysis and run in the checker, so every refusal is an Incan diagnostic and the host compiler never reports a borrow error in generated code.
- **Option A in generated code.** A view lowers to a reborrow of its source, never to a move of a caller-visible parameter's reference; a view selected by a `match` or `if` lowers to a borrow of the selected place; re-targeting assigns a new borrow. The planner must treat a view as fixed: no copy, and no owned materialization except where A8 stores it.
- **Option B in generated code.** A binding copies unless the planner proves a move or a borrow unobservable, which the liveness facts decide. Under B3 the checker has already refused every binding whose copy is observable, so a move or a borrow is legal for most accepted bindings and the copy disappears.

## Layers affected

- **Typechecker / Symbol resolution**: the contract's rules, as the #1773 fixes implement them; the transitional refusal, extended to `self` in a `mut self` method; the change analysis follows the chosen option (views under A, the parameter only under B); under Option A, view classification, liveness, conflicts, suspension, reassignment, and the conflict code; under Option B with B3, the explicit-copy check and its code.
- **IR Lowering**: a binding's representation follows the chosen rule rather than its source's; under Option A, views lower to reborrows; under Option B, copies are planned by duckborrowing with the liveness facts.
- **Emission**: under Option A, borrow-shaped local bindings and re-targeting assignments; under Option B, nothing beyond the planner's existing conversions.
- **Stdlib**: `clear()` on `list`, `dict`, and `set`; under Option A, `copy()` on the same types.
- **Diagnostics catalog**: the new stable codes with catalog entries and `incan explain` text; the CLI reference lists them.
- **LSP / Tooling**: hover on a binding made from a place states whether it is a view of its source (Option A) or an independent copy (Option B); diagnostics carry the spans named above.
- **Docs**: the functions reference (the `mut` parameter contract), the scopes and name resolution explanation (what a binding made from a value means), the guide for Python users, and release notes.

## Inspectability and tooling surface

- **Artifact or metadata:** none new. Whether a parameter is caller-visible is already part of a function's checked signature through the `mut` marker (#1790).
- **Inspection command:** `incan check` reports every refusal; `incan explain` renders the new codes, `INCAN-T0001`, and `INCAN-T0117`; LSP hover shows what a binding made from a place is.
- **Diagnostics:** the transitional refusal under `INCAN-T0001`; under Option A the view conflict, including suspension; under Option B with B3 the explicit-copy refusal; `INCAN-T0001` for rebinding; `INCAN-T0117` for an argument the caller cannot change.
- **Provenance:** a view conflict names the view's binding, the conflicting use, and the later use of the view; an explicit-copy refusal names the binding, its source, and the use that makes the copy observable; `INCAN-T0117` names the callee and the parameter and says whether the callee changes or may change it.
- **Not implicit:** under Option A the generated code never copies a view; under Option B nothing shares except a caller-visible parameter; under both, the generated code's representation of a value never decides whether a binding shares.

## Acceptance criteria

This RFC is done when:

- every row of the summary table behaves as the chosen option's column says, with a behavior fixture for each: `mut`, annotated `mut`, `let`, reassignment, tuple unpacking, a `match` arm, an `if` branch, a `break` value, a `match` arm binding, collection and tuple literals, field and element assignment, `return`, a closure, an ordinary local, and a field of `self`;
- the contract's rules have fixtures: changes to a list, dict, model, and class parameter reach the caller; an `int`, `float`, and `bool` parameter's do not; rebinding is refused under `INCAN-T0001`; `INCAN-T0117` refuses each immutable argument kind and each "may change" callee kind and accepts temporaries; a closure that uses a caller-visible parameter cannot be returned or stored;
- no spelling makes a later use of a caller-visible parameter fail in generated code;
- `clear()` works on `list`, `dict`, and `set`, with a fixture that replaces a caller-visible list's contents in place;
- under Option A: each conflict kind of A5, a view across an `await` and across a `yield`, the refusals of A3 and A7, a view used by a closure, a view used in a later iteration, a view placed in storage, and `copy()` each have a checker test or fixture, and the release notes record the supersession of RFC 023 §6;
- under Option B with B3: each observable-copy kind is refused with its explicit-copy hint, each accepted binding builds, and codegen snapshots show no copy for a binding that is only read while its source is unchanged, or that is its source's last use;
- the documentation named under Layers affected describes the chosen rule, and the CLI reference lists the new codes.

## Design Decisions

- **The `mut` parameter contract** is as specified: caller-visible parameters share the caller's argument; an `int`, `float`, or `bool` parameter is the callee's own copy; a caller-visible parameter cannot be rebound (`INCAN-T0001`); and `INCAN-T0117` refuses an argument the caller cannot change for a parameter the call changes or may change, with trait dispatch, callable types, unread bodies, and reassigned function locals counting as "may change".
- **One meaning per binding.** Annotations, tuple unpacking, and selection through `match`, `if`, or `break` never change whether a binding shares.
- **Storage owns.** Fields, elements, literals, return values, `yield` values, and arguments for parameters that are not caller-visible always hold an independent value.
- **Copies are spelled with today's spellings**: `list(x)`, `dict(x)`, `set(x)`, `str(x)`, `.clone()`, or a new value. `clear()` makes in-place replacement possible.
- **No new syntax.** Neither option adds reference types or borrow annotations.
- **The transition changes no meaning.** Until the decision, a caller-visible parameter is used only directly.
- **The checker decides sharing**, never the generated code's representation of a value.

## Unresolved questions

- **Option A (views) or Option B (values)?** Incan's design leans toward Python where the generated code can follow it, and that lean favors Option A: `mut xs = self.items; xs.append(x)` would do what a Python reader expects. Recommendation: Option B, for five reasons. It is how ordinary locals already behave, so the rule the language has for most bindings becomes its only rule, and moving from the transitional rule to Option B changes no program's meaning. It keeps RFC 023 §6 whole: ownership stays an implementation decision the user never sees, and no borrow rule becomes a language rule, where Option A turns borrow conflicts into Incan diagnostics. Option A's Python fidelity is partial anyway: storage and returns copy under it too, a source cannot be read while its write view is live, and views cannot cross an `await` or a `yield`. The idiom Option A serves has a direct spelling that works under both options, `self.items.append(x)`. And with the explicit-copy rule (the next question), Option B never gives an accepted program a meaning that differs from how a Python reader reads its bindings, which serves the Python lean without views. The costs are copies where the planner cannot prove them unobservable, and refusals that ask for an explicit copy.
- **Under Option B, is a copy that can be told apart from sharing accepted silently, warned about, or refused until the copy is written out (B3)?** Silently is how ordinary locals behave today, and it is how `archive_all` above loses its messages without a word. Recommendation: refuse (B3), for every binding including ordinary locals, with a diagnostic that offers the explicit copy or a direct change of the source. Each fix is mechanical, and the refused programs are the ones whose meaning a Python reader would get wrong.
- **Does storage made from a caller-visible parameter, or under Option A from a view, copy silently, or must the copy be spelled?** A silent copy follows "Storage owns" and lets `holder.items = items` and `for xs in [items]:` compile, but a later change through the stored copy never reaches the caller's value, which is exactly what the transitional rule refuses today: in `for xs in [items]: xs.append(3)` the change is lost. The checker cannot follow a stored copy, so no liveness rule can tell whether the loss matters. A spelled copy (`holder.items = list(items)`) never diverges from Python silently, at the cost of a refusal for each such shape. Recommendation: keep the transitional refusals for a caller-visible parameter or a view written as a literal or comprehension element, or as the value of a field or element assignment, with the explicit copy in the hint; keep `return items` and argument passing as silent copies, because a return is where every language without runtime sharing copies and a parameter that is not caller-visible cannot be changed by the callee.

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
