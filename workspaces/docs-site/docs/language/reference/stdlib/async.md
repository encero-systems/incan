# `std.async`

`std.async` provides task spawning, timeouts, races, channels, and synchronization primitives. Import individual modules for narrow dependencies or use `std.async.prelude` for the common surface.

All APIs that accept a future require the future to produce the declared result type. Public spawned tasks and race arms also require transferable, runtime-owned values through the `Send` and `Static` bounds shown in the signatures.

## Cancellation terms

| Term | Contract |
| --- | --- |
| Cancel-safe | Cancelling a pending wait does not consume a value or acquire a resource. |
| Cancel-safe but lossy | Cancelling does not complete the operation, but it can discard an owned value or the waiter's queue position. |
| Durable once spawned | The task continues after its handle is dropped. Dropping the handle loses the result but does not cancel the task. |

## `std.async.time`

```incan
from std.async.time import Duration, TimeoutError, TimeoutJoinOutcome
from std.async.time import sleep, sleep_ms, timeout, timeout_ms, timeout_join, timeout_join_ms
```

### Functions

| Signature | Contract |
| --- | --- |
| `async sleep(seconds: float) -> None` | Suspends for `seconds`. Negative values, NaN, and either infinity are treated as zero. The wait is cancel-safe. |
| `async sleep_ms(milliseconds: int) -> None` | Suspends for `milliseconds`. Negative values are treated as zero. The wait is cancel-safe. |
| `async timeout[T with (Send, Static), TaskFuture with RuntimeFuture[T]](seconds: float, task: TaskFuture) -> Result[T, TimeoutError]` | Returns `Ok(value)` when `task` finishes before the deadline and `Err(TimeoutError())` when the deadline expires. Negative values, NaN, and either infinity are treated as zero. Expiry or cancellation drops the supplied future. |
| `async timeout_ms[T with (Send, Static), TaskFuture with RuntimeFuture[T]](milliseconds: int, task: TaskFuture) -> Result[T, TimeoutError]` | Millisecond form of `timeout`. Negative durations are treated as zero. |
| `async timeout_join[T with (Send, Static)](seconds: float, handle: JoinHandle[T]) -> TimeoutJoinOutcome[T]` | Waits for spawned work without aborting it at the deadline. Negative values, NaN, and either infinity are treated as zero. A timeout returns the live handle. Cancelling this wait drops its owned handle and detaches the task. |
| `async timeout_join_ms[T with (Send, Static)](milliseconds: int, handle: JoinHandle[T]) -> TimeoutJoinOutcome[T]` | Millisecond form of `timeout_join`. Negative durations are treated as zero. |

### `Duration`

```incan
pub model Duration:
    pub secs: int
    pub nanos: int
```

`Duration` is a value object for application APIs. The timing functions above accept primitive seconds or milliseconds rather than `Duration`.

| Signature | Result |
| --- | --- |
| `Duration(secs: int, nanos: int)` | Direct field construction. It does not clamp or normalize either field. |
| `Duration.from_secs(secs: int) -> Duration` | Returns zero for `secs <= 0`; otherwise returns `(secs, 0)` exactly. |
| `Duration.from_millis(millis: int) -> Duration` | Returns zero for `millis <= 0`; otherwise returns `secs = millis // 1000` and `nanos = (millis % 1000) * 1_000_000`. |
| `Duration.from_secs_f64(secs: float) -> Duration` | Returns zero for nonpositive inputs and NaN. Positive inputs are split into whole seconds and fractional nanoseconds using floating-point arithmetic and saturating integer conversions. Positive infinity produces `secs = 9223372036854775807` and `nanos = 9223372036854775807`. |

The integer constructors preserve the full positive `int` value without converting through `float`. Their results satisfy `secs >= 0` and `0 <= nanos < 1_000_000_000`.

`from_secs_f64` converts the input to integer seconds, then converts `(secs - float(whole_seconds)) * 1_000_000_000` to integer nanoseconds. Large positive inputs can saturate either integer conversion, so this constructor does not guarantee normalized nanoseconds. Unlike the timer functions, it does not clamp positive infinity to zero.

### `TimeoutError`

`TimeoutError` implements `Error`.

| Method | Returns |
| --- | --- |
| `message(self) -> str` | `"operation timed out"` |
| `source(self) -> Option[str]` | `None` |

### `TimeoutJoinOutcome[T]`

| Variant | Payload | Meaning |
| --- | --- | --- |
| `Completed` | `T` | The task returned successfully before the deadline. |
| `JoinFailed` | `TaskJoinError` | The task finished before the deadline but could not be joined. |
| `TimedOut` | `JoinHandle[T]` | The deadline expired. The task is still running and the returned handle can be awaited or aborted. |

## `std.async.task`

```incan
from std.async.task import JoinHandle, TaskJoinError, spawn, spawn_blocking, yield_now
```

### Functions

| Signature | Contract |
| --- | --- |
| `spawn[T with (Send, Static), TaskFuture with RuntimeFuture[T]](task: TaskFuture) -> JoinHandle[T]` | Starts an async task. The task is durable once spawned. |
| `spawn_blocking[T with (Send, Static), TaskFn with RuntimeFnOnce[T]](task: TaskFn) -> JoinHandle[T]` | Schedules a no-argument callable on the blocking pool. Once work starts, it cannot be cancelled. |
| `async yield_now() -> None` | Yields to the runtime scheduler. The wait is cancel-safe. |

### `JoinHandle[T]`

Awaiting a handle produces `Result[T, TaskJoinError]`. Dropping it detaches the task. `handle.abort() -> None` requests cancellation of async work; for `spawn_blocking`, abort can only prevent work that is still queued.

A handle can be neither copied nor cloned. A `for` loop over a list of handles held by a local binding or by a parameter not marked `mut`, whose body awaits each handle, returns it, assigns it to another name, passes it to a call or places it in a new value, takes the handles out of the list. Reading the list inside the loop or after it, or running the loop again from an enclosing loop over a list built outside that loop, is refused with `INCAN-T0119`.

```incan
handles = [spawn(work()), spawn(work())]
for handle in handles:
    match await handle:         # takes each handle out of `handles`
        Ok(value) => println(value)
        Err(_) => println("join failed")
println(len(handles))           # refused: INCAN-T0119
```

### `TaskJoinError`

| Method | Returns |
| --- | --- |
| `message(self) -> str` | The runtime's join failure message. |
| `source(self) -> Option[str]` | `None` |
| `is_cancelled(self) -> bool` | Whether cancellation caused the join failure. |
| `is_panic(self) -> bool` | Whether a panic caused the join failure. |

## `std.async.race`

```incan
from std.async.race import RaceArm, arm, race, race_timeout
```

| Signature | Contract |
| --- | --- |
| `arm[T with Send, R with (Send, Static), TaskFuture with RuntimeFuture[T], OnWin with RuntimeRaceCallback[T, R]](awaitable: TaskFuture, on_win: OnWin) -> RaceArm[R]` | Packages one future and a callback. The callback receives the future's value only if this arm wins. |
| `async race[R with (Send, Static)](*arms: RaceArm[R]) -> R` | Polls arms concurrently and returns the winning callback result. Losing arms are dropped. Ready ties use source order. At least one arm is required. |
| `async race_timeout[T with (Send, Static), TaskFuture with RuntimeFuture[T]](seconds: float, task: TaskFuture) -> Option[T]` | Returns `Some(value)` before the deadline or `None` at the deadline. Negative values, NaN, and either infinity are treated as zero. Expiry or cancellation drops `task`; spawn it first when it must continue. |

`RaceArm[R]` is the packaged branch type consumed by `race`.

## `std.async.channel`

```incan
from std.async.channel import channel, unbounded_channel, oneshot
from std.async.channel import Receiver, RecvError, Sender, SenderPermit, SendError
from std.async.channel import OneshotReceiver, OneshotSender
```

!!! warning "Explicit channel type arguments"
    Current native coverage verifies inference-based constructor calls such as `channel(4)`, `unbounded_channel()`, and `oneshot()`, including their wrapper methods. Explicit constructor type application such as `channel[str](4)` remains a compiler limitation. Let subsequent sends, receives, or annotations infer `T`.

### Constructors

| Signature | Returns | Constraints |
| --- | --- | --- |
| `channel[T](buffer: int) -> Tuple[Sender[T], Receiver[T]]` | A bounded multi-producer, single-consumer channel. | `buffer <= 0` is normalized to capacity `1`. Sends wait when the buffer is full. |
| `unbounded_channel[T]() -> Tuple[Sender[T], Receiver[T]]` | An unbounded multi-producer, single-consumer channel. | Producers can grow memory without a capacity limit. |
| `oneshot[T]() -> Tuple[OneshotSender[T], OneshotReceiver[T]]` | A channel that carries at most one value. | The sender is consumed by its single send operation. |

### `Sender[T]`

`Sender[T]` is cloneable. All clones address the same channel.

| Method | Result and errors | Cancellation |
| --- | --- | --- |
| `async send(self, value: T) -> Result[None, SendError[T]]` | Returns `SendError` with the unsent value when the receiver is closed. | Cancel-safe but lossy: cancellation while waiting for capacity drops `value`. |
| `async reserve(self) -> Result[SenderPermit[T], SendError[None]]` | Reserves one bounded slot, or returns an error when the receiver is closed. Unbounded senders return an immediately usable permit. | Cancellation gives up queue position but owns no message value. |
| `try_send(self, value: T) -> Result[None, SendError[T]]` | Returns immediately. Fails when a bounded channel is full or any receiver is closed. | Synchronous. |
| `is_closed(self) -> bool` | Whether the receiver side is closed. | Synchronous. |
| `clone(self) -> Self` | Another sender for the same channel. | Synchronous. |

### `SenderPermit[T]`

| Method | Result and errors |
| --- | --- |
| `send(self, value: T) -> Result[None, SendError[T]]` | Uses the reserved slot. An unbounded permit can still return the value if the receiver closes after reservation. A permit is single-use. |

### `Receiver[T]`

| Method | Result | Cancellation |
| --- | --- | --- |
| `async recv(self) -> Option[T]` | Returns the next value, or `None` after the channel is closed and drained. | Cancel-safe; cancellation does not remove a message. |
| `try_recv(self) -> Option[T]` | Returns an available value immediately, otherwise `None`. `None` does not distinguish empty from closed. | Synchronous. |
| `close(self) -> bool` | Prevents further sends. Returns `false` when another cloned receiver currently owns the receiver state in `recv`. | Synchronous. |

### One-shot types

| Method | Result | Cancellation |
| --- | --- | --- |
| `OneshotSender[T].send(self, value: T) -> Result[None, T]` | Delivers the value, or returns it when the receiver is gone. | Synchronous. |
| `async OneshotReceiver[T].recv(self) -> Result[T, RecvError]` | Returns the value, or `RecvError` when the sender is dropped first. | Cancel-safe; cancellation does not consume the value. |

### Channel errors

| Type | Public data and methods |
| --- | --- |
| `SendError[T]` | `value: T` preserves the unsent value. `message() -> str` returns `"channel send failed"`; `source() -> Option[str]` returns `None`. |
| `RecvError` | `message() -> str` returns `"channel closed: no more messages"`; `source() -> Option[str]` returns `None`. |

## `std.async.sync`

```incan
from std.async.sync import Barrier, Mutex, RwLock, Semaphore
from std.async.sync import MutexGuard, RwLockReadGuard, RwLockWriteGuard, SemaphorePermit
from std.async.sync import SemaphoreAcquireError
```

`Mutex[T]` and `RwLock[T]` require `T with Clone` because guard reads return cloned values.

### Mutex

| Method | Result | Cancellation |
| --- | --- | --- |
| `Mutex[T].new(value: T) -> Mutex[T]` | Creates a cloneable mutex. | Synchronous. |
| `async Mutex[T].lock(self) -> MutexGuard[T]` | Waits for exclusive access. The guard releases the lock when dropped. | Cancellation does not acquire the lock, but loses queue position. |
| `Mutex[T].try_lock(self) -> Option[MutexGuard[T]]` | Returns a guard immediately or `None` when held. | Synchronous. |
| `MutexGuard[T].get(self) -> T` | Clones the guarded value. | Synchronous. |
| `MutexGuard[T].set(self, value: T) -> None` | Replaces the guarded value. | Synchronous. |

### Read-write lock

| Method | Result | Cancellation |
| --- | --- | --- |
| `RwLock[T].new(value: T) -> RwLock[T]` | Creates a cloneable read-write lock. | Synchronous. |
| `async RwLock[T].read(self) -> RwLockReadGuard[T]` | Waits for shared read access. | Cancellation loses queue position. |
| `async RwLock[T].write(self) -> RwLockWriteGuard[T]` | Waits for exclusive write access. | Cancellation loses queue position. |
| `RwLock[T].try_read(self) -> Option[RwLockReadGuard[T]]` | Returns a read guard immediately or `None`. | Synchronous. |
| `RwLock[T].try_write(self) -> Option[RwLockWriteGuard[T]]` | Returns a write guard immediately or `None`. | Synchronous. |
| `RwLockReadGuard[T].get(self) -> T` | Clones the guarded value. | Synchronous. |
| `RwLockWriteGuard[T].get(self) -> T` | Clones the guarded value. | Synchronous. |
| `RwLockWriteGuard[T].set(self, value: T) -> None` | Replaces the guarded value. | Synchronous. |

### Semaphore

| Method | Result | Cancellation |
| --- | --- | --- |
| `Semaphore.new(permits: int) -> Semaphore` | Creates a cloneable semaphore. Negative values are normalized to zero permits. | Synchronous. |
| `async Semaphore.acquire(self) -> Result[SemaphorePermit, SemaphoreAcquireError]` | Waits for one permit. The permit is returned automatically when dropped. Returns an error if the semaphore closes. | Cancellation loses queue position but does not acquire a permit. |
| `Semaphore.try_acquire(self) -> Option[SemaphorePermit]` | Returns a permit immediately or `None`. | Synchronous. |
| `Semaphore.available_permits(self) -> int` | Current unreserved permit count. | Synchronous. |

`SemaphoreAcquireError.message() -> str` returns `"failed to acquire semaphore permit: semaphore closed"`. Its `source() -> Option[str]` returns `None`.

### Barrier

| Method | Result | Cancellation |
| --- | --- | --- |
| `Barrier.new(count: int) -> Barrier` | Creates a cloneable reusable barrier. Counts less than or equal to zero are normalized to one participant. | Synchronous. |
| `async Barrier.wait(self) -> int` | Waits for the generation to fill and returns a unique slot in `0..count`. The slot supports leader selection but is not a chronological arrival index. | Cancellation before release withdraws the participant and frees its slot; the remaining participants still need a full active generation. |

## `std.async.prelude`

```incan
from std.async.prelude import *
```

The prelude re-exports:

| Module | Names |
| --- | --- |
| `std.async.time` | `sleep`, `sleep_ms`, `timeout`, `timeout_ms`, `timeout_join`, `timeout_join_ms`, `Duration`, `TimeoutError`, `TimeoutJoinOutcome` |
| `std.async.task` | `spawn`, `spawn_blocking`, `yield_now`, `JoinHandle`, `TaskJoinError` |
| `std.async.channel` | `channel`, `unbounded_channel`, `oneshot`, `Sender`, `Receiver`, `OneshotSender`, `OneshotReceiver`, `SendError`, `RecvError` |
| `std.async.sync` | `Mutex`, `MutexGuard`, `RwLock`, `RwLockReadGuard`, `RwLockWriteGuard`, `Semaphore`, `SemaphorePermit`, `SemaphoreAcquireError`, `Barrier` |
| `std.async.race` | `RaceArm`, `arm`, `race`, `race_timeout` |

`SenderPermit` is public in `std.async.channel` but is not re-exported by `std.async.prelude`.
