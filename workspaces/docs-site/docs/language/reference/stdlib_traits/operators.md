# Operator traits (Reference)

This page documents stdlib traits that describe operator behavior for custom types.

Operator traits are capability contracts. Dunder methods are the implementation hooks that satisfy those contracts.

The surface follows a split model: dunder methods are the implementation surface, while traits are the nominal capability vocabulary.

A type may also define a dunder hook directly. When a dunder-only implementation matches a standard operator protocol, the compiler may expose the corresponding trait view for generic reasoning.

## Binary arithmetic

| Syntax   | Trait                   | Required hook                              |
| -------- | ----------------------- | ------------------------------------------ |
| `a + b`  | `Add[Rhs, Output]`      | `__add__(self, other: Rhs) -> Output`      |
| `a - b`  | `Sub[Rhs, Output]`      | `__sub__(self, other: Rhs) -> Output`      |
| `a * b`  | `Mul[Rhs, Output]`      | `__mul__(self, other: Rhs) -> Output`      |
| `a / b`  | `Div[Rhs, Output]`      | `__div__(self, other: Rhs) -> Output`      |
| `a // b` | `FloorDiv[Rhs, Output]` | `__floordiv__(self, other: Rhs) -> Output` |
| `a % b`  | `Mod[Rhs, Output]`      | `__mod__(self, other: Rhs) -> Output`      |
| `a ** b` | `Pow[Rhs, Output]`      | `__pow__(self, other: Rhs) -> Output`      |

## Unary operators

| Syntax | Trait         | Required hook                |
| ------ | ------------- | ---------------------------- |
| `-a`   | `Neg[Output]` | `__neg__(self) -> Output`    |
| `~a`   | `Not[Output]` | `__invert__(self) -> Output` |

`Not` names the nominal trait to stay aligned with Rust's `std::ops::Not`; its hook is `__invert__` because the operator surface is bitwise inversion.

## Bitwise, matrix, and pipeline operators

| Syntax    | Trait                       | Required hook                                   |
| --------- | --------------------------- | ----------------------------------------------- |
| `a >> b`  | `Shr[Rhs, Output]`          | `__rshift__(self, other: Rhs) -> Output`        |
| `a << b`  | `Shl[Rhs, Output]`          | `__lshift__(self, other: Rhs) -> Output`        |
| `a & b`   | `BitAnd[Rhs, Output]`       | `__and__(self, other: Rhs) -> Output`           |
| `a \| b`  | `BitOr[Rhs, Output]`        | `__or__(self, other: Rhs) -> Output`            |
| `a ^ b`   | `BitXor[Rhs, Output]`       | `__xor__(self, other: Rhs) -> Output`           |
| `a @ b`   | `MatMul[Rhs, Output]`       | `__matmul__(self, other: Rhs) -> Output`        |
| `a \|> b` | `PipeForward[Rhs, Output]`  | `__pipe_forward__(self, other: Rhs) -> Output`  |
| `a <\| b` | `PipeBackward[Rhs, Output]` | `__pipe_backward__(self, other: Rhs) -> Output` |

## Indexing hooks in the operator vocabulary

Indexing dunders are part of the operator protocol vocabulary:

| Syntax             | Trait                  | Required hook                                       |
| ------------------ | ---------------------- | --------------------------------------------------- |
| `obj[key]`         | `GetItem[Key, Output]` | `__getitem__(self, key: Key) -> Output`             |
| `obj[key] = value` | `SetItem[Key, Value]`  | `__setitem__(self, key: Key, value: Value) -> None` |

The existing `std.traits.indexing` module still exposes `Index[K, V]`, `IndexMut[K, V]`, and `Sliceable[T]` for the older indexing-specific vocabulary. See [Indexing and slicing](indexing_and_slicing.md).

## Compound assignment

Current compound assignment syntax covers `+=`, `-=`, `*=`, `/=`, `//=`, `%=`, `@=`, `&=`, `|=`, `^=`, `<<=`, and `>>=`.

Compound assignment first resolves an explicit in-place operator hook when present, then falls back to ordinary binary operator assignment.

| Syntax    | Explicit trait                | Explicit hook                               | Fallback     |
| --------- | ----------------------------- | ------------------------------------------- | ------------ |
| `a += b`  | `AddAssign[Rhs, Output]`      | `__iadd__(self, other: Rhs) -> Output`      | `a = a + b`  |
| `a -= b`  | `SubAssign[Rhs, Output]`      | `__isub__(self, other: Rhs) -> Output`      | `a = a - b`  |
| `a *= b`  | `MulAssign[Rhs, Output]`      | `__imul__(self, other: Rhs) -> Output`      | `a = a * b`  |
| `a /= b`  | `DivAssign[Rhs, Output]`      | `__idiv__(self, other: Rhs) -> Output`      | `a = a / b`  |
| `a //= b` | `FloorDivAssign[Rhs, Output]` | `__ifloordiv__(self, other: Rhs) -> Output` | `a = a // b` |
| `a %= b`  | `ModAssign[Rhs, Output]`      | `__imod__(self, other: Rhs) -> Output`      | `a = a % b`  |
| `a @= b`  | `MatMulAssign[Rhs, Output]`   | `__imatmul__(self, other: Rhs) -> Output`   | `a = a @ b`  |
| `a &= b`  | `BitAndAssign[Rhs, Output]`   | `__iand__(self, other: Rhs) -> Output`      | `a = a & b`  |
| `a \|= b` | `BitOrAssign[Rhs, Output]`    | `__ior__(self, other: Rhs) -> Output`       | `a = a \| b` |
| `a ^= b`  | `BitXorAssign[Rhs, Output]`   | `__ixor__(self, other: Rhs) -> Output`      | `a = a ^ b`  |
| `a <<= b` | `ShlAssign[Rhs, Output]`      | `__ilshift__(self, other: Rhs) -> Output`   | `a = a << b` |
| `a >>= b` | `ShrAssign[Rhs, Output]`      | `__irshift__(self, other: Rhs) -> Output`   | `a = a >> b` |

## Comparisons

Comparison fallback behavior is deliberately explicit: fallback must be defined by traits or user dunders. The compiler does not synthesize hidden comparison hooks merely because a related hook exists.

The comparison traits are declared with the derive-facing comparison family, not in `std.traits.ops`.
Use `std.derives.comparison.Eq` for `==` / `!=` and `std.derives.comparison.Ord` for `<`, `<=`, `>`, and `>=`.
Their default methods define the fallback behavior directly.
