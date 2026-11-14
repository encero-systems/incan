# Why Call-Site Type Arguments Exist

This page explains the design intent behind explicit call-site type arguments (`f[T](...)`, `obj.m[T](...)`).

For exact syntax and rules, see:

- [Derives and traits (Reference)](../reference/derives_and_traits.md#call-site-type-arguments)

## The problem this solves

Incan normally infers generic type parameters from value arguments.

That works well when value arguments fully constrain type parameters.

It breaks down when one or more type parameters are underconstrained at the call site, or when inference picks a type that is valid but not what you intend.

Common case: APIs where row/model type matters for downstream typing, but value arguments (like file paths) do not uniquely pin that type.

## What explicit call-site arguments add

Call-site type arguments let the caller lock the generic instantiation directly.

*Definition side*:

```incan
class Session:
    def read_csv[T](path: str) -> List[T]:
        ...
```

*Caller side*:

```incan
rows = session.read_csv[Order]("orders.csv")
```

`read_csv[T]` defines a generic contract: “for any `T`, return `List[T]`”. `read_csv[Order](...)` specializes that contract at the call site, so `rows` is typed as `List[Order]`.

This keeps normal signature typing intact while letting callers pin ambiguous generic slots at API boundaries.

## Rust receiver generics use the same call shape

For an imported Rust type whose receiver generics are all type parameters, `Type.method[T](...)` specializes the receiver type. It corresponds to Rust's `Type::<T>::method(...)`, not `Type::method::<T>(...)`.

```incan
from rust::rubato import Fft, FixedSync, ResamplerConstructionError

result: Result[Fft[f32], ResamplerConstructionError] = Fft.new[f32](
    48_000,
    24_000,
    480,
    1,
    FixedSync.Input,
)
```

When an assignment, return annotation, or function parameter already expects `Fft[f32]`, the brackets can be omitted and the same receiver type is inferred from that context. The explicit form remains useful when the result has no constraining context.

Receiver declarations containing Rust const parameters cannot use this syntax in Incan 0.5 because Incan type-argument brackets do not accept const values. The compiler reports that boundary instead of shifting a type argument into the wrong Rust turbofish position. Use a type-only Rust or Incan wrapper for those APIs.

Imported Rust methods may also declare their own type parameters. Their bracket lists are arity-complete just like Incan calls: a three-parameter method accepts `[T, U, V]` or an equally long list containing `_` placeholders, but not a shortened `[T]`. This keeps the checked Incan call and the native Rust method call structurally identical.

## Why `_` exists

Sometimes you only care about one slot and want the rest inferred.

*Definition side*:

```incan
def validate_rows[T, E](rows: List[T]) -> Result[List[T], E]:
    ...
```

*Caller side*:

```incan
a = validate_rows[Order, _](rows)         # pin T, infer E
b = validate_rows(_, RuntimeError)(rows)  # pin E, infer T
```

Here `T` is fixed to `Order`, while `E` is left as `_` and inferred. This avoids forcing callers to spell every type argument when only one slot needs pinning.

`_` keeps partial explicitness concise while preserving arity checks.

## What this feature is not

- It does not replace normal function/method signatures (`def f(x: T) -> U` remains the default form).
- It does not require callers to always be explicit.
- It does not provide overload-based "optional static/dynamic entrypoint" behavior by itself. That route belongs to RFC 028-style overloading design.

## Practical guidance

- Prefer inference by default.
- Use explicit call-site args when API boundaries or readability benefit from locking the type.
- Use `_` when only some type parameters need to be pinned.
