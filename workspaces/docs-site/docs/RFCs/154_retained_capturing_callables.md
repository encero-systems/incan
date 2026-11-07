# RFC 154: Retained capturing callables

- **Status:** Planned
- **Created:** 2026-10-17
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 036 (user-defined decorators)
    - RFC 042 (trait names as abstract types)
    - RFC 084 (callable presets)
    - RFC 088 (lazy iterator adapters)
    - RFC 094 (scoped resources)
    - RFC 103 (secret values)
    - RFC 115 (fallible iterator callbacks)
    - RFC 128 (callable types and invocation guarantees)
    - RFC 129 (binding and ownership)
    - RFC 141 (`ArgumentsOf` and wrapping decorators)
    - RFC 142 (tasks and task groups)
    - RFC 143 (explicit cloning)
    - RFC 153 (async closures)
    - RFC 164 (explicit ownership and borrowing)
    - [#1985](https://github.com/encero-systems/incan/issues/1985) (originating lazy iterator adapter case)
- **Issue:** [#1987](https://github.com/encero-systems/incan/issues/1987)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

A closure that reads a local can be used wherever a callable can be used: in a lazy iterator adapter, a model field, a list, a returned value, or across a library boundary. Its captures follow the same ownership rules as any other value. Only how long the closure lives changes the outcome, so an unsupported case gets a diagnostic naming the capture, the slot that keeps the closure and what to do, rather than a blanket refusal. Changing a value while a closure still uses it is refused rather than silently cloned, and a function that keeps a closure says so in its published signature, inferred from its body.

## Core model

1. **One rule for every slot.** A `Result` combinator, an iterator adapter, a field and a return all use the same capture rule. Only the closure's lifetime differs.
2. **Captures follow ownership.** A copyable value is copied, and anything else is borrowed or moved under RFC 129 and RFC 164.
3. **A closure sees the value as it was built.** A copy cannot change, and the checker refuses changing a borrowed value while the closure is live.
4. **No hidden clones.** `.clone()` is how an author asks for an independent snapshot.
5. **A kept closure owns its captures.** A closure that outlives the scope around it cannot borrow from that scope.
6. **The signature says what happens.** Whether a function keeps a callable is inferred from its body and published in its checked signature.

## Motivation

These all refuse today, with `INCAN-T0001`:

```incan
def matching_lines(lines: list[str], pattern: str) -> list[str]:
    return lines.iter().filter((line) => pattern in line).collect()

model Adjustment:
    apply: Callable[(int) -> int]

def offset_adjustment(offset: int) -> Adjustment:
    return Adjustment(apply=(value) => value + offset)
```

while `result.map((value) => value + offset)` works. A user cannot predict which from the call's shape, because the refusal comes from how a slot stores a callable rather than from anything wrong with the program. Most useful adapter callbacks capture something, so the rule removes most of the pipeline surface in practice, along with callback registries, stored handlers, strategy fields and wrapping decorators.

Today's slots also hide a bug. When later code still needs a captured list, the closure gets a silent clone of it, so a change made after the closure was built is silently ignored.

## Goals

- Accept capturing closures in every callable slot: adapters, fields, collections, returns, branches, presets, decorators, tasks and library boundaries.
- Give captures the ownership rules every other value already has.
- Replace blanket refusals with diagnostics that name the capture, the slot and the fix.
- Remove the hidden clone.
- Make keeping a callable visible across package boundaries without new syntax.

## Non-Goals

- Letting a closure change what it captures.
- Making lazy adapters eager.
- Making every callable `Send`, `Static` or repeatable.
- Changing RFC 128's `Callable[...]` spelling, signatures or invocation markers.

## Guide-level explanation

Accepting these programs is the change this RFC proposes. They use RFC 128's `Callable[...]` spelling.

### What a closure does with what it reads

A closure reading a local, a parameter or `self` captures it. What happens depends on the value and on how long the closure lives:

| Captured value | The closure ends with the scope around it | The closure outlives that scope: stored in a field or list, returned, kept by a callee |
| --- | --- | --- |
| `int`, `float`, `bool`, `@derive(Copy)` types | copied | copied |
| Any other value | borrowed | moved in if the local is not used again; otherwise refused, and you write `.clone()` |
| A borrowed parameter, or a view entered by `with` | borrowed, and the closure must finish before its source does | refused, unless the result declares `from source` |
| `self` | borrowed | refused when stored into `self` itself |

So a pipeline just works:

```incan
def matching_lines(lines: list[str], pattern: str) -> list[str]:
    return lines.iter().filter((line) => pattern in line).collect()
```

The filter keeps the closure until `collect` runs, and everything finishes inside the function. The closure still runs lazily: building the filter reads no lines.

A copy cannot change, so this uses 2 for every item:

```incan
def doubled(items: list[int]) -> list[int]:
    mut factor = 2
    projection = (value) => value * factor
    pending = items.iter().map(projection)
    factor = 9
    return pending.collect()
```

Module statics are read by name, so a closure does not capture them.

### Change a value while a closure uses it

A value that is not copied is borrowed, so it cannot change while the closure is live. Today this program silently clones `known`, and `missing` ignores `"range"`:

```incan
mut known = ["print", "len"]
missing = refs.iter().filter((name_ref) => name_ref.name not in known)
known.append("range")
report(missing.collect())
```

Now it is refused, and the diagnostic names all three places:

```text
error: `known` is changed while a closure still uses it
  --> resolve.incn:3
 2 | missing = refs.iter().filter((name_ref) => name_ref.name not in known)
   |                                     the closure borrows `known` here ^
 3 | known.append("range")
   | ^ changed here
 4 | report(missing.collect())
   |        ^ the closure is still used here
help: move the change before the closure, or capture `known.clone()`
```

The author decides which one they meant. Moving the `append` up makes `missing` see `"range"`, and `snapshot = known.clone()` keeps the old list. A closure is live until its last use, so changing `known` after the `collect` is fine.

### Keep a closure

An event bus keeps its handlers after `subscribe` returns:

```incan
class EventBus:
    handlers: list[Callable[(Event) -> None]]

    def subscribe(mut self, handler: Callable[(Event) -> None]) -> None:
        self.handlers.append(handler)
```

The handler now outlives the code that made it, so it must own what it captures:

```incan
def install_audit(mut bus: EventBus, log_path: Path) -> None:
    bus.subscribe((event) => append_line(log_path, event.summary()))
```

```text
error: `EventBus.subscribe` keeps `handler`, which captures `log_path`, and `install_audit` only borrows it
help: take `log_path: Owned[Path]`, or capture `log_path.clone()`
```

A local that is not used again moves in with no extra spelling:

```incan
def install_default_audit(mut bus: EventBus) -> None:
    log_path = Path("audit.log")
    bus.subscribe((event) => append_line(log_path, event.summary()))
```

### Hold different callables together

A trait in annotation position already means any value that satisfies it, under RFC 042 and RFC 128, so one collection holds a named function, a closure and a callable class:

```incan
class Router:
    routes: dict[str, Callable[(Request) -> Response]]

    def add(mut self, path: str, handler: Callable[(Request) -> Response]) -> None:
        self.routes[path] = handler

router.add("/health", health)
router.add("/orders", (request) => list_orders(request, db_url.clone()))
router.add("/metrics", MetricsHandler(prefix="checkout"))
```

When twenty routes would each clone one configuration, pass it as a parameter instead:

```incan
routes: dict[str, Callable[(Request, Borrowed[AppConfig]) -> Response]]
```

### Return a closure

A returned closure that reads a parameter depends on it, and RFC 164 carries that dependency through the result:

```incan
def matcher(patterns: list[str]) -> Callable[(str) -> bool]:
    return (name) => name in patterns
```

The caller keeps `patterns` alive while it uses the matcher. A wrapping decorator such as RFC 141's `timed` is the same case: its result keeps the function it wraps.

### Tasks and async

A task outlives the code that spawns it, so everything it captures is kept. `spawn`, `spawn_blocking` and `TaskGroup.spawn` add RFC 142's `Send` and `Static` requirements, and a borrowed capture fails them with the same kind of diagnostic. An async closure held in a field and called for each request needs owned captures and repeatable calls.

## Reference-level explanation

### Captures

1. A closure or local partial reading a local, a parameter or `self` must capture it under the table above. Module statics must not be captured. A local partial's presets are captures.
2. A copyable capture must be copied at construction. Every other capture must follow RFC 129's binding rules and RFC 164's modes: borrowed when the closure ends within the source's validity, and moved in when it outlives it and the local has no later use.
3. A capture must never be cloned implicitly. Where a closure outlives its scope and the local is used again, the program must be refused and the diagnostic must offer `.clone()` and `Owned[T]`.
4. Changing a borrowed capture while the closure is live must be refused under RFC 129. A closure is live through its last dependent use, including uses through an adapter or result that retains it.
5. A closure must not change what it captures, as today, and retaining it must not grant that.
6. A closure capturing `self` must be refused when stored into that same `self`, under RFC 164. Capturing a view entered by `with` must follow RFC 094: the closure must end before the suite does.
7. A lazy adapter must not run its callback until consumption requires it. A capture and a call are separate events.
8. Secret values must keep their RFC 103 protections when captured, stored or inspected.

### Slots

9. Every callable slot must accept a capturing callable, subject to these rules: a new local, a callee, an argument, a return, a field, a collection element, a branch value, a reassignment, a partial preset, a decorator's result, and a task.
10. A slot typed `Callable[S]` must accept a named function, a closure and an adopting class together, under RFC 042 and RFC 128, including in one collection.
11. A callable stored in a `Callable[S]` slot must be repeatable, and a closure whose body consumes a capture must be refused there with a diagnostic naming the capture. Consumers that call once, such as `spawn_blocking`, keep accepting it. RFC 128 decides whether a given call has the access a mutating callable needs.
12. An adapter written with a type parameter, such as `map[F with Callable[(T) -> U]]`, must store `F` by its own type, and RFC 164 must carry the captures' dependencies through its result.

### Keeping a callable

13. Each callable-typed parameter must be classified as **call-scoped**, **retained in the result** with its dependency, or **kept beyond the call**, which requires captures that own their values. The classification must be inferred from the function's body.
14. A callable-typed result must carry its RFC 164 dependency, inferred from its captures.
15. These facts must be part of RFC 128's one checked signature record, published with a library and read by consumers and `__signature__()`, exactly as RFC 129's `mut` marks are.
16. A function with no body to infer from, such as a trait method, a foreign function or an imported declaration with no published fact, must be treated as keeping the callable unless it declares `Borrowed[Callable[...]]`.
17. `Owned[Callable[...]]` must pin a parameter as kept, and `Borrowed[Callable[...]]` must pin it as not kept. A body that contradicts a pin must be refused.
18. There must be no separate keyword for keeping a callable.

### Tasks and suspension

19. A task's captures must be kept. `spawn`, `spawn_blocking` and `TaskGroup.spawn` must add their `Send` and `Static` requirements, and RFC 142's rule against borrowed scoped-task lifetimes stands.
20. RFC 153's two intervals both apply: the callback retained before a call, and an invocation suspended during one. A kept async callable called concurrently must have owned captures and repeatable calls.

### Diagnostics and inspection

21. A refusal must name the capture, the slot that keeps the closure or the place that changes the value, the later use where one exists, and a fix. It must not cite how a slot is represented.
22. Editor hover must show each capture and its mode, such as "`known`: borrowed until line 14", and the classification of a callable-typed parameter.

### Conformance workloads

| Workload | Required evidence |
| --- | --- |
| Adapters | Every RFC 088 and RFC 115 adapter accepts a capturing closure, in the same expression and when stored or returned before consumption. |
| Storage | A field, list, dict, branch value, reassignment and partial preset each accept a capturing callable, and one collection mixes functions, closures and adopters. |
| Boundaries | A kept callback across a module and a package boundary is checked from published facts, and a body change that starts keeping a callable appears in the published signature. |
| Snapshot | A copied capture keeps its value when the local changes, and a borrowed capture's local cannot change while the closure is live. |
| Refusals | Each refusal in this RFC produces its diagnostic with the capture, slot and fix. |
| Decorators | A wrapping decorator keeps the function it wraps, including on a `self` method under RFC 128. |
| Tasks and async | A kept async handler, `spawn`, `spawn_blocking` and `TaskGroup.spawn` accept owned captures and refuse borrowed ones. |
| `with` views | A closure using a view and finished inside the suite is accepted, and one that escapes it is refused. |
| Secrets | A captured secret stays protected in diagnostics and inspection. |

### Compatibility

Most of this RFC accepts programs that are refused today. One group stops compiling: a program that changes a captured value that is not copied while the closure is still live. Today it gets a hidden clone and the closure ignores the change. The diagnostic gives the two fixes: move the change, or capture `.clone()` to keep today's behavior. A program that only reads a local after building a closure is unaffected.

## Design details

### Why a refusal rather than a clone

A clone at capture time would keep today's behavior, but it copies large structures silently and hides which list a closure sees. RFC 129 says a clone that would detach intended mutations is never a legal conflict repair, and RFC 164 says no borrow conflict is repaired by an implicit clone. The refusal gives the same snapshot guarantee, because a value that cannot change needs no copy.

### Why no keyword

Swift marks a stored closure parameter `@escaping` so that an edit inside a function cannot silently change its public contract. Incan already publishes inferred facts, such as `mut` marks and duckborrowed dependencies, and a diff of the published signature shows the change. A second spelling of the same fact would have to be kept in step with the body, and `Owned[...]` already pins it for an author who wants that.

### Prior art

[Rust](https://doc.rust-lang.org/reference/types/closure.html) infers each capture's mode from what the body needs, and requires owned captures for what outlives its scope, which is the route taken here. [Swift](https://github.com/swiftlang/swift-evolution/blob/main/proposals/0103-make-noescape-default.md) makes closure parameters non-escaping by default and requires `@escaping` to store one. [Kotlin](https://kotlinlang.org/docs/inline-functions.html) distinguishes call-scoped from kept lambdas through `inline` and `noinline`, even with a garbage collector. [Python's](https://docs.python.org/3/faq/programming.html#why-do-lambdas-defined-in-a-loop-with-different-values-all-return-the-same-result) late binding makes loop lambdas all see the final value, the bug a snapshot rule avoids. [Go 1.22](https://go.dev/blog/loopvar-preview) made loop variables per-iteration to fix the same bug. Java allows only effectively final captures, which enforces a snapshot by forbidding change rather than by copying, the same shape as this RFC.

## Alternatives considered

- **Comprehensions only for capture-dependent pipelines:** workable for selection, but leaves callback registries, stored handlers and decorators with no answer.
- **A hidden clone at capture:** see "Why a refusal rather than a clone".
- **Capturing outer bindings by reference:** contradicts the snapshot behavior, and is Python's late-binding bug.
- **An `@escaping` keyword:** see "Why no keyword".
- **Different rules per slot:** today's situation, which users cannot predict.
- **Boxing every function value:** a uniform representation with unnecessary allocation. How callables are represented belongs in a design record, and a type parameter stores its callable directly.

## Drawbacks

Some programs that compile today stop compiling, and the fix asks the author to choose. A web service with many routes capturing one value has to clone it or pass it as a parameter. Inference means an edit inside a `pub` function can change what callers must supply, visible only in the published signature. Kept closures can allocate.

## Layers affected

- **Typechecker and ownership:** capture modes, retention classification, refusals and diagnostics.
- **Lowering and emission:** storing capturing callables in every slot, keeping non-capturing values cheap.
- **Standard library:** iterator adapters and other callback-taking APIs.
- **Checked metadata:** retention facts in the one signature record.
- **Language server:** capture and retention hovers.
- **Documentation:** the closures reference, replacing its table of accepted slots, and migration notes.

## Design Decisions

- **Capture modes:** the table above. Copyable values are copied, and anything else is borrowed or, when the closure outlives its scope, moved in.
- **Mutable aggregates:** changing a borrowed capture while the closure is live is refused, and `.clone()` is how an author asks for an independent snapshot. No capture is cloned implicitly.
- **Keeping a callable:** no keyword. It is inferred from the body and published in the checked signature, with `Owned[...]` and `Borrowed[...]` as optional pins.
- **Mixed collections:** `Callable[S]` in storage position holds any satisfying value, and each must own its captures and be repeatable. Representation belongs in a design record.
- **Metadata and async:** each callable-typed parameter is call-scoped, retained in the result or kept beyond the call, in RFC 128's one signature record. A task's captures are kept, with `Send` and `Static` added by the task consumer.
- **Unknown bodies:** a trait method, foreign function or undescribed import is treated as keeping the callable.
