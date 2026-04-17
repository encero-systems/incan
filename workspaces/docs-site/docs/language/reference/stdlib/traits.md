# std.traits.* (reference)

This page documents the standard trait families available under `std.traits.*`.
Use these modules when you want trait names explicitly in source, type annotations, or trait adoption.

!!! info "Related pages"
    - If you want the protocol-by-protocol language reference, see:
      [Stdlib traits overview].

<!-- References -->
[Stdlib traits overview]:../stdlib_traits/index.md

## Importing std.traits

Import from the specific trait submodule:

```incan
from std.traits.convert import From, Into, TryFrom, TryInto
from std.traits.ops import Add, Sub, Mul, Div, Neg, Mod
from std.traits.error import Error
from std.traits.indexing import Index, IndexMut, Sliceable
from std.traits.callable import Callable0, Callable1, Callable2
from std.traits.prelude import *
```

## Surface model

The `std.traits.*` modules define the standard trait contracts used by Incan.

- Import them directly when you want to write `with TraitName`, annotate against a trait, or refer to the trait family explicitly.
- Some language features map onto these traits at the surface level. For example, operator syntax corresponds to `std.traits.ops`, indexing syntax corresponds to `std.traits.indexing`, and callable-style invocation corresponds to `std.traits.callable`.
- `std.traits.prelude` re-exports the most common trait families for convenience.

## Submodules

### `std.traits.convert`

Provides explicit conversion traits:

- `From[T]`
- `Into[T]`
- `TryFrom[T]`
- `TryInto[T]`

`From[T]` and `TryFrom[T]` are constructor-style conversion traits. Their primary hooks are:

- `@classmethod def from(cls, value: T) -> Self`
- `@classmethod def try_from(cls, value: T) -> Result[Self, str]`

Use `From[T]` when conversion should always succeed, and `TryFrom[T]` when conversion can fail.

### `std.traits.ops`

Provides traits behind operator-style behavior:

- `Add[Rhs, Output]`
- `Sub[Rhs, Output]`
- `Mul[Rhs, Output]`
- `Div[Rhs, Output]`
- `Neg[Output]`
- `Mod[Rhs, Output]`

### `std.traits.error`

Provides:

- `Error`

### `std.traits.indexing`

Provides traits for indexed access and slicing:

- `Index[K, V]`
- `IndexMut[K, V]`
- `Sliceable[T]`

### `std.traits.callable`

Provides traits for callable objects with fixed arity:

- `Callable0[R]`
- `Callable1[A, R]`
- `Callable2[A, B, R]`

### `std.traits.prelude`

Re-exports the common `std.traits.*` families so you can import one module instead of each trait family separately.
