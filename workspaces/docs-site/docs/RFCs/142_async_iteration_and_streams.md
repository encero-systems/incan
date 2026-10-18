# RFC 142: Async composition, iteration, and streams

- **Status:** Draft
- **Created:** 2026-10-11
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 006 (Python-style generators)
    - RFC 039 (race and awaitable concurrency)
    - RFC 066 (`std.http` and explicit retry policy)
    - RFC 068 (protocol hooks for core language syntax)
    - RFC 070 (`Result` combinators)
    - RFC 088 (iterator adapter surface)
    - RFC 094 (context managers and scoped cleanup)
    - RFC 098 (implementation-chosen types in trait headers)
    - RFC 115 (fallible iteration and combinators)
    - RFC 128 (shared callable signatures and bound methods)
    - RFC 129 (bindings, borrowing, and ownership)
    - RFC 131 (first-class typed errors)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Add Python-familiar `async for` loops, lazy async mapping, and async consumers for individual values and batches. Sources await advancement; consumers await application callbacks without implicitly spawning tasks. A fallible source uses `async for item in source?:`, extending RFC 115's explicit error propagation. Separately, streams of `Result` items support independent `ErrorPolicy` choices for item failures and callback failures: stop, report and continue, or ignore and continue. Terminal outcomes preserve progress and pending successful values when processing stops. Native async protocols use RFC 098's proposed `infer` headers. The syntax and APIs in this document are proposals, not implemented functionality.

The broader composition scope includes `join` and its error-policy direction, plus an explicit Draft inventory of task ownership, cooperative cancellation, bounded concurrency, notifications, timers, and test-clock gaps. Plain joining and sequential consumption have concrete contracts here; the remaining candidate surfaces require further test drives and must not be mistaken for settled APIs.

## Core model

1. **Waiting belongs to advancement:** `async for` awaits each next-item operation without requiring the caller to write a manual polling loop.
2. **One item at a time:** the next advancement begins only after the current loop body completes normally or continues.
3. **Exhaustion is ordinary:** an infallible source returns `None` when finished; a fallible source returns `Ok(None)`.
4. **Errors remain explicit:** a fallible loop carries `?` and propagates a failed advancement through its surrounding `Result` contract.
5. **Sources own their policies:** a pagination source chooses when to fetch pages; a channel source chooses buffering; a recoverable source owns retry policy.
6. **Stopping and cleanup are distinct:** leaving the loop stops further advancement. Asynchronous resource shutdown requires a separate explicit contract.
7. **Composition remains lazy:** constructing a mapping pipeline does not advance its source or invoke its callback. Consumption drives work; collection explicitly requests materialization.
8. **Policies have separate owners:** an item error and a consumer callback error can use different policies. Neither grants permission to retry a failed source advancement.
9. **Progress survives failure:** stopping does not discard previously delivered values or a pending batch. A partially processed batch is not automatically committed, retried, or rolled back.
10. **Composition and task ownership are distinct:** joining concurrent operations does not implicitly spawn tasks. Owning spawned tasks, cancelling them, and awaiting shutdown require explicit contracts.

## Motivation

An ordinary iterator can produce its next item immediately. A paginated API, incoming channel, or event subscription may need to wait for data first. Without async iteration, callers must repeat the same loop scaffolding: await the next operation, distinguish an item from exhaustion, handle a possible error, process the item, and repeat.

The desired source-level experience is simple: consume users, rows, or events with a loop even when obtaining the next value requires waiting. The source can hide page boundaries while preserving its typed failure contract and fetching data as the consumer requests it.

RFC 006 defines synchronous generators, RFC 088 defines synchronous iterator adapters, and RFC 115 defines fallible synchronous iteration. Each leaves async iteration outside its scope. This RFC adds the asynchronous consumption model and the protocols needed to author sources in Incan, while preserving the distinction between iteration, error handling, concurrency, and resource lifetime.

The same source should support incremental processing without an application first building a list of all successes and failures. Parsing is an ordinary transformation, rather than a special row-processing framework. A user may want to stop on malformed messages but continue after a failed delivery, or report every malformed message while stopping if the destination fails. Those choices should be visible at the consumer without changing the pipeline into a policy-dependent union of containers.

## Goals

- Add `async for` inside async functions and methods.
- Define statically checked async iterable, iterator, and fallible iterator protocols.
- Reuse RFC 115's explicit loop-header `?` convention.
- Make sequential advancement, exhaustion, early exit, and cancellation behavior explicit.
- Allow Incan-authored sources without requiring a custom native backend for each source.
- Preserve ordinary iteration, existing async syntax activation, and existing `Result` semantics.
- Provide small pagination and source-authoring examples before more advanced composition.
- Define lazy `map` and async `for_each`, `for_each_batch`, and explicit `collect` consumers.
- Distinguish source advancement failures, yielded `Result` item failures, and consumer callback failures.
- Allow independent FailFast, CollectAll, and Ignore policies for item and callback failures, including runtime-selected policies.
- Preserve pending values and typed failure information when a consumer stops.
- Add plain heterogeneous `join` and capture the error-policy requirements for concurrent composition.
- Capture the wider async runtime gaps and their design boundaries without presenting unexplored API spellings as settled.

## Non-Goals

- Automatically fetching pages, retrying failures, or selecting a pagination framework.
- Implicit prefetching, spawning tasks, processing items concurrently, or making blocking operations asynchronous.
- Claiming that every source is lazy, bounded in memory, or safe to retry after cancellation.
- Defining async generator functions, async comprehensions, or the complete synchronous adapter family for async sources.
- Finalizing task-group, cancellation-token, concurrent mapping, notification, timer, and test-clock APIs before their remaining contracts are explored.
- Promising transactional batch delivery, automatic retries, checkpoints, or exactly-once external effects.
- Adding exception-based exhaustion or changing typed errors into exceptions.
- Automatically awaiting a resource's shutdown when a loop ends.
- Redesigning `Awaitable`, callable signatures, or task scheduling.

## Guide-level explanation

### Await independent operations together

Plain `join` is proposed composition functionality. The example defines both operations; `sleep` follows the existing time API. The standard module that will expose `join` remains an open question.

```incan
import std.async
from std.async.time import sleep

async def fetch_number() -> int:
    await sleep(0.01)
    return 42

async def fetch_label() -> str:
    await sleep(0.02)
    return "ready"

async def show_pair() -> None:
    number, label = await join(fetch_number(), fetch_label())
    println(f"{label}: {number}")
```

Both operations can make progress while either awaits. The result is a tuple in argument order with the distinct types `int` and `str`; the operations need not return a common element type. No task is implicitly spawned, and concurrency is not a promise of parallel execution. If branches return `Result`, plain join waits for both and returns both results unchanged.

An error-aware form reuses ErrorPolicy rather than making `failfast` a separate joining function. FailFast returns immediately on the first observed branch failure while preserving completed successes; CollectAll awaits and exposes all branch outcomes; Ignore awaits the remaining branches and discards application error payloads. The outcome retains one independently typed position per operation in argument order, distinguishing completed success, retained failure, ignored failure, and unfinished work. Policy selection, including a runtime enum value, must not change this form's static outcome type. Exact call options, generic declarations, and reporting remain open.

For example, suppose three operations would normally return `(42, "Alice", true)`. If the second fails while the third is still running, a FailFast outcome retains the following information:

```text
(Succeeded(42), Failed(original_error), Unfinished)
```

These labels illustrate states, not settled enum or constructor names. The successful first value remains available and the third position is not misreported as a failure. Under Ignore, a failed position is explicitly marked ignored without retaining its error payload; it is not removed or allowed to shift the remaining positions. An unfinished position means join did not obtain a completed result; it does not prove that underlying work has stopped, is resumable, or performed no effects. On FailFast, join drops directly owned pending operations. A previously spawned task retains the existing handle-detach contract and continues independently unless explicit task ownership supplies a cancellation policy. Task-group ownership is the proposed surface for that separate requirement.

### Own spawned work with a task group

The proposed TaskGroup owns its member tasks. If a pipeline returns early and drops its group, the group requests cancellation of its remaining async tasks through the existing task-abort mechanism. Tasks must be explicitly detached to continue independently after group destruction. This does not change the existing standalone JoinHandle's detach-on-drop contract.

Task outcomes are delivered in observed completion order, with each outcome identifying its task. For example, if three uploads are started as Slow, Fast, and Medium but finish as Fast, Medium, and Slow, the caller can receive them in that latter order. A completed upload does not wait behind a slower earlier submission. Task identity keeps each result attributable without treating its delivery position as its identity. Authors needing submission order may explicitly arrange identified results afterward.

Requesting cancellation is separate from observing termination. The proposed `await group.shutdown()` operation requests cancellation and waits for the remaining owned tasks to terminate. Ordinary dropping cannot await that work. Immediate FailFast reporting therefore remains compatible with a later explicit awaited shutdown. This is task termination, not a promise of transaction rollback, reversible effects, or arbitrary asynchronous application cleanup.

A Python-style asynchronous scope may later make awaited exit convenient. That scope design belongs with RFC 094; this ownership decision does not claim that async context-manager syntax or cleanup guarantees are already available.

### Link stop requests from parents to children

Consider an explicitly configured cancellation hierarchy for a backup application:

```text
Application
└── Backup job                 ← parent of both uploads
    ├── Upload photos          ← child
    └── Upload documents       ← child
```

Requesting cancellation of the backup job signals both uploads. Requesting cancellation of only the photos upload leaves the backup job and documents upload running. Requesting cancellation of the application signals the backup job and both uploads. Requests flow from parent to linked descendants, without automatically traveling upward or across to siblings.

These are explicit cancellation relationships, not relationships inferred merely because one function called another or spawned a task. A separate failure policy may decide that a failed upload should stop the backup job. A cooperative stop request still does not establish that a worker has terminated or reverse its completed effects.

### Read a sequence that may need to wait

```incan
import std.async

async def print_users(client: Client) -> None:
    async for user in client.users():
        println(user.name)
```

This illustrative `Client.users()` API returns an async source of `User` values. The example initially assumes an infallible source so the loop's basic behavior is visible. `Client`, `User`, and the method are application-defined examples, not new stdlib APIs.

The loop requests a user, waits if necessary, prints the name, then requests another user. It does not print multiple users concurrently. Other async work may progress while an asynchronous operation is pending; a blocking implementation still blocks.

### Keep pagination inside the source

For a paginated client, `users()` could keep a page buffer and a continuation cursor. It fetches a page when it needs one, yields that page's users individually, and fetches another page only when the buffer is empty and a next item is requested.

The caller sees users rather than pages. `async for` supplies the waiting loop; the client supplies pagination. A different API could expose whole pages instead, using the same loop syntax.

This example's source is deliberately lazy. Async iteration does not force every implementation to use that policy: an API that buffers or prefetches must document its own behavior.

### Make fetch errors visible

A realistic network source can fail while advancing. Assume this version of `client.users()` returns a fallible async source with error type `FetchError`:

```incan
import std.async

async def print_users(client: Client) -> Result[None, FetchError]:
    async for user in client.users()?:
        println(user.name)

    return Ok(None)
```

`async for` means fetching the next user may require waiting. The trailing `?` means a failed fetch returns an error from `print_users`. Exhaustion ends the loop normally.

If the third page fails, names printed from earlier pages stay printed. Error propagation does not undo completed work. The loop does not retry the failed request automatically.

### Stop requesting items

```incan
import std.async

async def first_active(client: Client) -> Result[Option[User], FetchError]:
    async for user in client.users()?:
        if user.active:
            return Ok(Some(user))

    return Ok(None)
```

For the lazy paginated source described above, finding a user prevents the next page request. The current page may already contain more users; the source fetches in pages even though the loop consumes individual users. `break` likewise stops further advancement.

### Author a source in Incan

A small source can implement the protocol directly. This counter waits before yielding each value:

```incan
from std.async.time import sleep

class DelayedCounter with AsyncIterator[int]:
    current: int
    end: int

    async def __anext__(mut self) -> Option[int]:
        if self.current >= self.end:
            return None

        await sleep(0.1)
        value = self.current
        self.current += 1
        return Some(value)

async def main() -> None:
    async for number in DelayedCounter(current=0, end=3):
        println(number)
```

The proposed async protocol is new. The `sleep` operation follows the existing `std.async.time` API. No async generator or background task is needed for this source. Once the counter reaches its end, the loop finishes without an additional sleep.

A fallible source instead adopts `AsyncFallibleIterator[ItemType, ErrorType]` and returns `Result[Option[ItemType], ErrorType]` from its async `__anext__` method. The item, exhaustion, and failure states are the same as RFC 115, with an awaitable advancement operation.

### Separate setup failure from advancement failure

An application might need asynchronous acquisition before it can return a stream:

```incan
import std.async

async def print_events(client: Client) -> Result[None, FeedError]:
    events = (await client.open_events())?

    async for event in events?:
        println(event)

    return Ok(None)
```

This hypothetical API uses `FeedError` for both acquisition and advancement. The first `?` handles acquisition; the loop-header `?` handles subsequent fetches. Acquiring the stream does not make its later reads infallible.

### Build a lazy pipeline with an async source and destination

This example shows the application definitions rather than assuming a `read_lines`, `parse_rows`, or `save_batch` helper. The caller supplies connected channels. Receiving an input and submitting a batch can both suspend; parsing one JSON message is synchronous.

```incan
import std.async
from std.async.channel import Receiver, Sender, SendError
from std.json import JsonValue, JsonError

# Proposed library enum, shown here to make the vocabulary explicit.
enum ErrorPolicy:
    FailFast
    CollectAll
    Ignore

class IncomingMessages with AsyncIterator[str]:
    receiver: Receiver[str]

    async def __anext__(mut self) -> Option[str]:
        return await self.receiver.recv()

def parse_message(text: str) -> Result[JsonValue, JsonError]:
    return JsonValue.parse(text)

class BatchQueue:
    sender: Sender[list[JsonValue]]

    async def submit_batch(
        self,
        batch: list[JsonValue],
    ) -> Result[None, SendError[list[JsonValue]]]:
        return await self.sender.send(batch)

async def process(
    incoming: Receiver[str],
    outgoing: Sender[list[JsonValue]],
) -> None:
    source = IncomingMessages(receiver=incoming)
    destination = BatchQueue(sender=outgoing)

    results = source.map(parse_message)

    outcome = await results.for_each_batch(
        destination.submit_batch,
        size=500,
        item_errors=ErrorPolicy.FailFast,
        batch_errors=ErrorPolicy.Ignore,
    )

    println(outcome)
```

`Receiver.recv()`, `Sender.send()`, and the JSON operations follow existing stdlib APIs. `IncomingMessages`, `BatchQueue`, `parse_message`, and `process` belong to the application. `AsyncIterator`, its async-source `map`, and the policy-aware consumer are proposed library functionality. Their exact module exposure is still an open question; this example does not invent importable modules for them.

`map` is provided by the async iterator protocol, analogous to RFC 088's synchronous iterator method. It constructs an adapter without reading a message. Its synchronous callback returns a `Result`, so the adapter's item type is `Result[JsonValue, JsonError]`. Mapping does not itself choose what a parsing error means.

The terminal consumer interprets those result items, batches successful values, and awaits `submit_batch`. It requests no further input while that callback is pending. A malformed message stops this example; a failed submission is ignored and processing continues with the next batch. Successful submission means queued downstream, not persisted. Replacing the parser with image decoding or message validation does not change the iteration model.

### Choose policies independently and report incrementally

The enum has the same meaning at either processing boundary:

| Policy       | Failed item or callback                                                         |
| ------------ | ------------------------------------------------------------------------------- |
| `FailFast`   | Stop consumption and return the typed failure with progress and pending values. |
| `CollectAll` | Deliver the failure to the corresponding reporting callback, then continue.     |
| `Ignore`     | Discard the failure and continue without invoking a reporting callback.         |

Reporting is incremental. CollectAll does not silently build an unbounded list of errors in the terminal summary. An application can write diagnostics, quarantine failed inputs, or accumulate errors explicitly in its reporter. A bare `JsonError` does not contain the original message; an application that needs that message must preserve it in its transformation's error payload.

For the definitions above, a reporting variant can use these application-owned callbacks:

```incan
def report_parse_failure(error: JsonError) -> None:
    println(error.detail())

def report_delivery_failure(
    batch: list[JsonValue],
    error: SendError[list[JsonValue]],
) -> None:
    println(f"Could not submit {len(batch)} values: {error.message()}")

async def process_with_policy(
    incoming: Receiver[str],
    outgoing: Sender[list[JsonValue]],
    policy: ErrorPolicy,
) -> None:
    source = IncomingMessages(receiver=incoming)
    destination = BatchQueue(sender=outgoing)

    outcome = await source.map(parse_message).for_each_batch(
        destination.submit_batch,
        size=500,
        item_errors=policy,
        batch_errors=policy,
        report_item_error=report_parse_failure,
        report_batch_error=report_delivery_failure,
    )

    println(outcome)
```

The same consumer and terminal outcome work for a runtime policy. Reporters are required when a policy can select CollectAll. They observe the error under FailFast and CollectAll; Ignore skips them. The batch reporter receives the attempted batch as well as its callback error. Reporting callbacks in this first surface return `None`; they may be synchronous or async, with the async form awaited before continuing.

### Preserve a pending batch when stopping

Consider a batch size of three and inputs A, B, a malformed message, C, and D. With item FailFast, A and B have parsed successfully but have not reached the destination. The candidate outcome spelling is:

```incan
match outcome:
    Consumption.Completed(summary) =>
        println(f"Delivered {summary.processed} values")
    Consumption.Stopped(summary, failure, pending) =>
        println(f"Delivered {summary.processed} values before stopping")
        println(f"Retained {len(pending)} pending values")
        println(failure)
```

The stopped outcome retains A and B in order. It reports the item failure and does not read C or D. It does not automatically submit the partial batch. The caller decides whether to deliver, inspect, or discard it.

Under CollectAll or Ignore, the malformed item does not count toward the batch size. The first delivery contains A, B, and C; ordinary exhaustion delivers the final partial batch containing D. A callback failure under FailFast instead exposes the attempted batch. Those values may already have had external effects: retaining them is not a claim that replay is safe.

`for_each` uses the same model for one successful value at a time, with `callback_errors` and `report_callback_error` in place of the batch options. `collect` explicitly materializes values and must retain the partial collection if consumption stops. Exact generic result declarations and collection reporting details remain Draft questions; these outcome names are candidate API spelling, not implemented types.

## Reference-level explanation

### Plain concurrent join

Plain `join` must accept a fixed argument list of async operations with independently checked output types and return a tuple of those outputs in argument order. It must await every branch, including branches returning `Err`, without flattening or converting their results. It must not require a homogeneous output type, implicitly spawn tasks, or claim parallel execution. Call arguments must follow ordinary evaluation order; polling and completion order must not alter tuple positions.

Joining must own its local branch operations until they finish or the composition is cancelled. Dropping the join operation must drop its locally owned pending branches according to their individual cancellation contracts. Awaiting a previously spawned task handle is different: dropping that handle retains the existing detach behavior and must not be described as cancellation of the spawned task.

The policy-aware form must retain one independently typed outcome position for each input operation in argument order. It must preserve completed success values on early failure and distinguish success, retained application failure, explicitly ignored application failure, and unfinished work. Ignore must discard the application error payload without removing or shifting its position. Policy values, including runtime-selected values, must share the same static outcome type within this form; this does not change plain join's tuple of unchanged outputs.

FailFast must return immediately on the first observed branch failure, rather than waiting to drain unfinished branches or selecting failure by lowest argument index. Task cancellation and panic must remain distinct from ordinary application failure. On FailFast, join must drop its directly owned pending branches under their existing cancellation contracts and return without awaiting their shutdown. A previously spawned task must retain its existing handle-detach behavior; dropping the wait or handle must not be represented as cancellation of that independently running task. Cancellation of such tasks requires an explicit owner, such as the proposed task group, and its separate contract. An unfinished position must mean that join did not obtain a completed result, not that underlying work is proven stopped. Neither an unfinished position nor retained success may promise reversal of external effects, resumability, or safe replay. Exact generic result declarations, call options, and reporting remain Draft API work.

### Task-group ownership and shutdown

A task group must own its members until their completion or explicit detachment. Dropping the group must request cancellation of remaining owned async tasks through the existing task-abort mechanism, without awaiting termination. Dropping the group must not silently detach its remaining members. Explicitly detached tasks must cease to be group members and retain their independent task lifecycle.

Task-group consumption must deliver available task outcomes in observed completion order rather than withhold an outcome behind an earlier unfinished submission. Every delivered outcome must retain its associated task identity, including failure outcomes, so attribution does not depend on position. The contract must not promise a deterministic tie order among concurrently ready tasks or infer precise wall-clock completion order beyond observed evidence. Submission-order reconstruction is an explicit consumer concern. This must not change fixed join's argument-order tuple contract.

The proposed async `shutdown()` operation must request cancellation of remaining members and wait for their termination. Normal completion of that operation must establish that no remaining owned task is running. A cancelled or interrupted shutdown must not claim that this milestone was reached; ownership and drop behavior must remain applicable. Exact shutdown outcome and failure-reporting declarations remain open and must not silently suppress failures by copying a backend convenience method's behavior.

This contract must not change standalone JoinHandle behavior, turn cancellation requests into proof of termination, promise that running blocking work is interruptible, or imply borrowed scoped-task lifetimes. General asynchronous scope exit and application resource-cleanup guarantees remain RFC 094 concerns. Immediate FailFast reporting must not acquire an implicit awaited-shutdown delay through this ownership rule.

### Cooperative cancellation hierarchy

Explicitly linked cancellation parents and children must support downward propagation: requesting cancellation of a parent must signal its linked descendants. Requesting cancellation of a child must not by itself signal its parent or siblings. Task spawning, function calls, or sharing an execution runtime must not implicitly establish those links.

An application failure and an explicit cancellation request must remain distinct. ErrorPolicy may decide whether a failure stops related work; the cancellation hierarchy must not infer such a failure policy from its topology. Cooperative cancellation must remain a request rather than proof of termination, an implicit task-abort operation, or rollback of completed effects. Exact token names, creation and observation methods, and integration with task groups, race, and deadlines remain open.

### Syntax and activation

The statement forms must be `async for pattern in expression:` and `async for pattern in expression?:`, using the ordinary loop body indentation and binding-pattern rules. The existing `std.async` import activation must govern the async syntax. The statement must be rejected outside an async function or method.

The source expression must be evaluated once on entry. The loop must not implicitly await that expression, acquire a resource, or invoke an async factory on the caller's behalf. Asynchronous acquisition must remain explicit, as in the separate local in the guide.

Ordinary `for` must retain its current protocol resolution. It must not automatically consume an async source. Conversely, `async for` must not silently wrap a synchronous iterator; a library may provide an explicit adapter.

### Protocol declarations

The proposed native protocols use RFC 098's implementation-chosen members:

```incan
pub trait AsyncIterable[infer Item]:
    def __aiter__(self) -> AsyncIterator[Self.Item]: ...

pub trait AsyncIterator[infer Item]:
    def __aiter__(self) -> Self:
        return self

    async def __anext__(mut self) -> Option[Self.Item]: ...

pub trait AsyncFallibleIterator[infer Item, infer Error]:
    def __aiter__(self) -> Self:
        return self

    async def __anext__(mut self) -> Result[Option[Self.Item], Self.Error]: ...
```

These declarations depend on the revised RFC 098 contract; they must not be interpreted as ordinary generic traits whose item or error arguments permit competing implementations. Concrete adoptions must supply their inferred choices. An async source must expose one selected item contract, or the compiler must diagnose ambiguity rather than pick an implementation.

The `__aiter__` operation must be synchronous. On an iterable it must supply a checked async iterator; on a single-pass iterator it must return the same iteration state. Returning a concrete implementation through an async iterator protocol must preserve its checked conformance, following the existing synchronous iterable model; this must not introduce unchecked type erasure or implicit dynamic trait-object dispatch.

An infallible advancement must produce `Some(item)` or `None`. A fallible advancement must produce `Ok(Some(item))`, `Ok(None)`, or `Err(error)`. `None` must represent exhaustion, not an instruction to wait and poll again. A source that has no item yet must keep its advancement pending until it can produce one of its declared outcomes.

The loop must await the selected async method directly. It must not store the call as a first-class future, require that the iterator itself adopt `Awaitable`, or change the existing distinction between direct async calls and awaitable values.

### Loop-header resolution and errors

An unmarked `async for` must accept a checked `AsyncIterable` or `AsyncIterator`. It must reject a source requiring fallible advancement. Ambiguous protocol selection must produce a diagnostic rather than fall back to synchronous iteration or discard a failure channel.

For `async for pattern in expression?:`, resolution must mirror RFC 115:

1. A `Result[I, E]` whose successful value supports infallible async iteration must have its setup result unwrapped once, then use infallible async advancement.
2. Otherwise, an `AsyncFallibleIterator` must use per-advancement propagation.
3. A setup `Result` containing a fallible async source must be rejected in this combined header, with guidance to unwrap setup into a local and use a separate fallible loop.

A `?` header on an ordinary non-`Result`, infallible async source must be rejected. The surrounding function's error type must be compatible with the propagated type under the existing `?` contract. The header must not invent an implicit error conversion or require a particular error representation. RFC 131 error values may participate under their own conversion and propagation rules.

A fetch failure must stop the loop and propagate immediately, without invoking the body for that failed advancement or polling again. Earlier body effects must remain intact. A body-level `?` must retain its ordinary meaning and must not be confused with a source failure or cause another source poll.

### Advancement, order, and exhaustion

The loop must acquire its iterator once and request the first item only when execution reaches the loop. Each advancement must complete before the body starts. The next advancement must begin only after normal body completion or a `continue` targeting that loop. It must not overlap another advancement on the same iterator state or run the body concurrently on the caller's behalf.

Source order must be preserved. The loop must not buffer extra items, prefetch, spawn producer tasks, clone its cursor, or rewind the iterator. Source implementations may have documented internal buffers or background work; the loop's sequential behavior must not be represented as a guarantee about all underlying activity.

On exhaustion, the loop must stop without another advancement. A fresh iterable may supply a fresh iterator on a later loop; an existing single-pass iterator must not be reset by iteration. Manual polling after exhaustion or failure must follow the source's documented contract, not an inferred retry policy.

For a demand-driven source, body processing naturally limits how often the loop requests another item. This must not be advertised as a global bounded-memory or transport-level backpressure guarantee for buffered or independently producing sources.

### Lazy async mapping

`AsyncIterator` must provide `map` for synchronous callbacks. A callback with input `Self.Item` and output `U` must produce an async iterator whose item type is `U`. `AsyncFallibleIterator` must provide the corresponding mapping operation while preserving its source error type. This first surface must not automatically await a callback result; async transformation callbacks require a separately specified operation or overload.

Constructing a mapping adapter must not advance its source or invoke the callback. Each adapter advancement must await at most one source advancement and invoke the callback exactly once for each yielded item. It must forward exhaustion and source failures without invoking the callback. It must preserve order and must not spawn tasks, prefetch another item, or change a `Result` callback output into a source advancement failure.

In particular, mapping an infallible async source with `(T) -> Result[U, E]` must produce result items, not an `AsyncFallibleIterator[U, E]`. If the source itself is fallible with error `S`, its mapped advancement result must retain the distinct structure `Result[Option[Result[U, E]], S]`. The outer error describes failure to advance; the inner error is an ordinary yielded item that a consumer can handle and continue past.

Mapping callbacks must use the shared callable checking model, including receiver access and repeated invocation requirements. An adapter must not accept a consuming callable as repeatable merely because its parameter and return types match.

### Async consumers and their policies

`AsyncIterator` and `AsyncFallibleIterator` must expose async `for_each`, `for_each_batch`, and `collect` consumers. Ordinary items are successful inputs. A policy-aware consumer for items of type `Result[T, E]` must distinguish `Ok(value)` from `Err(error)` explicitly. The result-item form must select a statically checked overload; this must not be dynamic shape inspection or an implicit recursive unwrapping of nested results. A nested `Result` in `T` remains a value.

`for_each` must invoke its application callback once for each successful value. `for_each_batch` must invoke it with ordered lists of successful values. Each consumer must accept a synchronous callback returning `None` or `Result[None, F]`, or an async callback with the same source-level return contract. Infallible callbacks have no callback-failure policy to apply. Async callbacks must be awaited once; synchronous callbacks must run directly and must not become nonblocking merely because the consumer is async.

The compiler must check callback async status, receiver access across suspension, error type, and repeatable invocation. A bound method must retain its bound receiver and ownership requirements under RFC 128 and RFC 129. Supporting async callback values is an explicit part of this proposal; its callable metadata and overload representation must be resolved before acceptance. The consumer must not rely on runtime guessing or await an arbitrary synchronous return value.

The working option names must distinguish these boundaries:

- `item_errors` and `report_item_error` apply to yielded `Err(E)` items.
- `callback_errors` and `report_callback_error` apply to a failed `for_each` callback.
- `batch_errors` and `report_batch_error` apply to a failed `for_each_batch` callback.
- `collect` has an item policy and reporter, but no application delivery callback.

An omitted policy must default to FailFast. Policy options that have no corresponding failure channel must be rejected rather than silently affecting some other stage. Policies may be chosen independently and may be runtime enum values. Selecting a runtime policy must not change a consumer's static outcome type or require a policy-dependent union of materialized containers.

FailFast must stop on the first failure observed by sequential consumption. If a corresponding reporter was supplied, it must run before the stopped outcome is returned. CollectAll must report every failure at that boundary and continue after reporting completes. Ignore must continue without invoking a reporter or retaining the error payload; it may count the failure without formatting or logging it. Neither continuation policy must retry the failed callback or reinsert its input automatically.

CollectAll must require a corresponding reporter. A runtime policy that can select CollectAll must also require one unless static checking proves that case impossible. A supplied reporter must be checked even when the selected policy is Ignore. Item reporters must receive `E`; callback reporters must receive the attempted input and `F`. Reporters must return `None`, either synchronously or asynchronously. Async reporting must complete before another item is requested or callback is invoked. Fallible reporting and recovery from reporter failure remain outside this first surface; a `Result`-returning reporter must not have its error silently ignored.

Item policies must not govern source advancement failures. Any outer source failure must stop the consumer, preserve pending values, and return a typed source failure. Generic consumers must not repoll after that failure. A source may implement recovery internally according to its documented cursor and cancellation contracts, consistent with RFC 115. Explicit result items are how a source exposes independent, recoverable records to a policy-aware consumer.

### Batching, progress, and terminal outcomes

Batch size must be a positive integer, checked before the first source advancement. A batch must contain up to that many successful values in source order. Item errors must not occupy successful-value slots. A full batch must be delivered before requesting another source item, and ordinary exhaustion must deliver a final nonempty partial batch. The consumer must never invoke the callback with an empty batch.

FailFast, a terminal source failure, and cancellation must not be treated as ordinary exhaustion or automatically flush a pending partial batch. A callback failure under CollectAll or Ignore must move on to the next batch after applying its reporting policy. Even when delivery could have partially succeeded, the user must be allowed to choose either continuation policy; generic consumption must not impose mandatory stopping on write errors.

The terminal outcome must distinguish normal exhaustion from stopping with a typed failure, using the candidate `Consumption.Completed` and `Consumption.Stopped` cases. A failure must identify its stage and preserve its typed error; an error string alone is insufficient. The concrete outcome and failure declarations remain Draft API work. Within each consumer, all policy values must share the same outcome type.

Progress must count successfully completed callback inputs, item failures, and callback failures separately. The candidate `summary.processed` field must count values whose callback returned success, including the size of successful batches. For `collect`, it must instead count successfully collected values. A failed batch must not be counted as successfully processed, even if its callback performed partial external work. An empty source must complete with zero progress and no callback invocation. A run that reaches exhaustion after reporting or ignoring errors must complete with failure counts rather than pretending every item succeeded; completion does not mean an error-free run.

A stopped batch consumer must retain the successful values buffered before an item or source failure. On callback failure under FailFast, it must instead expose the attempted batch. A single-item consumer must preserve its attempted value on callback failure. The candidate pending representation is an ordered list, containing at most one value for a single-item consumer. Already successful deliveries must not be duplicated in pending values. Delivered external effects must remain intact; retaining attempted inputs must not be described as proof that they are unsaved or safe to replay.

The consumer must have a checked means of retaining attempted input across a callback that takes ownership. It must not transfer the only copy into the callback and then claim to return that input on failure. The candidate initial constraint is `T with Clone`: retain input and pass a clone to the callback, dropping the retained input after success. Cloning must follow the type's existing contract; it does not promise a deep snapshot or independent mutable descendants. Exact ownership-preserving bounds and alternatives for non-cloneable values remain open before Planned.

`collect` must be an explicit materializing terminal. It must return the collected values on normal exhaustion and preserve the partial collection when stopping. Its outcome must carry those values in addition to progress and any terminal failure. This is the new async collection contract; it must not silently change RFC 115's implemented synchronous `collect`, which returns the first source error without returning its partial list. Exact collection outcome spelling remains open.

Non-materializing consumers must not silently retain all successful values or all errors. They may retain a pending value or batch and the recovery copy needed for an in-flight callback, plus progress and terminal failure information. Source buffering, error payloads, reporters, and callbacks may retain additional memory under their own contracts. On an unbounded source, CollectAll must report incrementally and may never reach a terminal summary.

### Control flow, ownership, and cancellation

`break` must stop further advancement. `return`, body-level error propagation, and panic must leave the loop under their existing control-flow rules. None of these exits may trigger an extra fetch merely to drain the source. Ordinary loop ownership and binding rules must apply under RFC 129; the syntax must not promise that a named source is reusable after the loop independently of those rules.

The pending advancement must retain access to its iterator state while suspended. Concurrent advancement through another alias must be rejected when it conflicts with that access. An iterator must not gain `Send`, `Static`, or other runtime capabilities merely because it appears in an async loop; the enclosing async execution contract must check those requirements where needed.

If cancellation drops the pending advancement, the loop must stop advancing under the existing async runtime cancellation rules. It must not resume or retry that operation automatically. Whether cancellation consumed data, advanced a remote cursor, or performed an external effect remains part of the source's cancellation contract. In particular, dropping a local future must not be presented as undoing an already transmitted network request.

Leaving an async loop must not implicitly await a close hook, unsubscribe operation, flush, or transaction rollback. Existing ownership-based synchronous cleanup must still run under its normal rules. A resource needing asynchronous shutdown must expose an explicit operation or a separately specified structured resource scope. RFC 094 is the related scoped-cleanup proposal; this RFC must not claim that plain iteration establishes its guarantees.

Async adapters and consumers must preserve those same cancellation boundaries. Dropping a pending consumer future must not imply that a stopped outcome is returned to its caller or that its pending values remain observable. ErrorPolicy must not turn cancellation or panic into an ordinary item failure. Cancellation during a callback or reporter must not trigger an automatic retry, partial-batch flush, awaited cleanup, or rollback. Stronger recoverable-cancellation behavior requires a separate explicit ownership and resource contract.

### Diagnostics and tooling

The compiler must distinguish unsupported async iteration, missing async activation, use outside an async function, a missing fallible header marker, incompatible error propagation, ambiguous protocols, and setup-versus-advancement ambiguity. Diagnostics should identify the source's selected item and error types and suggest a separate setup binding where appropriate.

Generated documentation, manifests, and editor tooling must preserve the async status of the advancement method, inferred member roles, selected source protocol, and propagated error type. Hover and semantic queries should explain whether the header's `?` handles setup or advancement.

Adapter and consumer diagnostics must also distinguish a synchronous mapping callback from an async callback, result items from source errors, unsupported callback ownership, missing CollectAll reporters, incompatible reporter signatures, and invalid policy options. Metadata must preserve these distinctions across library boundaries rather than rediscovering them through callback names or backend-only types.

## Design details

### Existing async foundation and the broader candidate scope

The existing public `std.async` surface already includes spawning, blocking-task offload, task handles, yielding, sleep and relative timeouts, race, bounded and unbounded channels, one-shot channels, mutexes, read/write locks, semaphores, and barriers. `timeout_join` already distinguishes deadline expiry from cancellation by returning a live handle. These are foundations to compose, not functionality this RFC claims to introduce.

The remaining areas below belong to this Draft's broader async exploration. Plain join, the sequential stream surface, selected policy-aware join rules, and task-group ownership and shutdown boundaries are specified above. Other entries record candidate capabilities and remaining decisions rather than finalized declarations or an implementation commitment.

#### Joins and task groups

The prior art for plain joining is Tokio's [join](https://docs.rs/tokio/latest/tokio/macro.join.html): independent branches are multiplexed on the current task, and all results are awaited. Tokio's [try_join](https://docs.rs/tokio/latest/tokio/macro.try_join.html) illustrates early error completion. Incan's direction uses ErrorPolicy across operations rather than adopting Rust macro syntax or reducing every failure to one early error value.

A dynamic group of spawned tasks is a separate capability from a fixed heterogeneous join. Tokio's [JoinSet](https://docs.rs/tokio/latest/tokio/task/struct.JoinSet.html) provides same-output-type task collection in completion order. It aborts tasks on drop; its explicit shutdown operation aborts and then awaits their termination. These distinctions matter for an Incan task-group API: ownership, completion order, failure policy, and whether exiting waits for cleanup must be explicit. A task group must not be advertised as providing borrowed scoped-task lifetimes merely because it owns task handles.

Existing Incan JoinHandles detach on drop. The proposed task group instead owns its members, requests cancellation on drop, and requires explicit detachment for independent continuation. Its awaited shutdown requests cancellation and confirms termination. This follows the distinction between Tokio JoinSet destruction and awaited shutdown while retaining Incan's own failure-reporting contract. Abort requests differ from completed shutdown; running blocking tasks cannot generally be interrupted. Task-group outcomes are delivered in observed completion order with task identity retained. Precise detach and shutdown result surfaces, child-failure policy, panic reporting, and interruption during shutdown still need concrete test drives. A Python-style awaited task scope remains an RFC 094 integration question.

#### Cooperative cancellation

Task abortion and graceful shutdown serve different purposes. A shared cancellation request lets a worker stop accepting input, finish or abandon its current unit under an application contract, and await its own cleanup. The prior art is [tokio-util CancellationToken](https://docs.rs/tokio-util/latest/tokio_util/sync/struct.CancellationToken.html), which supports waiting for a cancellation request and deriving child tokens.

Cancellation requests propagate downward through explicitly linked parent/child relationships, without automatically traveling upward or across to siblings. Incan still needs to choose the public token vocabulary, observation and lifecycle methods, interaction with race and deadlines, and the exact typed representation distinguishing cancellation from application failure. Requesting cancellation must not imply that a worker has finished. ErrorPolicy.Ignore must not suppress shutdown requests.

#### Bounded concurrent processing

Sequential `for_each` and `for_each_batch` await one callback at a time. A separately explicit concurrent operation could overlap work with a bounded number of in-flight callbacks. Existing bounded channels and semaphores provide useful primitives, but they do not by themselves define a generic consumer's ordering, buffering, or failure contract.

The missing decisions are the concurrency option or operation spelling, input order versus completion order, behavior when a slow early item blocks ordered delivery, preservation of completed successes, and the treatment of already in-flight callbacks after FailFast. Cancellation cannot make their previous effects disappear. No concurrency option is implicitly added to the sequential consumers in this revision.

#### State observation, fan-out, and notifications

The public async API does not yet expose watch channels, broadcast channels, or a general Notify primitive. A notification primitive is used internally, which is not equivalent to a checked public Incan API. Tokio's [synchronization documentation](https://docs.rs/tokio/latest/tokio/sync/index.html) distinguishes these capabilities: watch exposes current state, broadcast distributes messages to subscribers, and Notify wakes a waiter without carrying a message payload.

Candidate uses are live configuration, progress subscriptions, and worker wakeups. Contracts must distinguish missed intermediate states, lagging subscribers, bounded buffering, closure, and notification coalescing. An ordinary bounded channel must not be presented as providing all of these semantics through a name change.

#### Timers, deadlines, and deterministic async tests

Relative sleeps and timeouts already exist. The remaining public gaps include a monotonic Instant, absolute deadlines, repeating intervals, missed-tick behavior, and a controllable test clock. Tokio's [time APIs](https://docs.rs/tokio/latest/tokio/time/index.html) provide the prior art for these distinct capabilities. An interval is not simply sleeping after each completed unit of work, and a shared absolute deadline prevents nested operations from each receiving a fresh full timeout budget.

A test clock would let applications exercise retries, batching deadlines, and shutdown without depending on wall-clock sleeps. Tokio's [pause](https://docs.rs/tokio/latest/tokio/time/fn.pause.html) and [advance](https://docs.rs/tokio/latest/tokio/time/fn.advance.html) illustrate that capability, but paused time requires its test utilities and current-thread runtime. An Incan test surface must choose compatible runtime behavior explicitly rather than promise that those controls work unchanged in every executor configuration. A controlled clock does not make arbitrary task scheduling deterministic.

### Python familiarity and Incan differences

Python's `async for` obtains an async iterator and awaits its next-item operation before each body execution. Python also provides async generator functions, which are not implied by adding the loop statement alone. The official [async for reference](https://docs.python.org/3/reference/compound_stmts.html#the-async-for-statement) and [async generator reference](https://docs.python.org/3/reference/expressions.html#asynchronous-generator-functions) provide the principal prior art.

Incan keeps its `Option` exhaustion and typed `Result` failure conventions. It does not add Python's `StopAsyncIteration` exception to source programs. The separate `?` marker preserves RFC 115's distinction between an infallible item source and failure while obtaining an item.

### One async method contract

The protocol declares an async method whose source-level result is an `Option` or `Result`. Loop lowering awaits that method according to Incan's existing direct-call rules. This avoids introducing a user-visible future storage field, a second `Awaitable` adoption model, or a requirement that library authors write a backend polling API themselves.

The exact standard module and prelude exposure remain Draft questions. The protocol names here are the working public vocabulary; examples do not claim those names are already importable.

### Composition and generators

RFC 088 adapters and RFC 115 fallible adapters do not automatically apply to async protocols. This RFC explicitly introduces async-source `map` with a synchronous callback and async terminals that can await their callbacks. Further adapters and async transformations require their own checked contracts. Concurrency remains an explicit operation with its own ordering and capacity contract.

RFC 006's synchronous `yield` rules remain unchanged. Async generator functions would provide a useful source-authoring convenience, but they need an explicit return type and a decision about propagating errors from a suspended producer. They remain outside this revision; direct Incan-authored protocol implementations supply sources without depending on async generators.

### Result items and source failures

A parser can fail on one message while the next message remains available. Representing that failure as a `Result` item lets the consumer choose how to proceed. A failure of the reader itself is a different channel, represented by `AsyncFallibleIterator`. This keeps the recovery boundary concrete without changing RFC 115 or teaching generic adapters to guess whether another poll is safe.

Policies belong to consumption in this first surface. `map` keeps its ordinary transformation meaning, so the same lazy result-item pipeline can be consumed under different policies without hiding an error-handling decision inside the parser. This does not imply that a consumed single-pass source can be reused. Independent policies at multiple intermediate transformations would require a separate stage-composition contract.

### Lazy processing and explicit materialization

`for_each` and `for_each_batch` deliver values incrementally. A terminal outcome describes progress and any stopping point, rather than holding every successful value. `collect` is the explicit request to retain all values, and its partial collection remains available on failure. This avoids making policy selection determine whether the caller receives a plain list, a list of results, or a result containing a list.

CollectAll uses typed reporters instead of retaining every error until exhaustion. Reporters can choose an external diagnostic stream, a quarantine destination, or bounded aggregation. Returning a summary after exhaustion is useful for finite input; it must not be the only way to observe failures on a long-running subscription.

### Compatibility

This proposal is additive. Existing synchronous iteration, generator types, fallible iteration, and async calls retain their contracts. Native associated members are an explicit dependency on RFC 098's revised design, not a claim that its syntax has shipped. Adopting async iteration must not silently change the error type, resource lifetime, buffering, or retry policy of an existing source API.

## Alternatives considered

- **Manual await-and-match loops:** expressive enough for individual sources, but duplicate advancement, exhaustion, and propagation scaffolding at each call site.
- **Materialize all values before an ordinary loop:** simple for small bounded inputs, but delays processing and cannot represent unbounded sequences.
- **Make ordinary `for` await automatically:** hides a suspension point and blurs the synchronous and asynchronous contracts.
- **Yield `Result` items for every fallible source:** useful when individual records are themselves successes or failures, but different from a failure to advance the source. RFC 115 already keeps those cases distinct.
- **Automatically spawn and buffer a producer:** changes scheduling, resource lifetime, capacity, ordering, and cancellation. Those policies belong to an explicitly concurrent source or adapter.
- **Automatically await shutdown on loop exit:** would create a new asynchronous cleanup contract, including cancellation and failure rules, as an implicit consequence of iteration.
- **Give `map` an error policy:** changes its meaning when the callback returns a `Result` and hides the distinction between result values and failed advancement. The initial surface keeps mapping ordinary and makes handling explicit at consumption.
- **Return a different container for each policy:** can be useful for a specialized eager API, but complicates runtime policy selection and loses the incremental processing model. The async consumers use one outcome type per operation.
- **Always stop on failed delivery:** prevents legitimate best-effort processing. The callback policy must allow continued processing, with no promise of rollback or retry safety.
- **Automatically commit a partial batch on failure:** makes a stopping decision trigger an additional external effect. Stopping preserves pending values and leaves further delivery to the caller.
- **Collect every error until the end:** may consume unbounded memory or never deliver diagnostics for an unbounded source. CollectAll reports incrementally instead.

## Drawbacks

There are now synchronous and asynchronous iteration protocols, each with an explicit fallible form. Readers must learn which operation can wait and which can fail. Reusing `?` helps maintain consistency, but setup results containing streams still need a separate binding.

Async iteration can look lazy even when a source buffers eagerly or runs an independent producer. Documentation must distinguish the loop's sequential demand from the source's internal behavior. Early exit stops further loop requests but does not guarantee asynchronous shutdown or reversal of external effects.

The proposal depends on the revised associated-type and callable designs. Source authors initially write explicit stateful implementations; async generators and the remaining adapter family are follow-up work. Compiler and editor support must account for suspension, mutable iterator access, typed propagation, and cleanup without inventing stronger cancellation guarantees than the source provides.

The consumer surface adds policy options, reporters, progress, and recovery values. Retaining an attempted input may require cloning and can exclude types without that capability. Reporting every error can dominate processing cost; Ignore deliberately avoids reporting. A completed outcome can include handled failures, so users must distinguish exhaustion from complete success. Batch delivery may have partial effects, and generic library code cannot infer which values were committed by a failed callback.

## Layers affected

- **Parser / AST:** import-activated `async for`, existing loop patterns, and the explicit fallible header form.
- **Typechecker / Symbol resolution:** async protocol selection, inferred item and error members, async method and callback validation, result-item consumer overloads, reporter requirements, and error compatibility.
- **Ownership / async checking:** suspended iterator and callback receiver access, prevention of overlapping advancement, retained callback input, control-flow exits, and existing execution capability requirements.
- **IR Lowering / Emission:** sequential async advancement and callback invocation with ordinary exhaustion, typed propagation, and policy-aware terminal outcomes, preserving existing await and cleanup semantics.
- **Stdlib / Runtime:** plain join, async protocol declarations, lazy mapping, per-item and batch consumers, ErrorPolicy, reporting, progress, collection, and recoverable terminal outcomes. Task groups, cooperative cancellation, bounded concurrency, notification primitives, and time controls are additional candidate surfaces whose contracts remain open.
- **Formatter:** stable formatting for async loop headers and proposed protocol declarations.
- **Library manifests / LSP / Tooling:** async advancement and callback contracts, selected source protocols, inferred types, consumer policies, propagation boundaries, and diagnostics.

## Design Decisions

- Cooperative cancellation follows explicitly configured parent/child links downward to descendants. Cancelling a child does not by itself cancel its parent or siblings, and calls or task spawning do not implicitly create cancellation links. Failure policy remains separate; a stop request is not proof of termination or rollback.
- Task groups deliver identified outcomes in observed completion order, without holding completed work behind earlier unfinished submissions. Concurrently ready tasks have no promised deterministic tie order. Submission-order reconstruction is explicit, while fixed join retains its argument-order tuple.
- Task groups own their members, request cancellation of remaining async tasks on drop, and require explicit detachment for independent continuation. The proposed awaited shutdown requests cancellation and waits for termination; ordinary drop does not wait. Existing standalone JoinHandle semantics remain unchanged, immediate FailFast reporting remains immediate, and Python-style awaited scope exit is deferred to RFC 094.
- Policy-aware join preserves one independently typed result position per input operation in argument order, including completed successes after an early failure. Positions distinguish success, retained failure, ignored failure, and unfinished work. Ignore discards its error payload without removing the position. Runtime policy selection retains the same static outcome type within this form.
- FailFast drops directly owned pending join operations without awaiting shutdown. Previously spawned tasks retain the existing handle-detach contract unless an explicit task owner supplies cancellation. Unfinished identifies absence of an obtained result rather than proving the underlying work has stopped. Task-group ownership remains a distinct surface.
- FailFast returns immediately upon the first observed branch failure without draining unfinished branches. Unfinished work is distinct from failure and does not imply a resumable operation or absence of external effects. Exact public outcome names and generic declarations remain open below.

## Unresolved questions

- How do explicitly spawned tasks inherit or select RFC 122's validation_policy execution context, coordinated with RFC 094's scope lifetime and suspension safety? Ordinary helper calls inherit the active scope; spawning must not be treated as an implicitly settled equivalent.
- What are the exact policy-aware join call options, generic outcome declarations, and reporting contracts? Preserved typed positions, explicit success/failure/ignored/unfinished states, immediate FailFast, and a policy-independent static outcome type are settled.
- What exact detach and shutdown result surfaces, child-failure policy, and panic reporting should task groups expose, including interruption while awaiting shutdown? Identified outcomes in observed completion order, member ownership, cancel-on-drop without awaiting, explicit detach, and an awaited termination milestone are settled.
- What cancellation-token vocabulary, lifecycle and observation methods, and typed outcomes should integrate with race, task groups, and deadlines? Explicit downward parent/child propagation without automatic upward or sibling cancellation is settled.
- Which explicit bounded-concurrency API should accompany sequential consumers, and how should it preserve order and completed work after failure?
- Which watch, broadcast, and notification contracts should be exposed publicly, including lag, closure, and coalescing behavior?
- What monotonic deadline, interval, missed-tick, and test-clock contracts should fit Incan's supported runtimes?
- Which standard modules and preludes should expose the async protocols, ErrorPolicy, and terminal outcome types?
- What are the exact generic declarations for consumption outcomes, typed failure stages, progress, and partial collection values?
- How should shared callable metadata express async callback values and distinguish their terminal overloads from synchronous callbacks without treating arbitrary return values as awaitable?
- Should recoverable callback delivery initially require `T with Clone`, or can checked ownership-preserving callbacks also support non-cloneable values?
- How should a future async transformation adapter be spelled, while preserving `map` as a synchronous callback operation on an async source?
- Is a distinct `AsyncFallibleIterable` protocol needed initially, beyond single-pass `AsyncFallibleIterator` sources?
- Should loop `else` be admitted alongside async iteration, subject to the existing ordinary loop grammar and exhaustion rules?
- Should a future resource-scope proposal guarantee awaited shutdown for selected stream resources, and how should cleanup interact with cancellation and an already propagating error?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
