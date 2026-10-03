# Callable objects (Reference)

`std.traits.callable` declares three traits, for values that are called like functions with zero, one or two arguments. A bound names a trait imported from `std.traits.callable`. Function types, the `Callable[Params, R]` sugar and `mut` parameters are in [Functions and calls](../functions.md#function-types).

| Trait                | Hook                                  | Call form     |
| -------------------- | ------------------------------------- | ------------- |
| `Callable0[R]`       | `def __call__(self) -> R`             | `value()`     |
| `Callable1[A, R]`    | `def __call__(self, arg: A) -> R`     | `value(x)`    |
| `Callable2[A, B, R]` | `def __call__(self, a: A, b: B) -> R` | `value(x, y)` |

## `CallableN` bounds

A type parameter bounded by `CallableN[...]` accepts:

- a named function or a closure with exactly N parameters, whose parameter types match the bound's leading type arguments and whose return type matches its last one; a closure's parameters take their types from the bound;
- a value of a model or class that adopts the same `CallableN[...]` with `with` and defines `__call__`.

Inside the function, the call form calls the value: a function or closure runs, and an adopting type runs its `__call__`. A worked program: [Accept a function, a closure or a callable object](../../how-to/decorators.md#task-accept-a-function-a-closure-or-a-callable-object).

```incan
from std.traits.callable import Callable1

def apply[M with Callable1[int, str]](mapper: M, value: int) -> str:
    return mapper(value)
```

| Call                                                                            | Result                 |
| ------------------------------------------------------------------------------- | ---------------------- |
| `apply((v) => f"item:{v}", 3)`                                                  | `"item:3"`             |
| `apply(Prefixer(prefix="model"), 4)`, `Prefixer` adopting `Callable1[int, str]` | its `__call__(4)`      |
| `apply(shout, 5)`, `shout` of type `(str) -> str`                               | refused: `INCAN-T0001` |

## `Fn`, `FnMut` and `FnOnce` markers

A type parameter of a function or method bounded by `Fn[...]`, `FnMut[...]` or `FnOnce[...]`, imported from `std.rust`, with at most two type arguments, accepts a named function whose parameters match the marker's type arguments in number and type. The marker names no return type: the function's return type is the one it declares.

## Refusals

| Refused                                                                                                                                                                    | Code          |
| -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------- |
| A function or closure whose parameter count, parameter types or return type differ from the bound                                                                         | `INCAN-T0001` |
| A value of a type that does not adopt the bound's `CallableN[...]`                                                                                                         | `INCAN-T0001` |
| A function whose parameter count or parameter types differ from an `Fn`, `FnMut` or `FnOnce` marker                                                                       | `INCAN-T0001` |
| An `Fn`, `FnMut` or `FnOnce` marker from `std.rust` naming more than two parameters, or bounding a type parameter of a type alias, model, class, trait, enum or newtype | `INCAN-T0106` |

## See also

- [Function types and callable traits](../../explanation/functions_and_calls.md#function-types-and-callable-traits)
- [Accept a function, a closure or a callable object](../../how-to/decorators.md#task-accept-a-function-a-closure-or-a-callable-object)
