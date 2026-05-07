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

`read_csv[T]` defines a generic contract: “for any `T`, return `List[T]`”.
`read_csv[Order](...)` specializes that contract at the call site, so `rows` is typed as `List[Order]`.

This keeps normal signature typing intact while letting callers pin ambiguous generic slots at API boundaries.

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

Here `T` is fixed to `Order`, while `E` is left as `_` and inferred.
This avoids forcing callers to spell every type argument when only one slot needs pinning.

`_` keeps partial explicitness concise while preserving arity checks.

## What this feature is not

- It does not replace normal function/method signatures (`def f(x: T) -> U` remains the default form).
- It does not require callers to always be explicit.
- It does not provide overload-based "optional static/dynamic entrypoint" behavior by itself. That route belongs to RFC 028-style overloading design.

## Practical guidance

- Prefer inference by default.
- Use explicit call-site args when API boundaries or readability benefit from locking the type.
- Use `_` when only some type parameters need to be pinned.
