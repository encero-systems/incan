# std.traits.* (reference)

This page documents the standard trait families available under `std.traits.*`. Use these modules when you want trait names explicitly in source, type annotations, or trait adoption.

!!! info "Related pages"
    - If you want the protocol-by-protocol language reference, see: [Stdlib traits overview].

<!-- References -->
[Stdlib traits overview]:../stdlib_traits/index.md

## Importing std.traits

Import from the specific trait submodule:

```incan
from std.traits.convert import From, Into, TryFrom, TryInto
from std.traits.ops import Add, Sub, Mul, Div, FloorDiv, Mod, Pow
from std.traits.ops import Neg, Not
from std.traits.ops import Shr, Shl, BitAnd, BitOr, BitXor, MatMul
from std.traits.ops import PipeForward, PipeBackward
from std.traits.ops import AddAssign, BitAndAssign, MatMulAssign, ShrAssign
from std.traits.error import Error
from std.traits.indexing import Index, IndexMut, Sliceable
from std.traits.callable import Callable0, Callable1, Callable2
from std.traits.prelude import *
```

## Surface model

The `std.traits.*` modules define the standard trait contracts used by Incan.

- Import them directly when you want to write `with TraitName`, annotate against a trait, or refer to the trait family explicitly.
- Some language features map onto these traits at the surface level. For example, operator syntax corresponds to `std.traits.ops`, indexing syntax corresponds to `std.traits.indexing`, and callable-style invocation corresponds to `std.traits.callable`.
- Dunder methods are implementation hooks. Traits are nominal capability vocabulary for explicit adoption, bounds, docs, and diagnostics.
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
- `FloorDiv[Rhs, Output]`
- `Mod[Rhs, Output]`
- `Pow[Rhs, Output]`
- `Neg[Output]`
- `Not[Output]`
- `Shr[Rhs, Output]`
- `Shl[Rhs, Output]`
- `PipeForward[Rhs, Output]`
- `PipeBackward[Rhs, Output]`
- `BitAnd[Rhs, Output]`
- `BitOr[Rhs, Output]`
- `BitXor[Rhs, Output]`
- `MatMul[Rhs, Output]`
- `AddAssign[Rhs, Output]`
- `SubAssign[Rhs, Output]`
- `MulAssign[Rhs, Output]`
- `DivAssign[Rhs, Output]`
- `FloorDivAssign[Rhs, Output]`
- `ModAssign[Rhs, Output]`
- `MatMulAssign[Rhs, Output]`
- `BitAndAssign[Rhs, Output]`
- `BitOrAssign[Rhs, Output]`
- `BitXorAssign[Rhs, Output]`
- `ShlAssign[Rhs, Output]`
- `ShrAssign[Rhs, Output]`

The assignment traits are explicit in-place hooks for the currently supported compound assignment operators. When the explicit hook is absent, compound assignment falls back to ordinary binary operator assignment.

`GetItem[Key, Output]` and `SetItem[Key, Value]` remain compatibility/operator aliases for indexing hooks. Prefer `Index[Key, Output]` and `IndexMut[Key, Value]` from `std.traits.indexing` in new code and docs.

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

These traits can also be used as generic bounds that accept matching named functions and capturing closures. See [Callable objects](../stdlib_traits/callable.md) for the distinction between nominal callable capabilities and ordinary function types.

### `std.traits.prelude`

Re-exports the common `std.traits.*` families so you can import one module instead of each trait family separately.
