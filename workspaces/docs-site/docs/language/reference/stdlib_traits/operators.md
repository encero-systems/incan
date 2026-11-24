# Operator traits (Reference)

Operator syntax dispatches to the corresponding dunder method. A custom type can define that method directly. An explicit trait adoption also makes the type satisfy a bound on that trait.

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

## Indexing hooks

Indexing uses the canonical `std.traits.indexing` names. `GetItem` and `SetItem` remain compatibility/operator aliases for the same hooks; do not use them as a second source of truth in new docs or bounds.

| Syntax             | Trait                  | Required hook                                       |
| ------------------ | ---------------------- | --------------------------------------------------- |
| `obj[key]`         | `Index[Key, Output]`   | `__getitem__(self, key: Key) -> Output`             |
| `obj[key] = value` | `IndexMut[Key, Value]` | `__setitem__(self, key: Key, value: Value) -> None` |

See [Indexing and slicing](indexing_and_slicing.md) for the indexing-specific reference.

## Compound assignment

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

Comparison fallbacks require a default method from an adopted trait or a user-defined dunder. An individual comparison dunder supplies only its corresponding hook.

`std.derives.comparison.Eq` defines the `==` and `!=` hooks. `std.derives.comparison.Ord` defines the `<`, `<=`, `>`, and `>=` hooks. Their default methods define the fallbacks.

## See also

- [Traits as language hooks](../../explanation/traits_as_language_hooks.md)
