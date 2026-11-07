# RFC 153: Async closures

- **Status:** Planned
- **Created:** 2026-10-17
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 039 (awaitable concurrency)
    - RFC 094 (scoped cleanup)
    - RFC 128 (unified callable signatures)
    - RFC 129 (ownership and bindings)
    - RFC 142 (async streams and cancellation)
    - RFC 154 (retained capturing callables)
- **Issue:** [#1176](https://github.com/encero-systems/incan/issues/1176)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Add explicit `async (value) => expression` closures whose bodies may await, with `Callable[async (A) -> R]` annotations in RFC 128's shared callable model. R is the awaited payload. Construction and partial construction do not invoke or schedule work. Direct await and checked runtime task-argument positions accept invocation; retained captures and suspended calls follow ordinary ownership and consumer-specific concurrency requirements. This RFC does not introduce a general stored future value.

## Motivation

Short asynchronous projections otherwise need separately named functions. Inferring async from an await hidden inside an ordinary closure would obscure the effect. An explicit closure marker and expected-signature inference make the effect visible without repeating annotations at every call site.

## Goals

- Permit await inside explicitly async expression closures.
- Reuse ordinary expected-type inference and callable binding rules.
- Preserve async signatures through checked aliases, generic bounds, partials and supported retained slots.
- State capture retention, suspension and concurrent invocation requirements precisely.

## Non-Goals

- Making ordinary closures async by inference.
- Adding block-body closure syntax independently of the existing closure grammar.
- Introducing arity-specific callable traits or unrestricted first-class future values.
- Implicitly spawning tasks or changing RFC 142's cancellation and error policies.

## Guide-level explanation

All async closure and signature examples below describe proposed behavior. The imports identify existing async syntax and sleep support; every application helper is supplied.

### Keep the API type explicit and the call site short

```incan
import std.async
from std.traits.callable import Callable
from std.async.time import sleep

type AsyncTransform = Callable[async (int) -> int]

async def delayed_double(value: int) -> int:
    await sleep(0.01)
    return value * 2

async def apply(value: int, operation: AsyncTransform) -> int:
    return await operation(value)

async def example() -> int:
    return await apply(21, async (value) => await delayed_double(value))
```

The expected AsyncTransform type supplies value's integer parameter and awaited result type. The callback still says async because its body may suspend. The containing function says async because it awaits; neither marker substitutes for the other. The result is 42.

A callback reused across calls may be bound locally:

```incan
import std.async
from std.traits.callable import Callable
from std.async.time import sleep

async def delayed_double(value: int) -> int:
    await sleep(0.01)
    return value * 2

async def example() -> int:
    work: Callable[async (int) -> int] = async (value) => await delayed_double(value)
    first = await work(10)
    second = await work(11)
    return first + second
```

Constructing work does not sleep or schedule a task. Each awaited invocation performs one operation. A longer callback can remain a named async function; this RFC adds the expression form rather than a separate block-closure grammar.

### Generic helpers retain the same signature model

```incan
import std.async
from std.traits.callable import Callable

async def apply_twice[C with Callable[async (int) -> int]](
    value: int,
    operation: C,
) -> int:
    first = await operation(value)
    return await operation(first)

async def increment(value: int) -> int:
    return value + 1

async def example() -> int:
    return await apply_twice(40, async (value) => await increment(value))
```

The bound extends RFC 128's shared signature grammar with async. It does not create a second callable trait family. This helper invokes sequentially; it must not imply that its callback can also run concurrently.

### Explicit task creation is a separate operation

```incan
import std.async
from std.traits.callable import Callable
from std.async.task import spawn
from std.async.time import sleep

async def delayed_double(value: int) -> int:
    await sleep(0.01)
    return value * 2

async def example() -> None:
    work: Callable[async (int) -> int] = async (value) => await delayed_double(value)
    handle = spawn(work(21))
    # The explicit spawn operation schedules the invocation.
    match await handle:
        Ok(value) => println(value)
        Err(failure) => println(failure.message())
```

The task argument uses the same checked async metadata as a named async call. The existing spawn operation imposes its runtime transfer and lifetime requirements. Merely being async does not satisfy them. This example captures no local borrowed resource; a callback retaining one may fail those checks even when direct sequential await is valid.

Existing named callable presets such as `partial delayed_double(value=21)` preserve async metadata and prepare a call without invoking it. Support for closure-valued partial targets must follow RFC 154's checked retained-callable contract; this RFC does not silently broaden today's permitted targets.

## Reference-level explanation

- The closure spelling must be `async (PARAMETERS) => EXPRESSION`. Only explicitly async closures may contain await. A nested ordinary closure must not inherit an enclosing async context.
- Authored async callable-signature annotations must use `Callable[async (A) -> R]` or an alias under RFC 128. A bare async arrow is only a signature component, including inside named type operations under RFC 141.
- Parameters and results must use ordinary expected-signature inference and argument binding. The signature `async (A) -> R` describes an awaited R, not a synchronous R-returning callable.
- Async signatures must participate in RFC 128's common callable model, including `Callable[async (A) -> R]`. Async metadata must survive supported aliases, partials, generic bounds, retained fields and checked package exports.
- Closure construction and partial construction must not invoke the body or schedule work. Task creation must remain explicit.
- An async callback must not satisfy a synchronous signature merely because the eventual payload types match.
- Direct invocation in an await operand and explicit checked runtime task-argument positions must follow the same async-call contract as named async functions. Task consumers must enforce their actual transfer and lifetime bounds.
- A bare invocation result must not become a promised general storable future value under this RFC. Existing independently supported stored awaitables retain their own contracts.
- Ordinary duckborrowing and checked capture facts must keep a retained callback, its receiver and its required captures alive throughout retention and every suspended invocation. No authored lifetime syntax, hidden clone, or blanket borrowing ban is added.
- An overlapping invocation or competing access must be accepted only when the callback's access requirements permit it. Otherwise diagnostics must identify the conflicting use and retained value.
- Repeatability, mutable or consuming receiver access, capture lifetime and transfer requirements must be checked at the actual consumer. Sequential await does not imply permission for spawning or concurrent reuse.
- Cancellation must retain required cleanup and failures under RFC 094 and RFC 142. Async closure syntax must not silently grant another capability or weaken those obligations.

## Design details

There are two distinct retention intervals: the callback can be retained before invocation, and a particular invocation can remain suspended. RFC 154 governs ordinary capture storage and snapshots; this RFC requires the ownership proof to cover both intervals. Backend representation must preserve these source rules, rather than expose generated type mismatches or insert cloning to satisfy a runtime task API.

A named async function and an async closure with equivalent checked facts must have the same accepted invocation positions. The explicit task consumer determines scheduling and its required capabilities. This keeps a sequential helper usable without forcing every callback to meet spawned-task requirements.

## Alternatives considered

- Lift every callback into a named async function: clear and still useful for substantial work, but verbose for one-expression callbacks.
- Infer async from await: hides the effect boundary and can change a nested callable's type unexpectedly.
- Add AsyncCallable0, AsyncCallable1 and similar traits: duplicates the arity families RFC 128 removes.
- Add unrestricted future storage incidentally: bundles a separate value and ownership contract into closure syntax.

## Drawbacks

Retained async callbacks require ownership checks over suspension as well as storage. A short syntax can disguise a long-lived operation; diagnostics and signatures must expose the relevant boundaries. Some callbacks valid for sequential await will correctly be refused by a spawning or concurrent consumer.

## Layers affected

- **Parser / signature metadata:** explicit async expression closures and shared async signature forms.
- **Typechecker / ownership:** expected typing, await context, capture retention, suspended calls and consumer-specific capabilities.
- **IR / runtime integration:** named-call-equivalent invocation positions and explicit scheduling.
- **Formatter / LSP:** readable signatures and source-level ownership and async diagnostics.

## Design Decisions

- Use explicit async expression closures and `Callable[async (A) -> R]` annotations, with expected typing reducing call-site repetition. Substantial bodies can use named functions; no new block grammar is introduced here.
- Accept direct await and explicit checked runtime task arguments. Constructing a closure or partial never invokes or schedules it.
- Do not introduce general stored invocation results incidentally; retain existing stored-awaitable contracts separately.
- Use ordinary duckborrowing and checked capture retention across suspension, without implicit cloning or authored lifetime parameters.
- Check concurrency and transfer requirements at the actual consumer. Async alone grants no concurrent reuse or spawning capability.
