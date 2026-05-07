# std.result (reference)

`std.result` exposes Incan-authored helper functions for the `Result[T, E]` combinator surface.

Each helper has the same branch behavior as the corresponding `Result` method. The method form is usually the clearest spelling for pipelines:

```incan
return read_config(path).and_then(validate_config).map_err(ConfigError.Parse)
```

Direct helper imports are available when a function-shaped API is clearer:

```incan
from std.result import map as result_map

def double(value: int) -> int:
    return value * 2

def main() -> None:
    value: Result[int, str] = Ok(21)
    doubled = result_map(value, double)
```

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

## Relationship To Method Syntax

For named function callbacks, the compiler may lower method calls such as `result.map(double)` or `result.inspect(log_value)` through these `std.result` helpers. Callable objects and closure-shaped values can remain on the direct method-lowering path so they keep the same callable-object behavior documented for `Callable1`.

That split is an implementation detail, not a semantic distinction users should depend on. The reference contract is that method syntax and the corresponding helper function have the same branch behavior, return type shape, and observer-preservation behavior.

For workflow-oriented examples, see [Fallible and infallible paths](../../tutorials/fallible_and_infallible_paths.md). For the compiler-side ownership policy behind observer borrowing, see [Duckborrowing](../../../contributing/explanation/duckborrowing.md).
