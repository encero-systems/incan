# Callable objects (Reference)

This page documents callable types in Incan.

## `Callable[Params, R]` type sugar

`Callable[Params, R]` is syntactic sugar for function types. The parser desugars it to the arrow form at parse time:

| Sugar                 | Arrow form    |
| --------------------- | ------------- |
| `Callable[(), R]`     | `() -> R`     |
| `Callable[A, R]`      | `(A) -> R`    |
| `Callable[(A, B), R]` | `(A, B) -> R` |

Both forms are interchangeable in type annotations. Named `def` functions and closures are both accepted wherever a function type is expected.

## `mut` parameters in function types

A parameter of an arrow-form function type can carry the `mut` marker, `(mut T, ...) -> R`. It means what `mut` means on a `def` parameter: the callable's changes to that argument are visible to the caller.

```incan
class Counter:
    pub value: int

def grow(mut counter: Counter, by: int) -> int:
    counter.value += by
    return counter.value

def apply(step: (mut Counter, int) -> int, mut counter: Counter) -> int:
    return step(counter, 2)   # the caller's counter sees the change

def pick() -> (Counter, int) -> int:
    return grow               # refused: INCAN-T0001, found '(mut Counter, int) -> int'
```

| Rule                    | Behavior                                                                                                                                                                                              |
| ----------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Where it is written     | On a parameter of an arrow-form function type. In a tuple type, a parenthesized type, or the parameter list of `Callable[...]` it is a syntax error.                                                  |
| Type identity           | The marker is part of the function type. Two function types match only when they mark the same parameters; a mismatch in either direction is refused with `INCAN-T0001`.                              |
| `def` parameters        | A `def` parameter declared `mut` is marked in the function's type, except a parameter of type `int`, `float` or `bool`, or of a Rust type, and a `*args` or `**kwargs` parameter.                     |
| Closures                | A closure checked against a function type has each parameter that type marks marked in its own type.                                                                                                  |
| Display                 | Diagnostics and hovers spell the marker, as in `(mut Counter, int) -> int`.                                                                                                                           |

Method decorators use the marker for a `mut self` method's receiver; see [Decorators](../language.md#decorators).

## Rest-Aware Function Values

Function values preserve source-declared rest parameters. A function declared with `*args: T` accepts additional positional arguments and `*list_value` unpacking through the function value. A function declared with `**kwargs: T` accepts additional keyword arguments and `**dict_value` unpacking through the function value.

```incan
def collect(prefix: str, *items: int, **labels: str) -> int:
    return len(items) + len(labels)

def main() -> int:
    f = collect
    xs = [1, 2]
    labels = {"kind": "demo"}
    return f("event", 0, *xs, **labels)
```

The rest bindings still have explicit container types inside the callable: `List[T]` for `*args` and `Dict[str, T]` for `**kwargs`. A plain fixed-arity function type with a trailing list or dictionary parameter does not imply rest-call behavior by itself.

See [Functions and calls](../functions.md) for the complete rest parameter and call binding rules.

## Callable0 / Callable1 / Callable2

These stdlib traits model "objects that can be called" like `obj()`, `obj(x)`, `obj(x, y)`:

- **Callable0[R]**
    - Hook: `__call__(self) -> R`
- **Callable1[A, R]**
    - Hook: `__call__(self, arg: A) -> R`
- **Callable2[A, B, R]**
    - Hook: `__call__(self, a: A, b: B) -> R`

The `__call__` method is the implementation hook. A generic `CallableN` bound accepts named functions, capturing closures, and models that explicitly adopt the matching trait, so one API can accept both native function values and domain-owned callable objects:

```incan
from std.traits.callable import Callable1

def apply[Mapper with Callable1[int, str]](mapper: Mapper, value: int) -> str:
    return mapper(value)
```

Use explicit `CallableN` adoption when a model owns callable behavior. Function-typed fields and parameters that do not need a nominal generic capability should continue to use arrow types or the `Callable[Params, R]` sugar above.

--8<-- "_snippets/rfcs_refs.md"
