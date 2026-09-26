# Callable objects (Reference)

`std.traits.callable` declares one trait per arity for values that are called like functions. Function types, the `Callable[Params, R]` sugar and `mut` parameters are in [Functions and calls](../functions.md#function-types).

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
def apply[M with Callable1[int, str]](mapper: M, value: int) -> str:
    return mapper(value)
```

| Call                                                                            | Result                 |
| ------------------------------------------------------------------------------- | ---------------------- |
| `apply((v) => f"item:{v}", 3)`                                                  | `"item:3"`             |
| `apply(Prefixer(prefix="model"), 4)`, `Prefixer` adopting `Callable1[int, str]` | its `__call__(4)`      |
| `apply(shout, 5)`, `shout` of type `(str) -> str`                               | refused: `INCAN-T0001` |

## Refusals

| Refused                                                                                                                                                                    | Code          |
| -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------- |
| A function or closure whose parameter count, parameter types or return type differ from the bound                                                                         | `INCAN-T0001` |
| A value of a type that does not adopt the bound's `CallableN[...]`                                                                                                         | `INCAN-T0001` |
| An `Fn`, `FnMut` or `FnOnce` marker from `std.rust` naming more than two parameters, or bounding a type parameter of a type alias, model, class, trait, enum or newtype | `INCAN-T0106` |

The refusal message names the bound and the type passed, as in `type parameter 'M' requires 'Callable1[int, str]' but got '(str) -> str'`. An `Fn`, `FnMut` or `FnOnce` marker with N parameters is checked as the `CallableN` bound with those parameter types and the return type of the value passed. No `CallableN` trait exists for more than two parameters.

--8<-- "_snippets/rfcs_refs.md"
