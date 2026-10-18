# RFC 153: Async closures

- **Status:** Draft
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

Propose an explicit async arrow closure whose body may await. An async callable must remain distinguishable from an ordinary callable, preserve the shared signature model of RFC 128, and create work only when invoked rather than when the closure or its partial is constructed. The candidate spelling is `async (value) => expression`; callable type spelling, retained captures, and first-class future values remain open.

## Motivation

A short asynchronous projection currently needs a separately named async function because an ordinary closure is not an async context. Allowing await without marking that boundary would hide an effect. An explicit async closure should make the callback readable while keeping its invocation and lifetime requirements visible to users and tools.

## Goals

- Permit await inside explicitly async closures.
- Reuse ordinary parameter inference and callable binding rules.
- Support lazy async callbacks without making callback construction execute work.
- State retention, cancellation, and repeat-invocation requirements precisely.

## Non-Goals

- Making ordinary closures async by inference.
- Introducing a second family of arity-specific callable traits.
- Implicitly spawning tasks or changing RFC 142 error policies.
- Assuming every async call already produces a storable first-class future in Incan.

## Guide-level explanation

The existing sleep import is shown explicitly. The async closure and async function-type annotation are candidate syntax. Both application functions are supplied.

```incan
from std.async.time import sleep


async def delayed_double(value: int) -> int:
    await sleep(0.01)
    return value * 2


async def example() -> int:
    work: async (int) -> int = async (value) => await delayed_double(value)
    return await work(21)
```

Constructing work does not sleep. Invoking and awaiting it performs one delayed calculation and returns 42. The annotation describes the value obtained after await, not a synchronous integer-returning callback.

A generic helper could use the candidate bound `C with Callable[async (int) -> int]`, using the same Callable capability as RFC 128 with an explicitly async signature. This requires a deliberate extension of that signature grammar; it is not already promised by an ordinary `(int) -> int` type.

Existing callable presets already preserve whether their target is async. For example, `partial delayed_double(value=21)` prepares a call; it does not create or run a task. Extending local partial targets to closure-valued bindings must follow the same rule and the retained-callable contract of RFC 154, rather than silently broadening today's permitted targets.

## Reference-level explanation

- Only an explicitly async closure may contain await. A nested ordinary closure must not inherit an enclosing async context.
- Its parameters must use the same expected-signature inference and argument binding rules as ordinary closures.
- Its async nature must survive aliases, partials, generic bounds, fields, and exported checked metadata.
- Closure construction and partial construction must not invoke the body or schedule work.
- An async callback must not satisfy a synchronous result signature merely because their eventual payload types match.
- Direct invocation in an await operand must yield the declared payload through the existing async execution model.
- An async callback retained by a stream consumer must keep its required captures alive for both callback retention and each in-flight invocation.
- Cancellation must not erase required cleanup or retained failures under RFC 094 and RFC 142.
- Repeatable, mutable, consuming, and concurrently invoked callbacks must retain distinct guarantees under RFC 128; async syntax must not silently make every callback concurrently reusable.

## Design details

The current Awaitable contract distinguishes direct async calls from stored awaitable values. This RFC must not pretend that writing `task = work(21)` already creates a valid first-class future. The recommended first test drive covers direct await and the explicit runtime task-argument positions; stored invocation results need an accepted representation before they can be promised.

RFC 154 owns whether a retained closure can safely snapshot a particular capture. Async adds another lifetime: an invocation may remain suspended while its callback is used again or dropped. A safe refusal must explain the retained value and suspension boundary rather than exposing generated Rust errors. Named async functions remain a valid alternative for callbacks whose capture requirements are unclear.

## Alternatives considered

- Lift every callback into a named async function: explicit and supported, but unnecessarily verbose for local one-expression work.
- Infer async from await: hides the callable's effect at its declaration and can change typing unexpectedly.
- Add AsyncCallable0, AsyncCallable1, and similar traits: duplicates the arity problem RFC 128 removes.
- Add an unrestricted Future type as incidental syntax work: obscures a separate first-class value and ownership contract.

## Drawbacks

Async capture retention is more demanding than synchronous callback storage. A short syntax can disguise a long-lived operation unless signatures and diagnostics expose it. This draft cannot be accepted until invocation and concurrency guarantees are complete.

## Layers affected

- **Parser / signature metadata:** explicit async closure and candidate async signature forms.
- **Typechecker / ownership:** await context, capture retention, and invocation guarantees.
- **IR / runtime integration:** suspended callback execution without implicit task creation.
- **Formatter / LSP:** signatures, diagnostics, and async boundary display.

## Unresolved questions

- Is `async (value) => expression` the final closure spelling, and what block-body form is supported?
- Should async signatures use `async (A) -> R`, or another spelling within RFC 128's shared model?
- Which direct-call and runtime task-argument positions accept async closure invocation?
- What represents a stored invocation result, if this RFC supports one at all?
- When may a returned future retain or borrow the callback, and when may another invocation begin?
- What additional guarantees are required for concurrent execution under RFC 142?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
