# std.async (reference)

This page documents the `std.async` API surface exposed by the standard library.
See the module source files for authoritative behavior:

- `crates/incan_stdlib/stdlib/async/time.incn`
- `crates/incan_stdlib/stdlib/async/task.incn`
- `crates/incan_stdlib/stdlib/async/channel.incn`
- `crates/incan_stdlib/stdlib/async/sync.incn`
- `crates/incan_stdlib/stdlib/async/select.incn`
- `crates/incan_stdlib/stdlib/async/prelude.incn`

## Interop notes

`std.async.time` and `std.async.select` use direct Rust interop calls (for example `tokio::time`) for their timer-related
operations rather than wrapping stdlib-runtime helper functions. The public signatures listed below remain unchanged.
`std.async.task`, `std.async.channel`, `std.async.sync`, and `std.async.prelude` still retain wrapper-style surfaces where behavior
depends on native Rust adapter contracts.

`Awaitable[T]` is the Incan-facing async protocol, but user-authored `rusttype ... with Awaitable[T]` bridges are
currently gated. Rust `Future` bridge generation needs compiler metadata for safe `Pin` projection and output mapping;
until that exists, write the adapter in Rust and expose it through the stdlib/runtime boundary.

## Cancellation vocabulary

Async APIs on this page use these contract terms:

| Term                    | Meaning                                                                                                                                     |
| ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| `cancel-safe`           | Cancelling a pending wait does not consume the value, acquire the resource, or otherwise complete the operation.                            |
| `cancel-safe-but-lossy` | Cancelling the wait does not complete the operation, but a value or queue position owned by that wait may be lost.                          |
| `not cancel-safe`       | Cancelling a pending wait can break the operation's coordination contract or leave other participants waiting.                              |
| `durable once spawned`  | Work continues after it is spawned unless it finishes or is explicitly aborted; dropping the handle detaches the work and loses the result. |

## Module: `std.async.time`

Import with:

```incan
from std.async.time import sleep, timeout, timeout_join, TimeoutError, TimeoutJoinOutcome
```

**Functions**:

| Function                                                                                    | Returns                   | Cancellation contract                                                                                                                                                                                                                                                                                                      |
| ------------------------------------------------------------------------------------------- | ------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `sleep(seconds: float) -> None`                                                             | `None`                    | `cancel-safe`                                                                                                                                                                                                                                                                                                              |
| `sleep_ms(milliseconds: int) -> None`                                                       | `None`                    | `cancel-safe`                                                                                                                                                                                                                                                                                                              |
| `timeout[T, TaskFuture](seconds: float, task: TaskFuture) -> Result[T, TimeoutError]`       | `Result[T, TimeoutError]` | Cancels the supplied future when the deadline expires; cancelling the timeout wait also drops the supplied future.                                                                                                                                                                                                         |
| `timeout_ms[T, TaskFuture](milliseconds: int, task: TaskFuture) -> Result[T, TimeoutError]` | `Result[T, TimeoutError]` | Cancels the supplied future when the deadline expires; cancelling the timeout wait also drops the supplied future.                                                                                                                                                                                                         |
| `timeout_join[T](seconds: float, handle: JoinHandle[T]) -> TimeoutJoinOutcome[T]`           | `TimeoutJoinOutcome[T]`   | Stops waiting when the deadline expires and returns the live handle in `TimedOut(handle)`; the task continues running unless explicitly aborted. If the `timeout_join()` wait itself is cancelled by an outer boundary, the helper-owned handle is dropped and the task is detached unless another completion path exists. |
| `timeout_join_ms[T](milliseconds: int, handle: JoinHandle[T]) -> TimeoutJoinOutcome[T]`     | `TimeoutJoinOutcome[T]`   | Millisecond form of `timeout_join`, with the same outer-cancellation caveat.                                                                                                                                                                                                                                               |

**Types**:

| Name                 | Description                                                                                       |
| -------------------- | ------------------------------------------------------------------------------------------------- |
| `TimeoutJoinOutcome` | Outcome type returned by durable timeout helpers, including the recovered live handle on timeout. |
| `TimeoutError`       | Error type returned by canceling timeout helpers when the deadline expires.                       |
| `Duration`           | Simple duration value object exposed as a convenience type.                                       |

## Module: `std.async.select`

Import with:

```incan
from std.async.select import select_timeout
```

**Functions**:

| Function                                                                       | Returns     | Cancellation contract                                                                                                    |
| ------------------------------------------------------------------------------ | ----------- | ------------------------------------------------------------------------------------------------------------------------ |
| `select_timeout[T, TaskFuture](seconds: float, task: TaskFuture) -> Option[T]` | `Option[T]` | Cancels the supplied future when the deadline wins; cancelling the `select_timeout` wait also drops the supplied future. |

## Module: `std.async.task`

Exported top-level API:

- `spawn[T, TaskFuture](task: TaskFuture) -> JoinHandle[T]`
- `spawn_blocking[T, TaskFn](task: TaskFn) -> JoinHandle[T]`
- `yield_now() -> None`

`spawn()` and `spawn_blocking()` create work that is durable once spawned. Dropping a `JoinHandle[T]` detaches the task and loses the result; it does not cancel the task. Use `JoinHandle.abort()` to request cancellation for async tasks. `spawn_blocking()` work cannot be cancelled after it starts running; `abort()` can only prevent queued blocking work from starting.

Awaiting a `JoinHandle[T]` returns `Result[T, TaskJoinError]`. A task cancelled through `abort()` reports a join error unless it had already completed.

## Module: `std.async.channel`

Top-level API:

- `channel[T](buffer: int) -> Tuple[Sender[T], Receiver[T]]`
- `unbounded_channel[T]() -> Tuple[Sender[T], Receiver[T]]`
- `oneshot[T]() -> Tuple[OneshotSender[T], OneshotReceiver[T]]`
- `SendError[T]`
- `RecvError`
- `Sender[T]`, `SenderPermit[T]`, `Receiver[T]`
- `OneshotSender[T]`, `OneshotReceiver[T]`

Cancellation contracts:

| API                         | Contract                                                                                                                                                         |
| --------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `Sender.send(value)`        | `cancel-safe-but-lossy`: if cancelled while waiting for bounded-channel capacity, the message is not sent and the value is dropped.                              |
| `Sender.reserve()`          | Waits for bounded-channel capacity before a value is committed. Cancelling it does not drop a message value, but it gives up the sender's current wait position. |
| `SenderPermit.send(value)`  | Immediate single-use send through reserved capacity; either delivers the value or returns it in `SendError[T]`.                                                  |
| `Sender.try_send(value)`    | Immediate operation; returns the value in `SendError[T]` on failure.                                                                                             |
| `Receiver.recv()`           | `cancel-safe`: cancelling a pending receive does not remove a message from the channel.                                                                          |
| `Receiver.try_recv()`       | Immediate operation; no pending wait to cancel.                                                                                                                  |
| `Receiver.close()`          | Immediate best-effort close from the receiving side. Returns `false` if another cloned receiver is actively waiting in `recv()` and owns the receiver state.     |
| `OneshotSender.send(value)` | Immediate operation; either delivers the value or returns it.                                                                                                    |
| `OneshotReceiver.recv()`    | `cancel-safe`: cancelling a pending receive does not consume the one-shot value.                                                                                 |

## Module: `std.async.sync`

Top-level API:

- `Mutex[T]`, `MutexGuard[T]`
- `RwLock[T]`, `RwLockReadGuard[T]`, `RwLockWriteGuard[T]`
- `Semaphore`, `SemaphorePermit`
- `Barrier`
- `SemaphoreAcquireError`

Cancellation contracts:

| API                   | Contract                                                                                                                                                                                                                                                                                                                                                             |
| --------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `Mutex.lock()`        | `cancel-safe-but-lossy`: cancellation does not acquire the lock, but the waiter loses its queue position.                                                                                                                                                                                                                                                            |
| `RwLock.read()`       | `cancel-safe-but-lossy`: cancellation does not acquire the read lock, but the waiter loses its queue position.                                                                                                                                                                                                                                                       |
| `RwLock.write()`      | `cancel-safe-but-lossy`: cancellation does not acquire the write lock, but the waiter loses its queue position.                                                                                                                                                                                                                                                      |
| `Semaphore.acquire()` | `cancel-safe-but-lossy`: cancellation does not acquire a permit, but the waiter loses its queue position.                                                                                                                                                                                                                                                            |
| `Barrier.wait()`      | `cancellation-aware before release`: cancellation withdraws the pending participant from the current generation and frees its slot. Remaining participants still need enough active arrivals to complete the generation. The returned slot is unique within a completed generation but is not guaranteed to preserve chronological arrival order after cancellation. |

## Module: `std.async.prelude`

`std.async.prelude` re-exports the following:

- `time`: `sleep`, `sleep_ms`, `timeout`, `timeout_ms`, `timeout_join`, `timeout_join_ms`, `Duration`, `TimeoutError`, `TimeoutJoinOutcome`
- `task`: `spawn`, `spawn_blocking`, `yield_now`, `JoinHandle`, `TaskJoinError`
- `channel`: `channel`, `unbounded_channel`, `oneshot`, `Sender`, `Receiver`, `OneshotSender`, `OneshotReceiver`, `SendError`, `RecvError`
- `sync`: `Mutex`, `MutexGuard`, `RwLock`, `RwLockReadGuard`, `RwLockWriteGuard`, `Semaphore`, `SemaphorePermit`, `SemaphoreAcquireError`, `Barrier`
- `select`: `select_timeout`
