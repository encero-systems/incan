# Callable objects (Reference)

This page documents callable types in Incan.

## `Callable[Params, R]` type sugar

`Callable[Params, R]` is syntactic sugar for function types. The parser desugars it to the arrow form at parse time:

| Sugar                 | Arrow form    |
| --------------------- | ------------- |
| `Callable[(), R]`     | `() -> R`     |
| `Callable[A, R]`      | `(A) -> R`    |
| `Callable[(A, B), R]` | `(A, B) -> R` |

Both forms are interchangeable in type annotations. Named `def` functions and closures are both accepted wherever a function type is expected. See [RFC 035] for the full specification.

Function values preserve source-declared rest parameters. A function declared with `*args: T` accepts additional
positional arguments and `*list_value` unpacking through the function value; a function declared with `**kwargs: T`
accepts additional keyword arguments and `**dict_value` unpacking through the function value. The rest bindings still
have explicit container types inside the callable: `List[T]` for `*args` and `Dict[str, T]` for `**kwargs`.

## Callable0 / Callable1 / Callable2

These stdlib traits model "objects that can be called" like `obj()`, `obj(x)`, `obj(x, y)`:

- **Callable0[R]**
  - Hook: `__call__(self) -> R`
- **Callable1[A, R]**
  - Hook: `__call__(self, arg: A) -> R`
- **Callable2[A, B, R]**
  - Hook: `__call__(self, a: A, b: B) -> R`

--8<-- "_snippets/rfcs_refs.md"
