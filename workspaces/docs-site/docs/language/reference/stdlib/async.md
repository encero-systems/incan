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

## Module: `std.async.time`

Import with:

```incan
from std.async.time import sleep, timeout, TimeoutError
```

**Functions**:

| Function                                                                                    | Returns                   |
| ------------------------------------------------------------------------------------------- | ------------------------- |
| `sleep(seconds: float) -> None`                                                             | `None`                    |
| `sleep_ms(milliseconds: int) -> None`                                                       | `None`                    |
| `timeout[T, TaskFuture](seconds: float, task: TaskFuture) -> Result[T, TimeoutError]`       | `Result[T, TimeoutError]` |
| `timeout_ms[T, TaskFuture](milliseconds: int, task: TaskFuture) -> Result[T, TimeoutError]` | `Result[T, TimeoutError]` |

**Models**:

| Name           | Description                                                       |
| -------------- | ----------------------------------------------------------------- |
| `TimeoutError` | Error type returned by timeout helpers when the deadline expires. |
| `Duration`     | Simple duration value object exposed as a convenience type.       |

## Module: `std.async.select`

Import with:

```incan
from std.async.select import select_timeout
```

**Functions**:

| Function                                                                       | Returns     |
| ------------------------------------------------------------------------------ | ----------- |
| `select_timeout[T, TaskFuture](seconds: float, task: TaskFuture) -> Option[T]` | `Option[T]` |

## Module: `std.async.task`

Exported top-level API:

- `spawn[T, TaskFuture](task: TaskFuture) -> JoinHandle[T]`
- `spawn_blocking[T, TaskFn](task: TaskFn) -> JoinHandle[T]`
- `yield_now() -> None`

## Module: `std.async.channel`

Top-level API:

- `channel[T](buffer: int) -> Tuple[Sender[T], Receiver[T]]`
- `unbounded_channel[T]() -> Tuple[Sender[T], Receiver[T]]`
- `oneshot[T]() -> Tuple[OneshotSender[T], OneshotReceiver[T]]`
- `SendError[T]`
- `RecvError`
- `Sender[T]`, `Receiver[T]`
- `OneshotSender[T]`, `OneshotReceiver[T]`

## Module: `std.async.sync`

Top-level API:

- `Mutex[T]`, `MutexGuard[T]`
- `RwLock[T]`, `RwLockReadGuard[T]`, `RwLockWriteGuard[T]`
- `Semaphore`, `SemaphorePermit`
- `Barrier`
- `SemaphoreAcquireError`

## Module: `std.async.prelude`

`std.async.prelude` re-exports the following:

- `time`: `sleep`, `sleep_ms`, `timeout`, `timeout_ms`, `Duration`, `TimeoutError`
- `task`: `spawn`, `spawn_blocking`, `yield_now`, `JoinHandle`, `TaskJoinError`
- `channel`: `channel`, `unbounded_channel`, `oneshot`, `Sender`, `Receiver`, `OneshotSender`, `OneshotReceiver`, `SendError`, `RecvError`
- `sync`: `Mutex`, `MutexGuard`, `RwLock`, `RwLockReadGuard`, `RwLockWriteGuard`, `Semaphore`, `SemaphorePermit`, `SemaphoreAcquireError`, `Barrier`
- `select`: `select_timeout`
