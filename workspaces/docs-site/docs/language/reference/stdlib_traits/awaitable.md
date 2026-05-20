# Awaitable values (Reference)

`Awaitable[T]` is the async protocol for values that can be used with `await` and produce `T`.

```incan
import std.async

async def wait_for[T, F with Awaitable[T]](task: F) -> T:
    return await task
```

The compiler recognizes these await realization paths:

- direct async function and async method calls
- Rust-backed future values that cross the Rust interop boundary
- `JoinHandle[T]`, which awaits to `Result[T, TaskJoinError]`
- model or class wrappers that explicitly adopt `Awaitable[T]` and contain a compatible awaitable field

Wrapper adoption is checked. A type cannot claim `Awaitable[T]` unless the compiler can lower `await wrapper` to one known awaitable member:

```incan
import std.async
from std.async.task import JoinHandle, TaskJoinError

model TaskBox[T] with Awaitable[Result[T, TaskJoinError]]:
    handle: JoinHandle[T]

async def wait_for(box: TaskBox[int]) -> Result[int, TaskJoinError]:
    return await box
```

This is intentionally not a pure nominal marker. The adoption must preserve actual await behavior so generic bounds and ordinary `await` expressions agree.
