# std.result (reference)

`std.result` exposes Incan-authored helper functions for the value-transforming
`Result[T, E]` combinators.

The method form is the normal way to compose results:

```incan
return read_config(path).and_then(validate_config).map_err(ConfigError.Parse)
```

Direct helper imports are available when a function-shaped API is clearer or when
stdlib code needs the same behavior without relying on method syntax:

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

`inspect` and `inspect_err` are method-only today. They observe a branch through
an implicit borrow and return the original `Result[T, E]` unchanged; that borrowed
observer contract is compiler-provided until borrowed callback types are
user-spellable in Incan source.

## Relationship To Method Syntax

For named function callbacks, the compiler may lower value-transforming method
calls such as `result.map(double)` through these `std.result` helpers. Callable
objects and closure-shaped values remain on the direct method lowering path so
they keep the same callable-object behavior documented for `Callable1`.
