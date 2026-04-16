# std.traits.* (reference)

This page documents the curated `std.traits.*` standard-library surface.
The implementation source of truth lives in:

- `crates/incan_stdlib/stdlib/traits/ops.incn`
- `crates/incan_stdlib/stdlib/traits/error.incn`
- `crates/incan_stdlib/stdlib/traits/indexing.incn`
- `crates/incan_stdlib/stdlib/traits/callable.incn`
- `crates/incan_stdlib/stdlib/traits/prelude.incn`

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

Note:

- The `std.traits.convert` imports above document the intended surface, but that submodule remains blocked from compilable-stdlib closeout today. See below.

## Surface model

`std.traits.*` is already source-defined.

- `std.traits.{ops,error,indexing,callable,prelude}` compile directly from `.incn` trait declarations.
- They do not rely on `rust.module()` or `@rust.extern` helper shims.
- Syntax like operators, indexing, slicing, and callable invocation is still compiler-mediated, but the trait contracts themselves live in stdlib source.
- `std.traits.convert` is the current blocked closeout item: its `from` / `try_from` hooks collide with the hard `from` keyword in today's surface, so RFC 023 closeout for that submodule remains documentation-only pending follow-up trait-impl/keyword work.

## Submodules

### `std.traits.convert`

Provides conversion traits:

- `From[T]`
- `Into[T]`
- `TryFrom[T]`
- `TryInto[T]`

Blocked today:

- `std.traits.convert` is documented, but its source file is not yet compilable through the normal stdlib pipeline because `from` is still a hard keyword in declaration position.
- Follow-up is tracked in [RFC 043](../../../RFCs/043_rust_trait_impl_from_incan.md).

### `std.traits.ops`

Provides operator traits:

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

Provides:

- `Index[K, V]`
- `IndexMut[K, V]`
- `Sliceable[T]`

### `std.traits.callable`

Provides:

- `Callable0[R]`
- `Callable1[A, R]`
- `Callable2[A, B, R]`

### `std.traits.prelude`

Re-exports the common `std.traits.*` families for convenience.
