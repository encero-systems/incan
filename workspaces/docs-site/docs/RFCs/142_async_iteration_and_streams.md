# RFC 142: Async composition, iteration, and streams

- **Status:** Planned
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
    - RFC 122 (scoped validation policy and `ErrorPolicy`)
    - RFC 128 (shared callable signatures and bound methods)
    - RFC 129 (bindings, borrowing, and ownership)
    - RFC 131 (first-class typed errors)
    - RFC 153 (async closures and async callable types)
    - RFC 164 (explicit ownership and borrowing)
    - RFC 168 (`std.primitives`)
- **Issue:** [#2585](https://github.com/encero-systems/incan/issues/2585)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Add Python-familiar `async for` loops, lazy async mapping, and async consumers for individual values and batches. Sources await advancement; consumers await application callbacks without implicitly spawning tasks. A fallible source uses `async for item in source?:`, extending RFC 115's explicit error propagation. Separately, streams of `Result` items support independent `ErrorPolicy` choices for item failures and callback failures: stop, report and continue, or ignore and continue. Terminal outcomes preserve progress and pending successful values when processing stops. Native async protocols use RFC 098's proposed `infer` headers. The syntax and APIs in this document are proposals, not implemented functionality.

The broader composition scope adds plain and policy-aware `join`, task groups, cooperative cancellation tokens, bounded concurrent consumption, watch, broadcast and notification primitives, shared deadlines, intervals, and a virtual test clock. Following RFC 168, the iteration protocols with their consumers and outcome types, `ErrorPolicy`, cancellation tokens, the watch, broadcast and notification primitives, timers, and the monotonic `Instant` are `std.primitives` declarations, so compiler and platform code can use them without the higher-level library. Joins, task groups and deadline conveniences are `std.async` compositions. Applications import every name from a `std.async` module, which re-exports the foundations with the same identity.

## Core model

1. **Waiting belongs to advancement:** `async for` awaits each next-item operation without requiring the caller to write a manual polling loop.
2. **One item at a time:** the next advancement begins only after the current loop body completes normally or continues.
3. **Exhaustion is ordinary:** an infallible source returns `None` when finished; a fallible source returns `Ok(None)`.
4. **Errors remain explicit:** a fallible loop carries `?` and propagates a failed advancement through its surrounding `Result` contract.
5. **Sources own their policies:** a pagination source chooses when to fetch pages; a channel source chooses buffering; a recoverable source owns retry policy.
6. **Stopping and cleanup are distinct:** leaving the loop stops further advancement. Asynchronous resource shutdown requires an explicit operation or an RFC 094 `async with` scope.
7. **Composition remains lazy:** constructing a mapping pipeline does not advance its source or invoke its callback. Consumption drives work; collection explicitly requests materialization.
8. **Policies have separate owners:** an item error and a consumer callback error can use different policies. Neither grants permission to retry a failed source advancement.
9. **Progress survives failure:** stopping does not discard previously delivered values or a pending batch. A partially processed batch is not automatically committed, retried, or rolled back.
10. **Composition and task ownership are distinct:** joining concurrent operations does not implicitly spawn tasks. Owning spawned tasks, cancelling them, and awaiting shutdown require explicit contracts.
11. **Foundations below, compositions above:** under RFC 168, the async protocols, the consumer methods they provide, and every type those methods use are `std.primitives` declarations; `std.async` re-exports them with the same identity and composes joins and task groups on top. Existing types such as `SendError`, `RecvError`, `TimeoutError`, `TaskJoinError` and the monotonic `Instant` are reused, never redefined.

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
- Define lazy `map` and `for_each`, `for_each_batch`, and explicit `collect` consumers, on async sources and with the same contract on synchronous iterators of `Result` items.
- Distinguish source advancement failures, yielded `Result` item failures, and consumer callback failures.
- Allow independent FailFast, CollectAll, and Ignore policies for item and callback failures, including runtime-selected policies.
- Preserve pending values and typed failure information when a consumer stops.
- Add plain and policy-aware `join`, task groups, and cooperative cancellation tokens.
- Add explicit bounded concurrent consumption that reports failures immediately and stops in-flight work as a separate step.
- Add watch, broadcast, and notification primitives with distinct delivery promises.
- Add shared deadlines, intervals with an explicit missed-tick policy, and a virtual clock for async tests.
- Place foundations in `std.primitives` and compositions in `std.async` under RFC 168, with every name importable from `std.async` and one `ErrorPolicy`.

## Non-Goals

- Automatically fetching pages, retrying failures, or selecting a pagination framework.
- Implicit prefetching, spawning tasks, processing items concurrently, or making blocking operations asynchronous.
- Claiming that every source is lazy, bounded in memory, or safe to retry after cancellation.
- Defining async generator functions, async comprehensions, or the complete synchronous adapter family for async sources.
- Concurrent `map` adapters, concurrent batch delivery, and loop `else`.
- Simulating external I/O under the virtual test clock, or making task scheduling deterministic.
- Promising transactional batch delivery, automatic retries, checkpoints, or exactly-once external effects.
- Adding exception-based exhaustion or changing typed errors into exceptions.
- Automatically awaiting a resource's shutdown when a loop ends.
- Redesigning `Awaitable`, callable signatures, or task scheduling.

## Guide-level explanation

### Await independent operations together

Plain `join` is proposed composition functionality from `std.async.join`, beside the existing `std.async.race`. The example defines both operations; `sleep` follows the existing time API.

```incan
import std.async
from std.async.join import join
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

An error-aware form reuses ErrorPolicy rather than making `failfast` a separate joining function. FailFast returns immediately on the first observed branch failure while preserving completed successes; CollectAll awaits and exposes all branch outcomes; Ignore awaits the remaining branches and discards application error payloads. The outcome retains one independently typed position per operation in argument order, distinguishing completed success, retained failure, ignored failure, and unfinished work. Policy selection, including a runtime enum value, must not change this form's static outcome type.

The policy form is the same `join` with an `errors=` option. Each position becomes a `JoinSlot[T, E]` for that operation's own success and error types:

```incan
from std.async.join import join, JoinSlot
from std.async.iter import ErrorPolicy

error FetchError:
    Unavailable

async def count() -> Result[int, FetchError]:
    return Ok(42)

async def label() -> Result[str, FetchError]:
    await sleep(0.01)
    return Err(FetchError.Unavailable)

async def inspect(policy: ErrorPolicy) -> None:
    count_slot, label_slot = await join(count(), label(), errors=policy)
    match label_slot:
        JoinSlot.Succeeded(value) => println(value)
        JoinSlot.Failed(original) => println(original.message())
        JoinSlot.Ignored => println("failure deliberately ignored")
        JoinSlot.Unfinished => println("no completed result was obtained")
```

`count_slot` is a `JoinSlot[int, FetchError]` and `label_slot` a `JoinSlot[str, FetchError]`, whichever policy the caller passes at runtime. Suppose three operations would normally return `(42, "Alice", true)`. If the second fails while the third is still running, a FailFast outcome is:

```text
(JoinSlot.Succeeded(42), JoinSlot.Failed(original_error), JoinSlot.Unfinished)
```

The successful first value remains available and the third position is not misreported as a failure. Under Ignore, a failed position is explicitly marked ignored without retaining its error payload; it is not removed or allowed to shift the remaining positions. An unfinished position means join did not obtain a completed result; it does not prove that underlying work has stopped, is resumable, or performed no effects. On FailFast, join drops directly owned pending operations. A previously spawned task retains the existing handle-detach contract and continues independently unless explicit task ownership supplies a cancellation policy. Task-group ownership is the proposed surface for that separate requirement.

### Own spawned work with a task group

The proposed TaskGroup owns its member tasks. If a pipeline returns early and drops its group, the group requests cancellation of its remaining async tasks through the existing task-abort mechanism. Tasks must be explicitly detached to continue independently after group destruction. This does not change the existing standalone JoinHandle's detach-on-drop contract.

Task outcomes are delivered in observed completion order, with each outcome identifying its task. For example, if three uploads are started as Slow, Fast, and Medium but finish as Fast, Medium, and Slow, the caller can receive them in that latter order. A completed upload does not wait behind a slower earlier submission. Task identity keeps each result attributable without treating its delivery position as its identity. Authors needing submission order may explicitly arrange identified results afterward.

Requesting cancellation is separate from observing termination. The proposed `await group.shutdown()` operation requests cancellation and waits for the remaining owned tasks to terminate. Ordinary dropping cannot await that work. Immediate FailFast reporting therefore remains compatible with a later explicit awaited shutdown. This is task termination, not a promise of transaction rollback, reversible effects, or arbitrary asynchronous application cleanup.

```incan
from std.async.task import TaskGroup

enum Upload:
    Photos
    Documents

error UploadError:
    Rejected

async def upload(fail: bool) -> Result[int, UploadError]:
    await sleep(0.01)
    if fail:
        return Err(UploadError.Rejected)
    return Ok(1)

async def run() -> None:
    mut group = TaskGroup[Upload, int, UploadError]()
    group.spawn(Upload.Photos, upload(false))
    group.spawn(Upload.Documents, upload(true))
    first = await group.next()          # the first finished upload, with its id
    println(first)
    report = await group.shutdown()     # request cancellation, then await termination
    println(report)                     # termination evidence and undelivered failures
```

`group.detach(Upload.Photos)` would instead return an independent handle for that task and remove it from the group, so dropping the group no longer stops it. Each outcome is delivered once: a failure already returned by `next()` is not repeated in the shutdown report. If awaiting `shutdown()` is itself interrupted, the report is never produced and no termination claim is made; dropping the group still requests cancellation.

A task group is also an RFC 094 async context manager, which gives Python's `asyncio.TaskGroup` shape:

```incan
from std.async.task import TaskGroup, TaskGroupFailure

async def run_scoped() -> Result[None, TaskGroupFailure[Upload, int, UploadError]]:
    async with TaskGroup[Upload, int, UploadError]() as group:
        group.spawn(Upload.Photos, upload(false))
        group.spawn(Upload.Documents, upload(true))
    return Ok(None)
```

When the block ends normally, the group waits for its remaining members to finish. A failure or panic that nobody read with `next()` becomes the block's `TaskGroupFailure`, which leaves `run_scoped` as `?` would; here that is the rejected documents upload. A success nobody read is dropped. If the block itself leaves with an error or a panic, the group instead requests cancellation and waits for termination, as `shutdown()` does, and RFC 094 keeps the body's failure next to the members' failures. If the task running the block is cancelled, the async exit cannot run, and dropping the group requests cancellation of its members.

### Spawned tasks keep the validation policy they started with

Ordinary calls already run under the active RFC 122 validation policy. An explicitly spawned task captures the policy active at the moment it is spawned and keeps it, even after the parent leaves that scope:

```incan
async def child() -> None:
    await sleep(0.02)                 # the parent leaves its scope first
    println(current_policy())         # FailFast: the policy captured at spawn

async def parent() -> None:
    with validation_policy(ErrorPolicy.FailFast):
        handle = spawn(child())
    println(await handle)             # the parent's own policy was restored at exit
```

The child cannot change the parent's policy, and scopes it opens inside itself restore the child's own previous policy. With no active override at spawn, the child uses the documented default. `current_policy` is a proposed `std.validation` inspection helper shown only to display the policy.

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

The links are made with a `CancellationToken`. A worker receives a child token and checks it at its own stopping points:

```incan
from std.async.cancel import CancellationToken, Run

async def worker(stop: CancellationToken) -> None:
    loop:
        match await stop.run(sleep(1.0)):
            Run.Cancelled => return
            Run.Completed(_) => println("one scheduled check")

async def main() -> None:
    stop = CancellationToken()
    stop.cancel()
    await worker(stop.child_token())    # already cancelled: exits without a check
```

`stop.run(operation)` awaits the operation once and returns `Run.Completed(value)`, or `Run.Cancelled` as soon as a stop is requested, dropping the operation. It is not a repeating timer: the worker loops explicitly. Constructing a token starts no task, and a child's own `cancel()` reaches only that child and its descendants.

### Read a sequence that may need to wait

```incan
import std.async

async def print_users(client: Client) -> None:
    async for user in client.users():
        println(user.name)
```

This illustrative `Client.users()` API returns an async source of `User` values. The example assumes an infallible source so the loop's basic behavior is visible. `Client`, `User`, and the method are application-defined examples, not new stdlib APIs.

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
from std.async.iter import AsyncIterator
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

A fallible source that can be read more than once, such as a catalog, exposes a method that returns a fresh fallible iterator each time: `async for row in catalog.rows()?:`. This matches RFC 115, whose synchronous surface also has no separate fallible iterable protocol.

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
from std.async.iter import AsyncIterator, ErrorPolicy
from std.json import JsonValue, JsonError

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

`Receiver.recv()`, `Sender.send()`, and the JSON operations follow existing stdlib APIs. `IncomingMessages`, `BatchQueue`, `parse_message`, and `process` belong to the application. `AsyncIterator`, its async-source `map`, and the policy-aware consumer are proposed `std.primitives` declarations, imported here through their `std.async.iter` re-exports. `ErrorPolicy` is the same enum that RFC 122 imports from `std.validation`, defined once in `std.primitives`.

`map` is provided by the async iterator protocol, analogous to RFC 088's synchronous iterator method. It constructs an adapter without reading a message. Its synchronous callback returns a `Result`, so the adapter's item type is `Result[JsonValue, JsonError]`. Mapping does not itself choose what a parsing error means.

`map` also takes an async callback, typed `Callable[async (A) -> R]` as in RFC 153, and awaits it once per item: `source.map(async (text) => await enrich(text))`. An ordinary closure is never treated as async merely because it is written inside an async function.

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

The same consumer and terminal outcome work for a runtime policy. Reporters are required when a policy can select CollectAll. They observe the error under FailFast and CollectAll; Ignore skips them. The batch reporter receives the attempted batch as well as its callback error. Reporting callbacks return `None`; they may be synchronous or async, with the async form awaited before continuing.

### Preserve a pending batch when stopping

Consider a batch size of three and inputs A, B, a malformed message, C, and D. With item FailFast, A and B have parsed successfully but have not reached the destination. The outcome is:

```incan
from std.async.iter import Consumption, ConsumptionFailure

match outcome:
    Consumption.Completed(progress) =>
        println(f"Delivered {progress.delivered} values")
    Consumption.Stopped(progress, failure, pending, attempted) =>
        println(f"Delivered {progress.delivered} of {progress.seen} values read")
        println(f"Retained {len(pending)} values never handed to the callback")
        match failure:
            ConsumptionFailure.Item(error) => println(error.detail())
            ConsumptionFailure.Callback(error) => println(error.message())
            _ => println("stopped by the source, a stop request or the deadline")
```

The stopped outcome keeps A and B in `pending`, in order. It reports the item failure and does not read C or D. It does not automatically submit the partial batch. The caller decides whether to deliver, inspect, or discard it.

Under CollectAll or Ignore, the malformed item does not count toward the batch size. The first delivery contains A, B, and C; ordinary exhaustion delivers the final partial batch containing D. A callback failure under FailFast instead returns the attempted batch as `Attempted.Retained(batch)`, separate from `pending`, because those values may already have had external effects: retaining them is not a claim that replay is safe. For example, if the third 100-row batch fails to save, the outcome reports 300 rows seen, 200 delivered, the callback's original error, and the third batch as attempted.

The consumer keeps each input while the callback borrows it, so no copy is needed and the item type does not have to be `Clone`. A callback whose parameter is `Owned[T]` takes the input instead; its failed input is then reported as `Attempted.Consumed`, and a callback that wants it back returns it inside its own error value.

`for_each` uses the same model for one successful value at a time, with `callback_errors` and `report_callback_error` in place of the batch options. `collect` explicitly materializes values and returns them with `Collected.Completed(values, failures, progress)`, or keeps the partial collection with `Collected.Stopped(values, failures, progress, failure)`. Under CollectAll without a reporter, `failures` holds every failed item in order, so a caller can see all the good and all the bad records at once. A protective `max_items=` stops with `ConsumptionFailure.Limit` and keeps what was read. The same consumers exist on synchronous iterators of `Result` items, so JSONL decoding or any other synchronous source uses this vocabulary too.

Every consumer also accepts `stop=` with a `CancellationToken` and `deadline=` with an `Instant`. When either fires, the consumer stops its pending read or callback and returns `Stopped` with `ConsumptionFailure.Cancelled` or `ConsumptionFailure.Deadline`, still holding what it had read. Dropping the consumer itself returns nothing, as with any dropped operation.

### Process several items at once

`for_each` waits for each callback before starting the next. `for_each_concurrent` keeps up to `limit` async callbacks running at once:

```incan
from std.async.iter import ErrorPolicy

async def upload_all(photos: PhotoSource, client: Client) -> None:
    run = photos.for_each_concurrent(client.upload, limit=8, callback_errors=ErrorPolicy.FailFast)
    outcome = await run.outcome()     # returns at the first failed upload
    println(outcome)
    stopped = await run.shutdown()    # stops the uploads still running
    println(f"{len(stopped)} uploads were stopped part-way")
```

The run reads a new photo only when one of its eight slots is free, so it never reads far ahead. Once the first upload fails, no new upload starts. `outcome()` returns at once with the failed photo and progress so far; it does not wait for the others still running. `shutdown()` then stops those that are still running and returns their photos in source order. The two steps are separate because a single `await` cannot both report a failure immediately and promise that all other work has stopped.

Callbacks run on the consumer's own task, as `join` branches do, so `for_each_concurrent` spawns nothing. They finish in any order; reporters run in that completion order and receive the input with its error. Under CollectAll or Ignore, `outcome()` returns after exhaustion, when every callback has finished and there is nothing left to stop.

### Share state, fan out, and wake workers

Three primitives cover the remaining message patterns, each with a different promise to the reader:

```incan
from std.async.channel import BroadcastReceiver, BroadcastRecvError, WatchReceiver

async def status_panel(status: WatchReceiver[Status]) -> None:
    loop:
        match await status.changed():
            Ok(_) => println(status.latest())
            Err(_) => return              # the sender is gone

async def audit(events: BroadcastReceiver[Event]) -> None:
    loop:
        match await events.recv():
            Ok(event) => record(event)
            Err(BroadcastRecvError.Lagged(missed)) => println(f"missed {missed} events")
            Err(BroadcastRecvError.Closed) => return
```

- `watch(initial)` keeps only the latest value. A status panel that sleeps through three changes sees the newest one, never the three in between.
- `broadcast(capacity)` gives every subscriber every message through a bounded buffer. A subscriber that falls more than `capacity` messages behind receives `Lagged(missed)` with the count, then continues from the oldest message still buffered, instead of silently losing messages.
- `Notify()` wakes a waiter without carrying data. A `notify_one()` with nobody waiting is remembered as one permit, so several such calls merge into a single wake-up.

All three are `std.primitives` foundations. Applications import `watch` and `broadcast` from `std.async.channel`, next to the existing channels, and `Notify` from `std.async.sync`, next to `Semaphore` and `Barrier`.

### Share one deadline and test time without waiting

`timeout(5.0, op)` gives each call its own five seconds, so three helpers that each use it can take fifteen seconds in total. A shared deadline keeps them to five:

```incan
from std.async.time import TimeoutError, deadline_after, timeout_at

async def sync_account(client: Client) -> Result[Account, TimeoutError]:
    deadline = deadline_after(5.0)
    profile = (await timeout_at(deadline, client.fetch_profile()))?
    orders = (await timeout_at(deadline, client.fetch_orders()))?
    return Ok(Account(profile=profile, orders=orders))
```

`deadline_after` returns the existing monotonic `std.datetime.Instant`, and `timeout_at` returns the existing `TimeoutError`. `interval(1.0)` ticks once a second; if a slow tick handler misses ticks, the next tick waits for its next scheduled second rather than firing a burst of catch-up ticks, unless `missed=MissedTick.Burst` asks for them.

A test can run that code without waiting five real seconds:

```incan
from std.testing import VirtualClock, virtual_clock

async def test_sync_times_out(virtual_clock: VirtualClock) -> None:
    result = await sync_account(SlowClient())    # SlowClient answers after ten seconds
    assert result.is_err()
```

Under the `virtual_clock` fixture, time stands still until every task in the test is waiting on a timer, and then jumps straight to the next due timer. `await virtual_clock.advance(2.0)` moves it explicitly. The fixture exists only in test code. Where a test's runtime cannot provide virtual time, using the fixture is a test-build error rather than a silent fall-back to real time.

## Reference-level explanation

### Plain concurrent join

Plain `join` must accept a fixed argument list of async operations with independently checked output types and return a tuple of those outputs in argument order. It must await every branch, including branches returning `Err`, without flattening or converting their results. It must not require a homogeneous output type, implicitly spawn tasks, or claim parallel execution. Call arguments must follow ordinary evaluation order; polling and completion order must not alter tuple positions.

Joining must own its local branch operations until they finish or the composition is cancelled. Dropping the join operation must drop its locally owned pending branches according to their individual cancellation contracts. Awaiting a previously spawned task handle is different: dropping that handle retains the existing detach behavior and must not be described as cancellation of the spawned task.

The policy-aware form must retain one independently typed outcome position for each input operation in argument order. It must preserve completed success values on early failure and distinguish success, retained application failure, explicitly ignored application failure, and unfinished work. Ignore must discard the application error payload without removing or shifting its position. Policy values, including runtime-selected values, must share the same static outcome type within this form; this does not change plain join's tuple of unchanged outputs.

FailFast must return immediately on the first observed branch failure, rather than waiting to drain unfinished branches or selecting failure by lowest argument index. Task cancellation and panic must remain distinct from ordinary application failure. On FailFast, join must drop its directly owned pending branches under their existing cancellation contracts and return without awaiting their shutdown. A previously spawned task must retain its existing handle-detach behavior; dropping the wait or handle must not be represented as cancellation of that independently running task. Cancellation of such tasks requires an explicit owner, such as the proposed task group, and its separate contract. An unfinished position must mean that join did not obtain a completed result, not that underlying work is proven stopped. Neither an unfinished position nor retained success may promise reversal of external effects, resumability, or safe replay.

The policy-aware form must be spelled as `join` with an `errors=` option taking an `ErrorPolicy` value, which may be selected at runtime. Every argument of that form must return `Result[T, E]`, and its position must have type `JoinSlot[T, E]` with exactly the cases `Succeeded(value)`, `Failed(original)`, `Ignored` and `Unfinished`. `Failed` must retain the original typed error unchanged. Plain `join` with no `errors=` option must keep its tuple of unchanged outputs. A panic in a branch is not an application failure: it must propagate as a panic of the join, never become a `Failed` slot. Cancelling the task that awaits the join drops the whole composition and produces no outcome.

### Task-group ownership and shutdown

A task group must own its members until their completion or explicit detachment. Dropping the group must request cancellation of remaining owned async tasks through the existing task-abort mechanism, without awaiting termination. Dropping the group must not silently detach its remaining members. Explicitly detached tasks must cease to be group members and retain their independent task lifecycle.

Task-group consumption must deliver available task outcomes in observed completion order rather than withhold an outcome behind an earlier unfinished submission. Every delivered outcome must retain its associated task identity, including failure outcomes, so attribution does not depend on position. The contract must not promise a deterministic tie order among concurrently ready tasks or infer precise wall-clock completion order beyond observed evidence. Submission-order reconstruction is an explicit consumer concern. This must not change fixed join's argument-order tuple contract.

The proposed async `shutdown()` operation must request cancellation of remaining members and wait for their termination. Normal completion of that operation must establish that no remaining owned task is running. A cancelled or interrupted shutdown must not claim that this milestone was reached; ownership and drop behavior must remain applicable.

`TaskGroup[Id, T, E]` must expose `spawn(id, operation)`, an async `next()`, `detach(id)` and an async `shutdown()`. `next()` must return the next available identified outcome, or `None` once no owned member remains. Each outcome must carry its task's id and exactly one of: the success value, the original typed failure, a panic report, or cancellation. A member's panic must be delivered as its own identified outcome, never as a typed failure or a panic of the consumer. A member's failure must not by itself cancel other members; stopping related work is the consumer's explicit choice, such as calling `shutdown()`. `detach(id)` must remove that member from the group and return an independent handle with the standalone JoinHandle contract. `shutdown()` must return a structured report of termination and of every outcome not yet delivered, including failures and panics. An outcome already delivered by `next()` must not be repeated, and no failure may be silently suppressed.

Outcomes must use `TaskOutcome[Id, T, E]` with the cases `Succeeded(id, value)`, `Failed(id, error)`, `Panicked(id, error)` and `Cancelled(id)`. `Panicked` must carry the existing `TaskJoinError` that awaiting a standalone `JoinHandle` reports, rather than a second panic type. `shutdown()` must return `ShutdownReport[Id, T, E]`, whose `undelivered` list holds every outcome not yet delivered.

`TaskGroup` must adopt RFC 094's `AsyncContextManager`, entering with `Reference[TaskGroup[Id, T, E]] from self` and exiting with `Result[None, TaskGroupFailure[Id, T, E]]`. On a Success, Return, Break or Continue exit, the exit hook must wait for every remaining member to finish, as calling `next()` until it returns `None` would. On an Error or Panic exit, it must request cancellation and await termination, as `shutdown()` does. Either way, every `Failed` and `Panicked` outcome not already delivered must be returned in `TaskGroupFailure` and must not be suppressed; undelivered successes and cancellations are dropped. RFC 094 retains a body failure together with that exit failure. If the task running the scope is cancelled, the async exit is not called under RFC 094 and the group's drop must request cancellation of its members. Explicit `next()`, `detach()` and `shutdown()` remain available inside the scope.

This contract must not change standalone JoinHandle behavior, turn cancellation requests into proof of termination, promise that running blocking work is interruptible, or imply borrowed scoped-task lifetimes. Immediate FailFast reporting must not acquire an implicit awaited-shutdown delay through this ownership rule.

### Cooperative cancellation hierarchy

Explicitly linked cancellation parents and children must support downward propagation: requesting cancellation of a parent must signal its linked descendants. Requesting cancellation of a child must not by itself signal its parent or siblings. Task spawning, function calls, or sharing an execution runtime must not implicitly establish those links.

An application failure and an explicit cancellation request must remain distinct. ErrorPolicy may decide whether a failure stops related work; the cancellation hierarchy must not infer such a failure policy from its topology. Cooperative cancellation must remain a request rather than proof of termination, an implicit task-abort operation, or rollback of completed effects.

The public vocabulary must be `CancellationToken`, constructed with no arguments, with `child_token()`, `cancel()`, `is_cancelled()`, an async `cancelled()` that completes once a stop is requested, and an async `run(operation)` returning `Run[T]` with the cases `Completed(value)` and `Cancelled`. `child_token()` must return a token linked below its parent. `run` must await the operation once and drop it when a stop is requested; it must not repeat the operation or create a ticker. On an already cancelled token, `run` must return `Cancelled` without starting the operation. A token must not start, own or abort a task. Deadline expiry and application failure must remain separate from `Run.Cancelled`: a `run` around a timed operation reports the timeout through that operation's own result. A task group does not create token links; a worker receives a token explicitly.

### Validation policy at spawn

An explicitly spawned task must capture the RFC 122 validation policy active at the spawn call, or the documented default when no scope is active, and retain it for its lifetime. The parent restoring or changing its own policy afterward must not affect the child. A scope opened inside the child must restore the child's previous policy on exit and must not change the parent's. Inheriting a validation policy must not transfer any other authority or capability. Ordinary calls, awaited operations and `join` branches run in the caller's execution context and use its active policy, as RFC 122 specifies.

### Syntax and activation

The statement forms must be `async for pattern in expression:` and `async for pattern in expression?:`, using the ordinary loop body indentation and binding-pattern rules. The existing `std.async` import activation must govern the async syntax; under RFC 168 the foundational async import activates the same vocabulary, as specified under Layers, modules, and prelude. The statement must be rejected outside an async function or method.

The source expression must be evaluated once on entry. The loop must not implicitly await that expression, acquire a resource, or invoke an async factory on the caller's behalf. Asynchronous acquisition must remain explicit, as in the separate local in the guide.

Ordinary `for` must retain its current protocol resolution. It must not automatically consume an async source. Conversely, `async for` must not silently wrap a synchronous iterator; a library may provide an explicit adapter.

`async for` has no `else` clause, because ordinary `for` has none. Any loop `else` must give both forms the same meaning, running only after normal exhaustion.

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

No separate `AsyncFallibleIterable` protocol is defined, matching RFC 115's synchronous surface. A fallible source that can be read more than once must expose a method returning a fresh `AsyncFallibleIterator`. A fallible iterator must not be assumed restartable, and its cursor must not be cloned to make it so.

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

`AsyncIterator` must provide `map` for synchronous callbacks. A callback with input `Self.Item` and output `U` must produce an async iterator whose item type is `U`. `AsyncFallibleIterator` must provide the corresponding mapping operation while preserving its source error type.

`map` must also accept an async callback of type `Callable[async (Self.Item) -> U]` under RFC 153, awaiting it exactly once per yielded item before yielding its result `U`. The overload must be selected from the callback's declared async callable type, never by awaiting an arbitrary returned value. An ordinary closure must not become async because it is written inside an async function.

Constructing a mapping adapter must not advance its source or invoke the callback. Each adapter advancement must await at most one source advancement and invoke the callback exactly once for each yielded item. It must forward exhaustion and source failures without invoking the callback. It must preserve order and must not spawn tasks, prefetch another item, or change a `Result` callback output into a source advancement failure.

In particular, mapping an infallible async source with `(T) -> Result[U, E]` must produce result items, not an `AsyncFallibleIterator[U, E]`. If the source itself is fallible with error `S`, its mapped advancement result must retain the distinct structure `Result[Option[Result[U, E]], S]`. The outer error describes failure to advance; the inner error is an ordinary yielded item that a consumer can handle and continue past.

Mapping callbacks must use the shared callable checking model, including receiver access and repeated invocation requirements. An adapter must not accept a consuming callable as repeatable merely because its parameter and return types match.

### Async consumers and their policies

`AsyncIterator` and `AsyncFallibleIterator` must expose async `for_each`, `for_each_batch`, and `collect` consumers. Ordinary items are successful inputs. A policy-aware consumer for items of type `Result[T, E]` must distinguish `Ok(value)` from `Err(error)` explicitly. The result-item form must select a statically checked overload; this must not be dynamic shape inspection or an implicit recursive unwrapping of nested results. A nested `Result` in `T` remains a value.

`for_each` must invoke its application callback once for each successful value. `for_each_batch` must invoke it with ordered lists of successful values. Each consumer must accept a synchronous callback returning `None` or `Result[None, F]`, or an async callback with the same source-level return contract. Infallible callbacks have no callback-failure policy to apply. Async callbacks must be awaited once; synchronous callbacks must run directly and must not become nonblocking merely because the consumer is async.

The compiler must check callback async status, receiver access across suspension, error type, and repeatable invocation. A bound method must retain its bound receiver and ownership requirements under RFC 128 and RFC 129. Async callback values must use RFC 153's `Callable[async (A) -> R]` in RFC 128's shared callable model, and a consumer must select its async overload from that declared type. The consumer must not rely on runtime guessing or await an arbitrary synchronous return value.

The option names must distinguish these boundaries:

- `item_errors` and `report_item_error` apply to yielded `Err(E)` items.
- `callback_errors` and `report_callback_error` apply to a failed `for_each` or `for_each_concurrent` callback.
- `batch_errors` and `report_batch_error` apply to a failed `for_each_batch` callback.
- `collect` has an item policy and reporter, but no application delivery callback.
- `stop` takes a `CancellationToken` and `deadline` takes an `Instant`; both apply to every consumer and are stop conditions, not failure policies.
- `max_items` takes a positive integer and applies to every consumer as a protective limit on the number of source items consumed.

When the `stop` token is cancelled or the `deadline` passes, the consumer must drop its pending source advancement or callback, as `CancellationToken.run` drops its operation, request nothing further, and return `Stopped` with `ConsumptionFailure.Cancelled` or `ConsumptionFailure.Deadline`. A token already cancelled, or a deadline already past, on entry must stop before the first advancement. Dropping the consumer's own future must still return no outcome.

When `max_items` items have been consumed and the source has not yet reported exhaustion, the consumer must request nothing further and return `Stopped` with `ConsumptionFailure.Limit`, keeping everything already consumed. A limit is not evidence that more items exist. Selecting a prefix with an adapter such as `take(n)` is different: it defines the input, and consuming that prefix to its end completes normally without claiming anything about the rest of the source.

An omitted policy must default to FailFast. Policy options that have no corresponding failure channel must be rejected rather than silently affecting some other stage. Policies may be chosen independently and may be runtime enum values. Selecting a runtime policy must not change a consumer's static outcome type or require a policy-dependent union of materialized containers.

FailFast must stop on the first failure observed by sequential consumption. If a corresponding reporter was supplied, it must run before the stopped outcome is returned. CollectAll must report every failure at that boundary and continue after reporting completes. Ignore must continue without invoking a reporter or retaining the error payload; it may count the failure without formatting or logging it. Neither continuation policy must retry the failed callback or reinsert its input automatically.

For `for_each` and `for_each_batch`, CollectAll must require a corresponding reporter, and a runtime policy that can select CollectAll must also require one unless static checking proves that case impossible. For `collect`, which materializes by request, CollectAll without `report_item_error` must retain every item failure in source order in the outcome's `failures`; with a reporter, failures go to the reporter and `failures` stays empty. There is no hidden cap on retained failures; `max_items` is the explicit protective limit. A supplied reporter must be checked even when the selected policy is Ignore. Item reporters must receive `E`; callback reporters must receive the attempted input and `F`. Reporters must return `None`, either synchronously or asynchronously. Async reporting must complete before another item is requested or callback is invoked. Fallible reporters are not part of this design: a `Result`-returning reporter must be refused, so its error is never silently ignored.

Item policies must not govern source advancement failures. Any outer source failure must stop the consumer, preserve pending values, and return a typed source failure. Generic consumers must not repoll after that failure. A source may implement recovery internally according to its documented cursor and cancellation contracts, consistent with RFC 115. Explicit result items are how a source exposes independent, recoverable records to a policy-aware consumer.

### Batching, progress, and terminal outcomes

Batch size must be a positive integer, checked before the first source advancement. A batch must contain up to that many successful values in source order. Item errors must not occupy successful-value slots. A full batch must be delivered before requesting another source item, and ordinary exhaustion must deliver a final nonempty partial batch. The consumer must never invoke the callback with an empty batch.

FailFast, a terminal source failure, and cancellation must not be treated as ordinary exhaustion or automatically flush a pending partial batch. A callback failure under CollectAll or Ignore must move on to the next batch after applying its reporting policy. Even when delivery could have partially succeeded, the user must be allowed to choose either continuation policy; generic consumption must not impose mandatory stopping on write errors.

The terminal outcome must distinguish normal exhaustion from stopping with a typed failure. A failure must identify its stage and preserve its typed error; an error string alone is insufficient. Within each consumer, all policy values must share the same outcome type. The declarations are:

```incan
pub model Progress:
    pub seen: int                # items read from the source, including Err items
    pub delivered: int           # values whose callback succeeded; for collect, values collected
    pub item_failures: int       # Err items, whether reported, ignored or stopping
    pub callback_failures: int   # failed callback calls, whether reported, ignored or stopping

pub enum ConsumptionFailure[S, E, F]:
    Source(error: S)             # the source failed to advance
    Item(error: E)               # a yielded Err item under FailFast
    Callback(error: F)           # a failed callback under FailFast
    Cancelled                    # the stop token was cancelled
    Deadline                     # the deadline passed
    Limit(max_items: int)        # the protective item limit was reached

pub enum Attempted[T]:
    Nothing                      # no callback held an input when consumption stopped
    Retained(inputs: list[T])    # inputs a borrowing callback held, returned in source order
    Consumed                     # an Owned[T] callback took the input

pub enum Consumption[T, S, E, F]:
    Completed(progress: Progress)
    Stopped(progress: Progress, failure: ConsumptionFailure[S, E, F], pending: list[T], attempted: Attempted[T])

pub enum Collected[T, S, E]:
    Completed(values: list[T], failures: list[E], progress: Progress)
    Stopped(values: list[T], failures: list[E], progress: Progress, failure: ConsumptionFailure[S, E, None])
```

`S` is the source's advancement error, `E` the item error and `F` the callback error. A stage that cannot fail, such as the source of an infallible iterator or the callback of `collect`, must use `None` as its type argument, and its case must never be produced.

`delivered` must count values whose callback returned success, including the size of successful batches; for `collect`, it must instead count successfully collected values. A failed batch must not be counted as delivered, even if its callback performed partial external work. An empty source must complete with zero progress and no callback invocation. A run that reaches exhaustion after reporting or ignoring errors must complete with failure counts rather than pretending every item succeeded; completion does not mean an error-free run.

`pending` must hold, in source order, the successful values the consumer read but never handed to the callback, such as a batch buffered before an item or source failure. `attempted` must hold what the callback had at the stop: the batch or value of a failed FailFast callback, or the input of a callback dropped by a stop request or deadline. Already successful deliveries must not appear in either. Delivered external effects must remain intact; retaining attempted inputs must not be described as proof that they are unsaved or safe to replay.

The consumer must own each input until its callback has finished with it. Whether a callback borrows or takes its input must follow its checked parameter contract under RFC 129 and RFC 164. A callback that borrows the input, through duckborrowing or an explicit `Borrowed[T]`, must leave it with the consumer, which returns it as `Attempted.Retained` when the callback fails or is stopped. A callback declaring `Owned[T]`, or whose use requires ownership, takes the input; its failure must be reported as `Attempted.Consumed`, and a callback that needs the input back must return it inside its own error value. The consumer must not require `T with Clone` or add a hidden copy. A borrowing callback that would retain the input beyond its call must be refused with a diagnostic naming the ownership conflict.

`collect` must be an explicit materializing terminal. It must return the collected values on normal exhaustion and preserve the partial collection when stopping, through `Collected`.

### Synchronous sources

The same consumers and outcome types must be available on synchronous sources: an `Iterator` of `Result` items and a `FallibleIterator`. `for_each`, `for_each_batch`, and `collect` there must take the same options, follow the same policy, progress, pending, attempted, and limit rules, and return the same `Consumption` and `Collected` types, with synchronous callbacks and no `stop` or `deadline` suspension. `for_each_concurrent` stays asynchronous. On a synchronous source, a call without any consumer option keeps RFC 115's existing meaning, so a plain `collect()` still returns the first error; passing any consumer option, such as `item_errors=`, selects the policy-aware form. One consumption vocabulary therefore serves both kinds of source, and a library such as `std.json` uses it rather than defining its own collector.

Non-materializing consumers must not silently retain all successful values or all errors. They may retain a pending value or batch and the inputs in-flight callbacks borrow, plus progress and terminal failure information. Source buffering, error payloads, reporters, and callbacks may retain additional memory under their own contracts. On an unbounded source, CollectAll must report incrementally and may never reach a terminal summary.

### Concurrent consumption

`for_each_concurrent(callback, limit=n, ...)` must accept an async callback and a positive `limit`, checked before the first source advancement, plus the options of `for_each`. A synchronous callback must be refused with a diagnostic pointing to `for_each`. It must return a `ConcurrentRun[T, S, E, F]` without advancing the source.

The run must keep at most `limit` callbacks in flight, and it must read a new source item only when a slot is free, so it never holds read but unstarted inputs. Err items must be handled by the item policy without occupying a slot. Callbacks must start in source order and run on the consumer's own task, as `join` branches do; the run must not spawn tasks or claim parallel execution. The limit has the meaning of a `Semaphore` with `limit` permits, each held for one callback's duration. Callbacks may finish in any order; reporters must run in that completion order and receive the input with its error. The run must not buffer results to restore source order.

The run must start no further callback once a FailFast failure is observed, `shutdown()` is called, or its `stop` token or `deadline` fires. The async `outcome()` must return the `Consumption` outcome as soon as it is known. Under FailFast it must return at the first observed failure, without waiting for the other in-flight callbacks, and its `attempted` must hold that failed callback's input. Otherwise it must return after exhaustion, once every callback has finished.

The async `shutdown()` must stop the callbacks still in flight and complete once every one has been dropped under its own cancellation contract. It must return the inputs those callbacks borrowed, as a `list[T]` in source order; inputs taken by `Owned[T]` callbacks are not returned. After a run that completed normally, it must return an empty list. While the caller awaits neither operation, in-flight callbacks make no progress, as with any operation that is not being awaited. Dropping the run must drop its in-flight callbacks without returning their inputs.

### Watch, broadcast, and notification primitives

The foundation must provide `watch[T with Clone](initial)` returning `Tuple[WatchSender[T], WatchReceiver[T]]`, `broadcast[T with Clone](capacity)` returning `Tuple[BroadcastSender[T], BroadcastReceiver[T]]`, and `Notify`. `std.async.channel` must re-export the first two with their endpoint and error types, and `std.async.sync` must re-export `Notify`. These must remain distinct types with distinct promises; a bounded channel must not be renamed to provide them.

A watch must hold exactly one current value. `send(value)` must replace it and return `Err(SendError)` when no receiver remains. `latest()` must return a copy of the current value. The async `changed()` must complete once a value newer than the one this receiver last saw is available, coalescing intermediate values, and must return `Err(RecvError)` once the sender is gone. `sender.subscribe()` must create another receiver that starts at the current value.

A broadcast must require a positive `capacity`, checked at construction, and must deliver every sent value to every subscriber that existed when it was sent, in send order. `send(value)` must return the number of subscribers reached, or `Err(SendError)` when none remain. The async `recv()` must return `Result[T, BroadcastRecvError]`. A subscriber more than `capacity` values behind must receive `Err(BroadcastRecvError.Lagged(missed))` with the exact number of values it missed and then continue from the oldest value still buffered; it must never receive a fabricated or reordered value. `Err(BroadcastRecvError.Closed)` must be returned once every sender is gone and the subscriber has drained its buffer. `sender.subscribe()` must create a subscriber that receives values sent after it subscribed.

`Notify()` must carry no payload. The async `notified()` must complete once it is notified. `notify_one()` must wake one waiter, or store a single permit when none is waiting, so repeated calls with no waiter coalesce into one wake-up. `notify_waiters()` must wake every current waiter and store no permit.

Dropping a pending `changed()`, `recv()` or `notified()` must lose no value, change or permit; a permit delivered to a dropped `notified()` must pass to another waiter. Closure and lag must be observable through these typed results, never inferred from silence.

### Deadlines, intervals, and the virtual test clock

Deadlines must use the existing monotonic `Instant` that `std.datetime` exposes; this RFC must not add a second instant type. Because consumers accept `deadline=`, that `Instant` must be a `std.primitives` declaration that `std.datetime` re-exports with the same identity. The foundation must provide the async `sleep_until(deadline)` and the async `timeout_at(deadline, task)` returning `Result[T, TimeoutError]` with the existing `TimeoutError`; `std.async.time` must re-export both and add the conveniences `deadline_after(seconds)` and `deadline_after_ms(milliseconds)` returning an `Instant`. A deadline already past must time out without starting the task. Every operation given the same deadline must share that one absolute budget.

`interval(seconds, missed=MissedTick.Skip)` and `interval_ms(milliseconds, missed=...)` must require a positive period and return an `Interval` whose async `tick()` returns the `Instant` of the tick. The first tick must complete immediately; later ticks are due at whole multiples of the period after creation. When a tick is late by more than one period, `MissedTick.Skip` must wait for the next due multiple and drop the missed ticks, `MissedTick.Burst` must deliver the missed ticks immediately until caught up, and `MissedTick.Delay` must schedule the next tick one period after the late one. Skip is the default.

`std.testing` must expose a `virtual_clock` fixture providing a `VirtualClock`. It must be available only in test code. It must act on the foundational clock: under the fixture, `Instant.now()` and every timer, including consumer deadlines, must read the test's virtual clock; the wall clock (`SystemTime`) must not be affected. Virtual time must not move on its own until every task of the test is waiting on a timer, and must then jump to the earliest due timer. The async `advance(seconds)` must move it forward explicitly, completing due timers in deadline order. A test whose runtime cannot provide virtual time must be rejected with a diagnostic when the test is built, rather than silently use real time. Virtual time must not simulate external I/O or make task scheduling deterministic.

### Layers, modules, and prelude

Under RFC 168, `std.primitives` must not depend on a higher-level standard package, including through methods a trait provides. The consumers are methods of the async iteration protocols, so the protocols and every type those methods use must be foundations:

- **Foundations in `std.primitives`:**
    - `AsyncIterable`, `AsyncIterator` and `AsyncFallibleIterator`, with `map`, `for_each`, `for_each_batch`, `for_each_concurrent` and `collect`;
    - `ErrorPolicy`, `Consumption`, `Collected`, `ConsumptionFailure`, `Attempted`, `Progress` and `ConcurrentRun`;
    - `CancellationToken` and `Run`;
    - `Notify`, and `watch` and `broadcast` with their endpoints and `BroadcastRecvError`;
    - `sleep_until`, `timeout_at`, `interval`, `interval_ms`, `Interval` and `MissedTick`;
    - the monotonic `Instant` and the clock that timers and the virtual clock act on;
    - the errors these return: `SendError`, `RecvError` and `TimeoutError`.
- **Compositions in `std.async`:** `join` and `JoinSlot`; `TaskGroup`, `TaskOutcome`, `ShutdownReport` and `TaskGroupFailure`; `deadline_after` and `deadline_after_ms`; and the prelude. `std.testing` adds the `virtual_clock` fixture and `VirtualClock`.

The grouping of foundations into `std.primitives` modules follows RFC 168. Applications import every name from `std.async`:

- `std.async.iter` (new): the protocols, the consumer outcome types, `ConcurrentRun` and `ErrorPolicy`.
- `std.async.join` (new, beside `std.async.race`): `join`, `JoinSlot`.
- `std.async.cancel` (new): `CancellationToken`, `Run`.
- `std.async.task` (existing): adds `TaskGroup`, `TaskOutcome`, `ShutdownReport`, `TaskGroupFailure`.
- `std.async.channel` (existing): adds `watch`, `WatchSender`, `WatchReceiver`, `broadcast`, `BroadcastSender`, `BroadcastReceiver`, `BroadcastRecvError`.
- `std.async.sync` (existing): adds `Notify`.
- `std.async.time` (existing): adds `deadline_after`, `deadline_after_ms`, `sleep_until`, `timeout_at`, `interval`, `interval_ms`, `Interval`, `MissedTick`.
- `std.testing` (existing): adds `virtual_clock` and `VirtualClock`.

A `std.async` module must re-export a foundation with its identity and must not redefine it. The existing `SendError`, `RecvError`, `TimeoutError` and `std.datetime.Instant` must become re-exports of their foundations rather than parallel types. `ErrorPolicy` must have one definition, in `std.primitives`; `std.validation` must re-export it for RFC 122, as `std.async.iter` and the prelude do, and no consumer may define a parallel policy enum.

`std.async.prelude` must re-export, beside its existing names, `join`, `JoinSlot`, `TaskGroup`, `TaskOutcome`, `CancellationToken`, `Run`, `AsyncIterable`, `AsyncIterator`, `AsyncFallibleIterator`, `Consumption`, `Collected`, `ErrorPolicy`, `deadline_after`, `sleep_until`, `timeout_at`, `interval`, `Notify`, and the watch and broadcast constructors with their endpoint and error types, as it already does for the existing channels. Progress, failure-detail, run, report and missed-tick types are imported from their modules.

`async for` must resolve its protocols without an import. The `std.async` import and the foundational async import must activate the same vocabulary, without competing definitions; importing an unrelated primitive must not activate it or require an async runtime. Compiler and platform code must be able to author an async source, loop over it, and consume it under any policy using `std.primitives` alone.

### Control flow, ownership, and cancellation

`break` must stop further advancement. `return`, body-level error propagation, and panic must leave the loop under their existing control-flow rules. None of these exits may trigger an extra fetch merely to drain the source. Ordinary loop ownership and binding rules must apply under RFC 129; the syntax must not promise that a named source is reusable after the loop independently of those rules.

The pending advancement must retain access to its iterator state while suspended. Concurrent advancement through another alias must be rejected when it conflicts with that access. An iterator must not gain `Send`, `Static`, or other runtime capabilities merely because it appears in an async loop; the enclosing async execution contract must check those requirements where needed.

If cancellation drops the pending advancement, the loop must stop advancing under the existing async runtime cancellation rules. It must not resume or retry that operation automatically. Whether cancellation consumed data, advanced a remote cursor, or performed an external effect remains part of the source's cancellation contract. In particular, dropping a local future must not be presented as undoing an already transmitted network request.

Leaving an async loop must not implicitly await a close hook, unsubscribe operation, flush, or transaction rollback. Existing ownership-based synchronous cleanup must still run under its normal rules. A resource needing asynchronous shutdown must expose an explicit operation, and may also adopt RFC 094's `AsyncContextManager` so callers can scope it with `async with`. RFC 094 then governs its exit, including cancellation and a body failure that is already propagating; plain iteration must not establish those guarantees.

Async adapters and consumers must preserve those same cancellation boundaries. Dropping a pending consumer future must not imply that a stopped outcome is returned to its caller or that its pending values remain observable; a `stop` token or `deadline` is the way to stop a consumer and still receive its outcome. ErrorPolicy must not turn cancellation or panic into an ordinary item failure. Cancellation during a callback or reporter must not trigger an automatic retry, partial-batch flush, awaited cleanup, or rollback. Stronger recoverable-cancellation behavior requires a separate explicit ownership and resource contract.

### Diagnostics and tooling

The compiler must distinguish unsupported async iteration, missing async activation, use outside an async function, a missing fallible header marker, incompatible error propagation, ambiguous protocols, and setup-versus-advancement ambiguity. Diagnostics should identify the source's selected item and error types and suggest a separate setup binding where appropriate.

Generated documentation, manifests, and editor tooling must preserve the async status of the advancement method, inferred member roles, selected source protocol, and propagated error type. Hover and semantic queries should explain whether the header's `?` handles setup or advancement.

Adapter and consumer diagnostics must also distinguish a synchronous mapping callback from an async callback, result items from source errors, unsupported callback ownership, missing CollectAll reporters, incompatible reporter signatures, and invalid policy options. Metadata must preserve these distinctions across library boundaries rather than rediscovering them through callback names or backend-only types.

## Design details

### Existing async foundation and prior art

The existing public `std.async` surface already includes spawning, blocking-task offload, task handles, yielding, sleep and relative timeouts, race, bounded and unbounded channels, one-shot channels, mutexes, read/write locks, semaphores, and barriers. `timeout_join` already distinguishes deadline expiry from cancellation by returning a live handle. `std.datetime` already provides a monotonic `Instant`. These are foundations to compose, not functionality this RFC claims to introduce, and the additions reuse their types and modules rather than defining parallel versions. RFC 168 places the foundational parts, including the iteration protocols and their consumers, in `std.primitives` beneath these modules.

The sections below give the prior art for each addition specified above.

#### Joins and task groups

The prior art for plain joining is Tokio's [join](https://docs.rs/tokio/latest/tokio/macro.join.html): independent branches are multiplexed on the current task, and all results are awaited. Tokio's [try_join](https://docs.rs/tokio/latest/tokio/macro.try_join.html) illustrates early error completion. Incan's direction uses ErrorPolicy across operations rather than adopting Rust macro syntax or reducing every failure to one early error value.

A dynamic group of spawned tasks is a separate capability from a fixed heterogeneous join. Tokio's [JoinSet](https://docs.rs/tokio/latest/tokio/task/struct.JoinSet.html) provides same-output-type task collection in completion order. It aborts tasks on drop; its explicit shutdown operation aborts and then awaits their termination. These distinctions matter for an Incan task-group API: ownership, completion order, failure policy, and whether exiting waits for cleanup must be explicit. A task group must not be advertised as providing borrowed scoped-task lifetimes merely because it owns task handles.

Existing Incan JoinHandles detach on drop. The proposed task group instead owns its members, requests cancellation on drop, and requires explicit detachment for independent continuation. Its awaited shutdown requests cancellation and confirms termination. This follows the distinction between Tokio JoinSet destruction and awaited shutdown while retaining Incan's own failure-reporting contract. Abort requests differ from completed shutdown; running blocking tasks cannot generally be interrupted. Task-group outcomes are delivered in observed completion order with task identity retained. Like Tokio's `JoinError`, a member panic is reported as that member's outcome rather than tearing down the consumer. Python's [asyncio.TaskGroup](https://docs.python.org/3/library/asyncio-task.html#task-groups) is the prior art for the `async with` form: leaving the block normally waits for the members, and their failures surface together. Unlike Python, a member failure does not cancel its siblings here; the explicit contract above keeps that the caller's choice.

#### Cooperative cancellation

Task abortion and graceful shutdown serve different purposes. A shared cancellation request lets a worker stop accepting input, finish or abandon its current unit under an application contract, and await its own cleanup. The prior art is [tokio-util CancellationToken](https://docs.rs/tokio-util/latest/tokio_util/sync/struct.CancellationToken.html). Its `new`, `child_token`, `cancel`, `is_cancelled`, `cancelled` and `run_until_cancelled` correspond one to one with the vocabulary above; `run_until_cancelled` returns `Option`, where Incan's `Run` names the two cases. Go's `context.WithCancel` and .NET's linked `CancellationTokenSource` use the same downward-only parent/child propagation. Python's asyncio instead injects cancellation into a task, which is closer to task abort than to a cooperative request.

Cancellation requests propagate downward through explicitly linked parent/child relationships, without automatically traveling upward or across to siblings. Requesting cancellation must not imply that a worker has finished. ErrorPolicy.Ignore must not suppress shutdown requests.

#### Bounded concurrent processing

Sequential `for_each` and `for_each_batch` await one callback at a time. `for_each_concurrent` is the explicit operation that overlaps a bounded number of callbacks. The Rust futures crate's [for_each_concurrent](https://docs.rs/futures/latest/futures/stream/trait.StreamExt.html#method.for_each_concurrent) is the prior art: it runs up to a limit of callbacks on the current task, without spawning. Existing bounded channels and semaphores remain available primitives, but they do not by themselves define a generic consumer's ordering, buffering, or failure contract, which is why the run states them.

Delivery follows completion order, because waiting to restore source order would hold finished work behind a slow early item and could buffer without bound. The run reports a FailFast failure immediately and stops in-flight work as a separate step, matching the task-group split between an outcome and awaited shutdown. Stopping in-flight callbacks cannot make their previous effects disappear. No concurrency option is added to the sequential consumers.

#### State observation, fan-out, and notifications

The public async API did not expose watch channels, broadcast channels, or a general Notify primitive. A notification primitive is used internally, which is not equivalent to a checked public Incan API. Tokio's [synchronization documentation](https://docs.rs/tokio/latest/tokio/sync/index.html) distinguishes these capabilities, and the contracts above follow it: watch exposes current state, broadcast distributes messages to subscribers with a typed lag result, and Notify wakes a waiter without carrying a message payload. Tokio also places Notify beside its semaphore and barrier, as `std.async.sync` does here.

Typical uses are live configuration, progress subscriptions, and worker wakeups. The contracts distinguish missed intermediate states, lagging subscribers, bounded buffering, closure, and notification coalescing, so an ordinary bounded channel is not presented as providing all of them through a name change. Watch and broadcast values are copied for each reader, so they require `Clone`, as the existing `Mutex` and `RwLock` do.

#### Timers, deadlines, and deterministic async tests

Relative sleeps and timeouts already exist, and `std.datetime` already has a monotonic `Instant`. The additions are absolute deadlines on that `Instant`, repeating intervals with a missed-tick policy, and a controllable test clock. Tokio's [time APIs](https://docs.rs/tokio/latest/tokio/time/index.html) provide the prior art, including `timeout_at`, `sleep_until`, and the Burst, Delay and Skip choices of `MissedTickBehavior`. An interval is not simply sleeping after each completed unit of work, and a shared absolute deadline prevents nested operations from each receiving a fresh full timeout budget. Tokio's default is Burst; Incan defaults to Skip so that a delayed handler does not overload a service with catch-up ticks.

A test clock lets applications exercise retries, batching deadlines, and shutdown without depending on wall-clock sleeps. Tokio's [pause](https://docs.rs/tokio/latest/tokio/time/fn.pause.html) and [advance](https://docs.rs/tokio/latest/tokio/time/fn.advance.html) are the prior art, including the jump to the next timer when every task is idle, but paused time requires its test utilities and current-thread runtime. The `virtual_clock` fixture therefore rejects an unsupported runtime when the test is built, rather than promising that those controls work unchanged in every executor configuration. A controlled clock does not make arbitrary task scheduling deterministic.

### Python familiarity and Incan differences

Python's `async for` obtains an async iterator and awaits its next-item operation before each body execution. Python also provides async generator functions, which are not implied by adding the loop statement alone. The official [async for reference](https://docs.python.org/3/reference/compound_stmts.html#the-async-for-statement) and [async generator reference](https://docs.python.org/3/reference/expressions.html#asynchronous-generator-functions) provide the principal prior art.

Incan keeps its `Option` exhaustion and typed `Result` failure conventions. It does not add Python's `StopAsyncIteration` exception to source programs. The separate `?` marker preserves RFC 115's distinction between an infallible item source and failure while obtaining an item.

### One async method contract

The protocol declares an async method whose source-level result is an `Option` or `Result`. Loop lowering awaits that method according to Incan's existing direct-call rules. This avoids introducing a user-visible future storage field, a second `Awaitable` adoption model, or a requirement that library authors write a backend polling API themselves.

The protocols are `std.primitives` declarations, re-exported from `std.async.iter` for authors who adopt them; `async for` resolves them without an import.

### Composition and generators

RFC 088 adapters and RFC 115 fallible adapters do not automatically apply to async protocols. This RFC explicitly introduces async-source `map` with a synchronous or async callback and async terminals that can await their callbacks. Further adapters require their own checked contracts. Concurrency remains an explicit operation, `for_each_concurrent`, with its own ordering and capacity contract.

RFC 006's synchronous `yield` rules remain unchanged. Async generator functions would provide a useful source-authoring convenience, but they need an explicit return type and a decision about propagating errors from a suspended producer. They remain outside this revision; direct Incan-authored protocol implementations supply sources without depending on async generators.

### Result items and source failures

A parser can fail on one message while the next message remains available. Representing that failure as a `Result` item lets the consumer choose how to proceed. A failure of the reader itself is a different channel, represented by `AsyncFallibleIterator`. This keeps the recovery boundary concrete without changing RFC 115 or teaching generic adapters to guess whether another poll is safe.

Policies belong to consumption. `map` keeps its ordinary transformation meaning, so the same lazy result-item pipeline can be consumed under different policies without hiding an error-handling decision inside the parser. This does not imply that a consumed single-pass source can be reused. Independent policies at multiple intermediate transformations would require a separate stage-composition contract.

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
- **Give `map` an error policy:** changes its meaning when the callback returns a `Result` and hides the distinction between result values and failed advancement. This design keeps mapping ordinary and makes handling explicit at consumption.
- **Return a different container for each policy:** can be useful for a specialized eager API, but complicates runtime policy selection and loses the incremental processing model. The async consumers use one outcome type per operation.
- **Always stop on failed delivery:** prevents legitimate best-effort processing. The callback policy must allow continued processing, with no promise of rollback or retry safety.
- **Automatically commit a partial batch on failure:** makes a stopping decision trigger an additional external effect. Stopping preserves pending values and leaves further delivery to the caller.
- **Collect every error until the end:** may consume unbounded memory or never deliver diagnostics for an unbounded source. CollectAll reports incrementally instead.
- **Require `T with Clone` to retain attempted inputs:** easy to implement, but it copies every item and rules out large values and open handles. Borrowing callbacks let the consumer keep the original.
- **A concurrent consumer that drains every callback before reporting:** simpler, but it turns FailFast into wait-for-all. The run reports first and stops in-flight work as a separate step.
- **Restore source order in concurrent delivery:** holds finished work behind a slow early item and can buffer without bound.
- **One generic broadcast channel for every fan-out pattern:** hides whether readers may miss values. Watch, broadcast and Notify each state their own promise.
- **A second `Instant` in `std.async.time`:** would give two monotonic clocks with different meanings. Deadlines reuse the existing `Instant`, whose definition becomes the foundational one.
- **Consumers as `std.async.iter` functions over policy-free primitive protocols:** keeps `ErrorPolicy` and the outcome types above the foundation, but replaces `source.map(f).for_each(...)` chains with `for_each(source.map(f), ...)` calls. A `std.async` sub-trait that adds the consumer methods fails differently: `map` returns a primitive adapter, so the chain still breaks. Placing the consumers and their types in the foundation keeps the chain and gives compiler and platform code the same consumers.
- **A separate `ErrorPolicy` per consumer or per module:** the same three choices would drift apart. RFC 122's enum is the single definition.
- **A separate `map_async` operation:** names the effect, but splits one familiar operation in two. The async overload is selected from the callback's declared type instead.

## Drawbacks

There are now synchronous and asynchronous iteration protocols, each with an explicit fallible form. Readers must learn which operation can wait and which can fail. Reusing `?` helps maintain consistency, but setup results containing streams still need a separate binding.

Async iteration can look lazy even when a source buffers eagerly or runs an independent producer. Documentation must distinguish the loop's sequential demand from the source's internal behavior. Early exit stops further loop requests but does not guarantee asynchronous shutdown or reversal of external effects.

The proposal depends on the revised associated-type and callable designs. Source authors write explicit stateful implementations, because async generators and further adapters are not part of this design. Compiler and editor support must account for suspension, mutable iterator access, typed propagation, and cleanup without inventing stronger cancellation guarantees than the source provides.

The consumer surface adds policy options, reporters, progress, and recovery values. A callback that takes ownership of its input cannot have that input returned on failure unless it returns the input itself. Reporting every error can dominate processing cost; Ignore deliberately avoids reporting.

The library grows by several primitives at once. After `for_each_concurrent` reports a FailFast failure, its in-flight callbacks make no progress until the caller stops them, because they run on the consumer's own task. The virtual clock works only on runtimes that can pause time, so some test configurations cannot use it. A completed outcome can include handled failures, so users must distinguish exhaustion from complete success. Batch delivery may have partial effects, and generic library code cannot infer which values were committed by a failed callback.

## Layers affected

- **Parser / AST:** import-activated `async for`, existing loop patterns, and the explicit fallible header form.
- **Typechecker / Symbol resolution:** async protocol selection, inferred item and error members, async method and callback validation, result-item consumer overloads, reporter requirements, and error compatibility.
- **Ownership / async checking:** suspended iterator and callback receiver access, prevention of overlapping advancement, retained callback input, control-flow exits, and existing execution capability requirements.
- **IR Lowering / Emission:** sequential async advancement and callback invocation with ordinary exhaustion, typed propagation, and policy-aware terminal outcomes, preserving existing await and cleanup semantics.
- **Stdlib / Runtime:** `std.primitives` foundations for the async protocols and their consumers, `ErrorPolicy` and the outcome types, cancellation tokens, watch, broadcast and `Notify`, timers, the monotonic `Instant` and its clock, and their errors; the new `std.async.iter`, `std.async.join` and `std.async.cancel` modules; `TaskGroup` and its outcome types in `std.async.task`; re-exports in the existing `std.async` modules, `std.datetime` and `std.validation`; deadline conveniences in `std.async.time`; the `virtual_clock` fixture in `std.testing`; and the prelude.
- **Formatter:** stable formatting for async loop headers and proposed protocol declarations.
- **Library manifests / LSP / Tooling:** async advancement and callback contracts, selected source protocols, inferred types, consumer policies, propagation boundaries, and diagnostics.

## Design Decisions

- Cooperative cancellation follows explicitly configured parent/child links downward to descendants. Cancelling a child does not by itself cancel its parent or siblings, and calls or task spawning do not implicitly create cancellation links. Failure policy remains separate; a stop request is not proof of termination or rollback.
- Task groups deliver identified outcomes in observed completion order, without holding completed work behind earlier unfinished submissions. Concurrently ready tasks have no promised deterministic tie order. Submission-order reconstruction is explicit, while fixed join retains its argument-order tuple.
- Task groups own their members, request cancellation of remaining async tasks on drop, and require explicit detachment for independent continuation. The proposed awaited shutdown requests cancellation and waits for termination; ordinary drop does not wait. Existing standalone JoinHandle semantics remain unchanged, immediate FailFast reporting remains immediate, and awaited scope exit uses RFC 094's `async with`, as the scoped task-group decision specifies.
- Policy-aware join preserves one independently typed result position per input operation in argument order, including completed successes after an early failure. Positions distinguish success, retained failure, ignored failure, and unfinished work. Ignore discards its error payload without removing the position. Runtime policy selection retains the same static outcome type within this form.
- FailFast drops directly owned pending join operations without awaiting shutdown. Previously spawned tasks retain the existing handle-detach contract unless an explicit task owner supplies cancellation. Unfinished identifies absence of an obtained result rather than proving the underlying work has stopped. Task-group ownership remains a distinct surface.
- FailFast returns immediately upon the first observed branch failure without draining unfinished branches. Unfinished work is distinct from failure and does not imply a resumable operation or absence of external effects.
- An explicitly spawned task captures the active validation policy, or the default, at spawn and keeps it. The parent's later changes do not reach it, its own scopes restore its own policy, and no other authority is inherited with the policy.
- Policy-aware join is `join(..., errors=policy)`, with one `JoinSlot[T, E]` per argument: `Succeeded(value)`, `Failed(original)`, `Ignored` or `Unfinished`. Plain join keeps unchanged outputs. A branch panic propagates as a panic of the join, never as a failed slot.
- `TaskGroup[Id, T, E]` exposes `spawn(id, operation)`, `next()`, `detach(id)` and `shutdown()`. Outcomes are identified and delivered once; a member panic is its own outcome; a member failure does not cancel siblings by itself. `shutdown()` reports termination and every undelivered outcome; an interrupted shutdown claims nothing and drop cancellation still applies.
- Cooperative cancellation uses `CancellationToken` with `child_token()`, `cancel()`, `is_cancelled()`, async `cancelled()` and async `run(operation)` returning `Run.Completed(value)` or `Run.Cancelled`, following tokio-util's CancellationToken. Deadlines and application failures stay separate outcomes, and tokens never own or abort tasks.
- Bounded concurrency is `for_each_concurrent(callback, limit=n, ...)`, returning a run. `outcome()` reports at once, including a FailFast failure while other callbacks are in flight; `shutdown()` stops those callbacks and returns the inputs they borrowed. Callbacks run on the consumer's task, start in source order, finish in completion order, and are never reordered by buffering.
- Message patterns use three distinct primitives: `watch` for the latest value with coalescing, bounded `broadcast` with a typed `Lagged(missed)` result, and payload-free `Notify` whose stored permits coalesce. Watch and broadcast join the existing channels; Notify joins `Semaphore` and `Barrier`.
- Deadlines use the existing monotonic `Instant`, with `deadline_after`, `sleep_until` and `timeout_at` returning the existing `TimeoutError`. Intervals take an explicit `MissedTick` policy defaulting to Skip. The test-only `virtual_clock` fixture jumps to the next timer when every task waits, and an unsupported runtime is a test-build error rather than a fall-back to real time.
- Under RFC 168, the async protocols keep their consumer methods, so the protocols, the consumers, `ErrorPolicy`, the outcome types, cancellation tokens, watch, broadcast, `Notify`, timers, the monotonic `Instant` and their errors are `std.primitives` foundations. Joins, task groups, deadline conveniences and the prelude are `std.async` compositions. `std.validation`, `std.datetime` and the `std.async` modules re-export foundations with their identity, so `ErrorPolicy` and `Instant` each have one definition.
- Applications import from `std.async.iter`, `std.async.join`, `std.async.cancel`, and the existing `std.async.task`, `channel`, `sync` and `time` modules, plus `std.testing` for the clock fixture. Compiler and platform code can use the foundations without `std.async`.
- Consumers return `Consumption.Completed(progress)` or `Consumption.Stopped(progress, failure, pending, attempted)`, and `collect` returns `Collected` with its values and, under CollectAll without a reporter, every failure. `ConsumptionFailure` names the source, item, callback, cancellation, deadline, or limit stage with the original typed error; a stage that cannot fail uses `None`. `Progress` counts seen, delivered, item failures and callback failures. Every consumer accepts `stop=`, `deadline=`, and the protective `max_items=`; a `take(n)` prefix completes normally.
- Synchronous iterators of `Result` items and `FallibleIterator` sources get the same consumers and outcome types when a consumer option is passed; a plain call keeps RFC 115's meaning. Libraries such as `std.json` reuse this vocabulary instead of defining collectors.
- Async callbacks use RFC 153's `Callable[async (A) -> R]`, and overloads are selected from that declared type, never from a returned value.
- Consumers keep each input while the callback borrows it, so items need not be `Clone`. An `Owned[T]` callback takes its input, which is then reported as `Attempted.Consumed`.
- `map` accepts a synchronous or an async callback; the async one is awaited once per item, and an ordinary closure never becomes async from its surroundings.
- There is no `AsyncFallibleIterable`, matching RFC 115; a reusable fallible source returns a fresh iterator from a method.
- `async for` has no `else`, because ordinary loops have none; any loop `else` must cover both with one meaning.
- `TaskGroup` adopts `AsyncContextManager`. A normal exit waits for its members and returns undelivered failures and panics as `TaskGroupFailure`; an error or panic exit requests cancellation and awaits termination, with RFC 094 keeping both failures. Explicit `shutdown()` remains, and `async for` never awaits a source's shutdown.
