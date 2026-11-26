# std.result (reference)

`std.result` exposes Incan-authored helper functions for the `Result[T, E]` combinator surface.

Each helper has the same branch behavior and return type as the corresponding `Result` method.

## Functions

| Function | Signature | Behavior |
| --- | --- | --- |
| `map` | `map[T, E, U](result: Result[T, E], f: Callable[T, U]) -> Result[U, E]` | Transform `Ok(T)` with `f`; preserve `Err(E)`. |
| `map_err` | `map_err[T, E, F](result: Result[T, E], f: Callable[E, F]) -> Result[T, F]` | Transform `Err(E)` with `f`; preserve `Ok(T)`. |
| `and_then` | `and_then[T, E, U](result: Result[T, E], f: Callable[T, Result[U, E]]) -> Result[U, E]` | Chain a `Result`-returning function after `Ok(T)`; preserve `Err(E)`. |
| `or_else` | `or_else[T, E, F](result: Result[T, E], f: Callable[E, Result[T, F]]) -> Result[T, F]` | Recover or remap from `Err(E)` with a `Result`-returning function; preserve `Ok(T)`. |
| `inspect` | `inspect[T, E](result: Result[T, E], f: Callable[T, None]) -> Result[T, E]` | Observe `Ok(T)` with `f`; preserve the original `Result`. |
| `inspect_err` | `inspect_err[T, E](result: Result[T, E], f: Callable[E, None]) -> Result[T, E]` | Observe `Err(E)` with `f`; preserve the original `Result`. |

## Observer Borrowing

`inspect` and `inspect_err` pass the observed payload through an implicit borrow when the original branch value must remain available after the callback. Source code still spells the observer as `Callable[T, None]` or `Callable[E, None]`; there is no separate borrowed callback syntax.

## See also

- [Fallible and infallible paths](../../tutorials/fallible_and_infallible_paths.md)
- [Function types and callable traits](../../explanation/functions_and_calls.md#function-types-and-callable-traits)
