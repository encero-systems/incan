# Awaitable values (Reference)

`Awaitable[T]` is the async capability bound for a value whose `await` result is `T`. It is a builtin trait name and needs no import.

## Bound form

```incan
F with Awaitable[T]
```

`F` is the value type. `T` is the type produced by `await value`.

The argument bound to `F` does not determine `T`. A call supplies `T` by an explicit type argument or by the call's expected result type.

```incan
import std.async
from std.async.task import JoinHandle, TaskJoinError, spawn

async def wait_for[T, F with Awaitable[T]](task: F) -> T:
    return await task

async def work() -> int:
    return 1

async def main() -> None:
    handle = spawn(work())
    result = await wait_for[Result[int, TaskJoinError], JoinHandle[int]](handle)  # accepted
    match result:
        Ok(value) => println(value)
        Err(_) => println("join failed")
```

## Awaited values

| Value | Await result |
| --- | --- |
| A direct call of an `async def` or async method, written as the `await` operand | The declared return type |
| `JoinHandle[T]` | `Result[T, TaskJoinError]` |
| A Rust-backed future | Its declared output type |
| A value of a type parameter bounded by `Awaitable[T]` | `T` |

A direct call satisfies `await` only as its operand. As a value, including as an argument for a parameter whose type is bounded by `Awaitable`, the call's result has the declared return type and is not awaitable.

## Refusals

`Awaitable[T]` is a bound only.

| Refused | Code |
| --- | --- |
| A `model`, `class`, `enum`, `newtype` or `rusttype` declaration that adopts `Awaitable[T]` | `INCAN-T0001` |
| A type argument for `F` whose await result is not `T`, such as `JoinHandle[int]` for `Awaitable[int]` | `INCAN-T0001` |
| An argument for `F` that is not awaitable, including the result of an `async def` call | `INCAN-T0001` |
| `await` on a value that is not awaitable | `INCAN-T0001` |
| `await` outside an `async def` or async method | `INCAN-T0001` |

```incan
import std.async
from std.async.task import JoinHandle, spawn

async def wait_for[T, F with Awaitable[T]](task: F) -> T:
    return await task

async def work() -> int:
    return 1

model TaskBox[T] with Awaitable[T]:  # refused: a declaration cannot adopt Awaitable (INCAN-T0001)
    value: T

async def main() -> None:
    first = await wait_for[int, JoinHandle[int]](spawn(work()))  # refused: JoinHandle[int] awaits to Result[int, TaskJoinError] (INCAN-T0001)
    second: int = await wait_for(work())                          # refused: an async def call is not an awaitable value (INCAN-T0001)
    third = await 1                                               # refused: int is not awaitable (INCAN-T0001)
```

## See also

- [Async programming](../../how-to/async_programming.md): task creation and use.
- [Rust interop](../../how-to/rust_interop.md): Rust-backed futures.
- [`std.async`](../stdlib/async.md): task, timeout and race signatures.
