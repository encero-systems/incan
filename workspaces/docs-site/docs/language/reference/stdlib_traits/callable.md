# Callable objects (Reference)

This page states the contract of function types, the `Callable[Params, R]` sugar, the `mut` parameter marker, rest-aware function values, and the `Callable0`, `Callable1` and `Callable2` traits.

## Function types

`(A, B) -> R` is the type of a callable that takes an `A` and a `B` and returns an `R`; `() -> R` takes no arguments. A named `def` function and a closure are both values of a function type.

```incan
def double(x: int) -> int:
    return x * 2

def main() -> None:
    f: (int) -> int = double          # accepted
    g: (int) -> int = (x) => x + 1    # accepted
    h: (str) -> int = double          # refused: INCAN-T0001
```

## `Callable[Params, R]` type sugar

`Callable[Params, R]` is another spelling of a function type; the parser rewrites it to the arrow form, and the two spellings are the same type.

| Sugar                 | Arrow form    |
| --------------------- | ------------- |
| `Callable[(), R]`     | `() -> R`     |
| `Callable[A, R]`      | `(A) -> R`    |
| `Callable[(A, B), R]` | `(A, B) -> R` |

`Callable[...]` with other than two type arguments is a syntax error.

## `mut` parameters in function types

`mut` on a parameter lets the function change the value. Changes to a collection, model or class object reach the caller. An `int`, `float` or `bool` parameter is the function's own copy, so its changes stay in the function.

A parameter of an arrow-form function type can carry the `mut` marker, `(mut T, ...) -> R`: the callable's changes to that argument reach the caller.

```incan
class Counter:
    pub value: int

def grow(mut counter: Counter, by: int) -> int:
    counter.value += by
    return counter.value

def apply(step: (mut Counter, int) -> int, mut counter: Counter) -> int:
    return step(counter, 2)

def pick() -> (Counter, int) -> int:
    return grow                           # refused: INCAN-T0001, found '(mut Counter, int) -> int'

def twice(step: (mut int) -> int) -> int:  # refused: INCAN-T0001, `mut` cannot mark the `int` parameter
    return step(step(1))

def main() -> None:
    mut counter = Counter(value=1)
    println(apply(grow, counter))         # 3
    println(counter.value)                # 3
```

| Rule                | Contract                                                                                                                                                                  |
| ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Where it is written | On a parameter of an arrow-form function type. In a tuple type, a parenthesized type, or the parameter list of `Callable[...]` it is a syntax error.                      |
| Copied scalars      | On an `int`, `float` or `bool` parameter, also through a type alias, the marker is refused with `INCAN-T0001`.                                                            |
| Type identity       | The marker is part of the function type. Two function types match only when they mark the same parameters; a mismatch in either direction is refused with `INCAN-T0001`.  |
| `def` parameters    | A `def` parameter declared `mut` is marked in the function's type, except a parameter of type `int`, `float` or `bool`, a parameter of a Rust type, and `*args` or `**kwargs`. |
| Libraries           | A published function keeps its marked parameters: a consumer sees the function type the producer checked.                                                               |
| Closures            | A closure checked against a function type has each parameter that type marks marked in its own type.                                                                      |
| Display             | Diagnostics and hovers spell the marker, as in `(mut Counter, int) -> int`.                                                                                               |

A `mut self` method's decorators spell the receiver with this marker: see [Method decorators](../language.md#method-decorators).

## Rest-aware function values

A function value keeps the rest parameters of the function it names.

| Rule               | Contract                                                                                                                                                  |
| ------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `*args: T`         | Through the value, the call accepts extra positional arguments of type `T` and `*list_value` unpacking; inside the function, `args` is a `List[T]`.        |
| `**kwargs: T`      | Through the value, the call accepts extra keyword arguments of type `T` and `**dict_value` unpacking; inside the function, `kwargs` is a `Dict[str, T]`.   |
| Fixed-arity types  | A function type written with a trailing `List[T]` or `Dict[str, T]` parameter takes that container as one argument; it accepts no extra arguments and no unpacking. |
| Errors             | A call through the value is bound by the rules of a direct call, and refused in the cases [Functions and calls: Type errors](../functions.md#type-errors) lists. |

```incan
def collect(prefix: str, *items: int, **labels: str) -> int:
    return len(items) + len(labels)

def main() -> None:
    f = collect
    xs = [1, 2]
    labels = {"kind": "demo"}
    println(f("event", 0, *xs, **labels))   # 4
```

## `Callable0`, `Callable1`, `Callable2`

`std.traits.callable` declares one trait per arity. Each has a single hook, `__call__`:

| Trait                | Hook                                     | Call form   |
| -------------------- | ---------------------------------------- | ----------- |
| `Callable0[R]`       | `def __call__(self) -> R`                | `value()`     |
| `Callable1[A, R]`    | `def __call__(self, arg: A) -> R`        | `value(x)`    |
| `Callable2[A, B, R]` | `def __call__(self, a: A, b: B) -> R`    | `value(x, y)` |

A type parameter bounded by `CallableN[...]` accepts:

- a named function or a closure with exactly N parameters, whose parameter types match the bound's leading type arguments and whose return type matches its last one; a closure's parameters take their types from the bound;
- a value of a model or class that adopts the same `CallableN[...]` with `with` and defines its `__call__` hook.

Inside the function, the call form calls the value: a function or closure runs, and an adopting type runs its `__call__`.

```incan
from std.traits.callable import Callable1

def apply[Mapper with Callable1[int, str]](mapper: Mapper, value: int) -> str:
    return mapper(value)

@derive(Clone)
model Prefixer with Callable1[int, str]:
    prefix: str

    def __call__(self, value: int) -> str:
        return f"{self.prefix}:{value}"

def shout(text: str) -> str:
    return text.upper()

def main() -> None:
    println(apply((value) => f"item:{value}", 3))       # item:3
    println(apply(Prefixer(prefix="model"), 4))         # model:4
    println(apply(shout, 5))                            # refused: INCAN-T0001
```

| Refused                                                                                                  | Code          |
| -------------------------------------------------------------------------------------------------------- | ------------- |
| A function or closure whose parameter count or types, or return type, differ from the bound               | `INCAN-T0001` |
| A value of a type that does not adopt the bound's `CallableN[...]`                                         | `INCAN-T0001` |
| An `Fn`, `FnMut` or `FnOnce` marker from `std.rust` naming more than two parameters, or bounding a type parameter of a type alias, model, class, trait, enum or newtype | `INCAN-T0106` |

The refusal message names the bound and the type passed, as in `type parameter 'Mapper' requires 'Callable1[int, str]' but got '(str) -> str'`. An `Fn`, `FnMut` or `FnOnce` marker with N parameters is checked as the `CallableN` bound with those parameter types and the return type of the value passed. No `CallableN` trait exists for more than two parameters.

--8<-- "_snippets/rfcs_refs.md"
