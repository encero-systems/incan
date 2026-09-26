# Awaitable values (Reference)

`Awaitable[T]` is the async capability bound for a value whose `await` result is `T`.

## Bound form

```incan
F with Awaitable[T]
```

`F` is the value type. `T` is the type produced by `await value`.

The compiler does not infer `T` from `F`; callers provide both type arguments when they cannot be inferred elsewhere.

```incan
from std.async.task import JoinHandle, TaskJoinError

async def wait_for[T, F with Awaitable[T]](task: F) -> T:
    return await task

# A JoinHandle[int] awaits to Result[int, TaskJoinError].
wait_for[Result[int, TaskJoinError], JoinHandle[int]](handle)
```

## Accepted values

| Value | Await result |
| --- | --- |
| Rust-backed future | Its declared output type |
| `JoinHandle[T]` | `Result[T, TaskJoinError]` |
| Direct async call | The call's declared return type, only at an `await` expression |

An async call result does not satisfy an `Awaitable` bound as a value. Pass a `JoinHandle` from `spawn(...)` when a bounded parameter requires an awaitable value.

## Declaration restrictions

`model`, `class`, `enum`, `newtype`, and `rusttype` declarations cannot adopt `Awaitable[T]`. `Awaitable` is a generic bound, not an adoptable nominal trait.

```incan
model TaskBox[T] with Awaitable[T]:  # refused
    pass
```

Use a generic bound on a function or method instead.

## Diagnostics

The type checker rejects an invalid `Awaitable` adoption at the declaration. It also rejects a bound whose awaited output does not match `T`; for example, `JoinHandle[int]` does not satisfy `Awaitable[int]` because its await result is `Result[int, TaskJoinError]`.

See [async programming](../../how-to/async_programming.md) for task creation and use, and [Rust interop](../../how-to/rust_interop.md) for Rust-backed futures.

--8<-- "_snippets/rfcs_refs.md"
