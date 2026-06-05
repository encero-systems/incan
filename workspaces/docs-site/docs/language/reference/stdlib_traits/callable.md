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

The `__call__` method is the implementation hook. Explicit `CallableN` adoption gives generic bounds and diagnostics a named capability; function type annotations should continue to use arrow types or the `Callable[Params, R]` sugar above.

--8<-- "_snippets/rfcs_refs.md"
