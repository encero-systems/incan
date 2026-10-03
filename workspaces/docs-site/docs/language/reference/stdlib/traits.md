# std.traits.* (reference)

`std.traits` declares the standard trait contracts for conversions, operators, errors, indexing and callable values. Its modules are `std.traits.convert`, `std.traits.ops`, `std.traits.error`, `std.traits.indexing` and `std.traits.callable`; the root module `std.traits` re-exports the common traits, and `std.traits.prelude` names the same root module.

!!! info "Related pages"
    - The protocol-by-protocol reference: [Stdlib traits overview].

<!-- References -->
[Stdlib traits overview]:../stdlib_traits/index.md

## Importing std.traits

A trait is imported by name from the module that declares it, or from `std.traits` (or `std.traits.prelude`, the same module) when `std.traits` re-exports it.

- Refused (`INCAN-P0001`): a wildcard import.
- Refused (`INCAN-I0001`): a name that the named module neither declares nor re-exports.

```incan
from std.traits.convert import From, Into, TryFrom, TryInto   # accepted
from std.traits.ops import Add, AddAssign, Neg, GetItem       # accepted
from std.traits.error import Error                            # accepted
from std.traits.indexing import Index, IndexMut, Sliceable    # accepted
from std.traits.callable import Callable0, Callable1          # accepted
from std.traits import Sum, Callable2                         # accepted
from std.traits import GetItem                                # refused: std.traits does not re-export GetItem (INCAN-I0001)
from std.traits.prelude import Error, Into                    # accepted
from std.traits import *                                      # refused: wildcard import (INCAN-P0001)
```

## Traits and hooks

A type supports an operator, indexing or call syntax by defining the syntax's hook method; adopting the trait is not required for the syntax. A trait names the capability in a `with` adoption and in a generic bound. An adopter that does not define a hook method the trait gives no default body is refused (`INCAN-T0001`).

## Submodules

### `std.traits.convert`

| Trait | Hook |
| --- | --- |
| `From[T]` | `@classmethod def from(cls, value: T) -> Self` |
| `Into[T]` | `def into(self) -> T` |
| `TryFrom[T]` | `@classmethod def try_from(cls, value: T) -> Result[Self, str]` |
| `TryInto[T]` | `def try_into(self) -> Result[T, str]` |

`from` returns a `Self` for every `value`. `try_from` returns `Ok(converted)`, or `Err(message)` for a `value` it does not convert. See [Conversion traits](../stdlib_traits/conversions.md).

### `std.traits.ops`

| Trait | Hook | Syntax |
| --- | --- | --- |
| `Add[Rhs, Output]` | `__add__(self, other: Rhs) -> Output` | `a + b` |
| `Sub[Rhs, Output]` | `__sub__(self, other: Rhs) -> Output` | `a - b` |
| `Mul[Rhs, Output]` | `__mul__(self, other: Rhs) -> Output` | `a * b` |
| `Div[Rhs, Output]` | `__div__(self, other: Rhs) -> Output` | `a / b` |
| `FloorDiv[Rhs, Output]` | `__floordiv__(self, other: Rhs) -> Output` | `a // b` |
| `Mod[Rhs, Output]` | `__mod__(self, other: Rhs) -> Output` | `a % b` |
| `Pow[Rhs, Output]` | `__pow__(self, other: Rhs) -> Output` | `a ** b` |
| `Neg[Output]` | `__neg__(self) -> Output` | `-a` |
| `Not[Output]` | `__invert__(self) -> Output` | `~a` |
| `Shr[Rhs, Output]` | `__rshift__(self, other: Rhs) -> Output` | `a >> b` |
| `Shl[Rhs, Output]` | `__lshift__(self, other: Rhs) -> Output` | `a << b` |
| `BitAnd[Rhs, Output]` | `__and__(self, other: Rhs) -> Output` | `a & b` |
| `BitOr[Rhs, Output]` | `__or__(self, other: Rhs) -> Output` | <code>a &#124; b</code> |
| `BitXor[Rhs, Output]` | `__xor__(self, other: Rhs) -> Output` | `a ^ b` |
| `MatMul[Rhs, Output]` | `__matmul__(self, other: Rhs) -> Output` | `a @ b` |
| `PipeForward[Rhs, Output]` | `__pipe_forward__(self, other: Rhs) -> Output` | <code>a &#124;> b</code> |
| `PipeBackward[Rhs, Output]` | `__pipe_backward__(self, other: Rhs) -> Output` | <code>a <&#124; b</code> |
| `AddAssign[Rhs, Output]` | `__iadd__(self, other: Rhs) -> Output` | `a += b` |
| `SubAssign[Rhs, Output]` | `__isub__(self, other: Rhs) -> Output` | `a -= b` |
| `MulAssign[Rhs, Output]` | `__imul__(self, other: Rhs) -> Output` | `a *= b` |
| `DivAssign[Rhs, Output]` | `__idiv__(self, other: Rhs) -> Output` | `a /= b` |
| `FloorDivAssign[Rhs, Output]` | `__ifloordiv__(self, other: Rhs) -> Output` | `a //= b` |
| `ModAssign[Rhs, Output]` | `__imod__(self, other: Rhs) -> Output` | `a %= b` |
| `MatMulAssign[Rhs, Output]` | `__imatmul__(self, other: Rhs) -> Output` | `a @= b` |
| `BitAndAssign[Rhs, Output]` | `__iand__(self, other: Rhs) -> Output` | `a &= b` |
| `BitOrAssign[Rhs, Output]` | `__ior__(self, other: Rhs) -> Output` | <code>a &#124;= b</code> |
| `BitXorAssign[Rhs, Output]` | `__ixor__(self, other: Rhs) -> Output` | `a ^= b` |
| `ShlAssign[Rhs, Output]` | `__ilshift__(self, other: Rhs) -> Output` | `a <<= b` |
| `ShrAssign[Rhs, Output]` | `__irshift__(self, other: Rhs) -> Output` | `a >>= b` |
| `GetItem[Key, Output]` | `__getitem__(self, key: Key) -> Output` | `a[key]` |
| `SetItem[Key, Value]` | `__setitem__(self, key: Key, value: Value) -> None` | `a[key] = value` |

`a op= b` calls the in-place hook of `a`'s type when the type defines it. When it does not, `a op= b` is `a = a op b`, through the binary hook.

`GetItem` and `SetItem` name the same hooks as `Index` and `IndexMut` of `std.traits.indexing`.

See [Operator traits](../stdlib_traits/operators.md).

### `std.traits.error`

| Trait | Hooks |
| --- | --- |
| `Error` | `message(self) -> str`; `source(self) -> Option[str]`, which returns `None` by default |

See [Error trait](../stdlib_traits/error.md).

### `std.traits.indexing`

| Trait | Hook | Syntax |
| --- | --- | --- |
| `Index[K, V]` | `__getitem__(self, key: K) -> V` | `a[key]` |
| `IndexMut[K, V]` | `__setitem__(self, key: K, value: V) -> None` | `a[key] = value` |

`Sliceable[T]` declares the hook `__getslice__(self, start: Option[int], end: Option[int], step: Option[int]) -> List[T]`.

See [Indexing and slicing](../stdlib_traits/indexing_and_slicing.md).

### `std.traits.callable`

| Trait | Hook | Syntax |
| --- | --- | --- |
| `Callable0[R]` | `__call__(self) -> R` | `a()` |
| `Callable1[A, R]` | `__call__(self, arg: A) -> R` | `a(x)` |
| `Callable2[A, B, R]` | `__call__(self, a: A, b: B) -> R` | `a(x, y)` |

A type parameter bounded by one of these traits also accepts a named function or a capturing closure of the matching shape. See [Callable objects](../stdlib_traits/callable.md) for the bound contract and [Function types](../functions.md#function-types) for ordinary function types.

### `std.traits`

The root module `std.traits` re-exports:

| Module | Names |
| --- | --- |
| `std.traits.convert` | `From`, `Into`, `TryFrom`, `TryInto` |
| `std.traits.ops` | `Add`, `Sub`, `Mul`, `Div`, `Neg`, `Mod` |
| `std.derives.collection` | `Sum` |
| `std.traits.error` | `Error` |
| `std.traits.indexing` | `Index`, `IndexMut`, `Sliceable` |
| `std.traits.callable` | `Callable0`, `Callable1`, `Callable2` |

## See also

- [Traits as language hooks](../../explanation/traits_as_language_hooks.md): why a trait and its hook methods are separate, and how the conversion and indexing trait families relate.
