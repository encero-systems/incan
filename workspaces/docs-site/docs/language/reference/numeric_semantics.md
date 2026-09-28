# Numeric semantics (reference)

This page specifies the numeric types and their aliases, numeric literals, decimal types, assignment between numeric types, resizing methods, numeric arguments to Rust functions, numeric operators, and compound assignment.

## Types

| Type | Family | Rust representation | Values |
| --- | --- | --- | --- |
| `i8` | Signed integer | `i8` | -128 to 127 |
| `i16` | Signed integer | `i16` | -32,768 to 32,767 |
| `i32` | Signed integer | `i32` | -2,147,483,648 to 2,147,483,647 |
| `i64` | Signed integer | `i64` | -9,223,372,036,854,775,808 to 9,223,372,036,854,775,807 |
| `i128` | Signed integer | `i128` | -2^127 to 2^127 - 1 |
| `isize` | Signed integer | `isize` | The platform's pointer-sized signed range |
| `u8` | Unsigned integer | `u8` | 0 to 255 |
| `u16` | Unsigned integer | `u16` | 0 to 65,535 |
| `u32` | Unsigned integer | `u32` | 0 to 4,294,967,295 |
| `u64` | Unsigned integer | `u64` | 0 to 18,446,744,073,709,551,615 |
| `u128` | Unsigned integer | `u128` | 0 to 2^128 - 1 |
| `usize` | Unsigned integer | `usize` | The platform's pointer-sized unsigned range |
| `float` | Binary float | `f64` | IEEE 754 binary64 values, NaN and infinity included |
| `f32` | Binary float | `f32` | Finite IEEE 754 binary32 values |
| `f64` | Binary float | `f64` | Finite IEEE 754 binary64 values |
| `decimal[p, s]` | Decimal | `incan_std_core::num::Decimal128` | Base-10 values with precision `p` and scale `s` (see [Decimal types](#decimal-types)) |

## Aliases

An alias is the same type as its canonical type.

| Alias | Canonical type |
| --- | --- |
| `byte` | `u8` |
| `short`, `smallint` | `i16` |
| `integer` | `i32` |
| `int`, `bigint`, `long` | `i64` |
| `hugeint` | `i128` |
| `real`, `fp32` | `f32` |
| `double`, `fp64` | `f64` |
| `numeric[p, s]`, `decimal128[p, s]` | `decimal[p, s]` |

- `float` is not an alias: it is a type of its own, whose values include NaN and infinity. `float` and `f64` are assignable to each other (see [Assignment between numeric types](#assignment-between-numeric-types)).

```incan
def main() -> None:
    x: integer = 10
    y: i32 = x                                   # accepted
    z: i16 = x                                   # refused: i32 is not assignable to i16
```

## Literals

| Literal | Type without a destination | Type at a destination |
| --- | --- | --- |
| `42`, `-42`, `1_000_000` | `int` | The numeric type the destination expects. At a `float`, `f32` or `f64` destination, the float of the same value. |
| `0.5`, `1e3` | `float` | The binary float type the destination expects: `f32` or `f64`. |
| `42u16`, `7i8`, `3.14f32` | The type its suffix names | The type its suffix names. |
| `19.99d`, `12345d` | A decimal literal requires a decimal destination | The destination's `decimal[p, s]` type (see [Decimal types](#decimal-types)). |

- An integer literal outside the range of its integer type is refused (`INCAN-T0001`), as a declared value, an argument and a returned value alike. A suffixed literal outside the range of its suffix's type is refused.
- A literal whose value is not finite in `f32` or `f64` is refused at a destination of that type (`INCAN-T0001`). At a `float` destination, a float literal may be infinite (`1e309`).
- Numeric literals are written in decimal digits. A float literal has a `.` followed by a digit (`0.5`), an exponent (`1e3`), or both.
- `_` separators do not change a literal's value.
- A negated integer literal takes its destination's type as the literal does.
- At an `Option` or union destination that holds one numeric type other than `int`, an integer literal takes that type: `1` is the float `1.0` at an `Option[float]` or `float | str` destination. At a destination that holds `int`, or two other numeric types, it is an `int`.
- An integer literal argument of a generic function takes its parameter's type in the call: with `def pick[T](a: T, b: T) -> T`, `pick(2.5, 1)` and `pick(1, 2.5)` bind `T` to `float`, and `1` is `1.0`.
- An `int` value is not assignable to a float type (see [Assignment between numeric types](#assignment-between-numeric-types)).
- A float literal at an integer destination is refused (`INCAN-T0001`).

```incan
def main() -> None:
    scale: float = 2                             # accepted
    mut total: float = 0.5
    total = -1                                   # accepted: -1.0
    pair: tuple[float, int] = (3, 4)             # accepted: (3.0, 4)
    weights: dict[str, float] = {"a": 1}         # accepted: {"a": 1.0}
    maybe: Option[float] = 1                     # accepted
    ok: u8 = 255                                 # accepted
    bad: u8 = 256                                # refused: 256 is outside u8
    ratio: f32 = 1e39                            # refused: not finite in f32
    count = 5
    total = count                                # refused: count is an int value
```

## Decimal types

- `decimal[p, s]` takes exactly two integer arguments: a precision `p` from 1 to 38 and a scale `s` from 0 to `p`. Any other argument list is refused (`INCAN-T0001`).
- Bare `decimal` and bare `numeric` are not types (`INCAN-T0001`).
- A decimal literal is a plain decimal number followed by a lowercase `d`. A literal in exponent notation (`1e2d`) is refused (`INCAN-T0001`).
- A decimal literal at a `decimal[p, s]` destination has at most `s` fractional digits, and its integer part has at most `p - s` digits (an integer part of `0` has none). A literal outside these limits is refused (`INCAN-T0001`). A literal with fewer than `s` fractional digits is accepted.
- An `int` value is not assignable to a decimal type (`INCAN-T0001`).
- A decimal value is assignable to a decimal type with at least its scale and at least its number of integer digits (`p - s`). A conversion that loses precision or scale is not implicit.
- The language defines no arithmetic on decimal values: an arithmetic operator with a decimal operand, unary `-` included, is refused (`INCAN-T0001`).
- Decimal values compare by value with `==`, `!=`, `<`, `<=`, `>` and `>=` against decimal values of any precision and scale: `1.5d` equals `1.50d`, and `1.49d` is less than `1.5d`. Equal decimal values are the same set element and the same dict key.
- A decimal value displays the fractional digits it was written with, whatever the scale of its type: `1.5d` displays as `1.5` and `1.50d` as `1.50`.

```incan
def main() -> None:
    price: decimal[10, 2] = 19.99d               # accepted
    amount: numeric[12, 4] = 1000.2500d          # accepted
    whole: decimal[5, 0] = 12345d                # accepted
    wider: decimal[12, 4] = price                # accepted
    narrower: decimal[5, 2] = price              # refused: 3 integer digits, not 8
    too_precise: decimal[10, 2] = 1.234d         # refused: 3 fractional digits
    too_large: decimal[7, 2] = 123456.78d        # refused: 6 integer digits
    missing_shape: decimal = 1.00d               # refused: bare decimal
    total = price + price                        # refused: no decimal arithmetic
    cheaper = amount < price                     # accepted
```

## Assignment between numeric types

A value of one numeric type is assignable to another numeric type only as the table lists; every other assignment between different numeric types is refused (`INCAN-T0001`). Arguments and returned values follow the same rule.

| Source | Assignable to |
| --- | --- |
| `i8` | `i16`, `i32`, `i64`, `i128` |
| `i16` | `i32`, `i64`, `i128` |
| `i32` | `i64`, `i128` |
| `i64` | `i128` |
| `u8` | `u16`, `u32`, `u64`, `u128`, `i16`, `i32`, `i64`, `i128` |
| `u16` | `u32`, `u64`, `u128`, `i32`, `i64`, `i128` |
| `u32` | `u64`, `u128`, `i64`, `i128` |
| `u64` | `u128`, `i128` |
| `f32` | `f64`, `float` |
| `f64` | `float` |
| `float` | `f64` |

- `i128`, `u128`, `isize` and `usize` are assignable to no other numeric type.
- No integer type is assignable to a float type, and no float type to an integer type.
- A `float` value that is NaN or infinite raises `ValueError` when it becomes an `f64` value.

```incan
def main() -> None:
    small: i8 = 120
    wide: int = small                            # accepted
    huge: i128 = wide                            # accepted
    bits: u8 = 200
    more_bits: u16 = bits                        # accepted
    single: f32 = 1.25
    double: float = single                       # accepted
    back: i8 = wide                              # refused: int is not assignable to i8
```

## Resizing methods

A resizing method converts a numeric value to its destination type `T`, the expected type of the call, such as a binding's declared type.

| Method | Destination | Receiver and `T` | Result |
| --- | --- | --- | --- |
| `resize()` | `T` | The receiver's type is `T` or assignable to `T` | The same value |
| `try_resize()` | `Option[T]` | Integer types | `Some(value)` when `T` holds the value, otherwise `None` |
| `wrapping_resize()` | `T` | Integer types | The value modulo 2^N, in the range of `T`, where N is the bit width of `T` |
| `saturating_resize()` | `T` | Integer types | The value clamped to the minimum and maximum of `T` |

- A call without a destination type, with arguments or with type arguments, or outside the table's receiver and destination types is refused (`INCAN-T0001`).

```incan
def main() -> None:
    small: i8 = 120
    wide: int = small.resize()                   # accepted
    incoming: i16 = 240
    maybe: Option[i8] = incoming.try_resize()    # accepted
    wrapped: i8 = incoming.wrapping_resize()     # accepted
    capped: i8 = incoming.saturating_resize()    # accepted
    narrow: i8 = wide.resize()                   # refused: int is not assignable to i8
    loose = small.resize()                       # refused: no destination type
```

## Rust interop numeric arguments

A numeric argument to a Rust parameter of a primitive numeric type is accepted when the types match or when the Incan type widens without loss: a signed type to a signed type at least as wide, an unsigned type to an unsigned type at least as wide or to a wider signed type, and `f32` to `f64`. `int` matches `i64` and `float` matches `f64`. `isize` and `usize` match only themselves. Every other pairing is refused.

| Incan argument | Rust parameter | Accepted |
| --- | --- | --- |
| `i32` | `i32` | Yes |
| `i32` | `i64` | Yes |
| `int` | `i32` | No |
| `u8` | `i16` | Yes |
| `i16` | `u16` | No |
| `f32` | `f64` | Yes |
| `float`, `f64` | `f32` | No |

## Operators

| Operator | Operands | Result |
| --- | --- | --- |
| `+`, `-`, `*` | Two `f32` / two `f64` | `f32` / `f64` |
| | Any other pair with a float operand | `float` |
| | Two integer operands | `int` |
| `/` | Two `f32` / two `f64` | `f32` / `f64` |
| | Any other numeric pair | `float` |
| `//`, `%` | Two `f32` / two `f64` | `f32` / `f64` |
| | Any other pair with a float operand | `float` |
| | Two operands of one unsigned type, or an unsigned operand and a non-negative integer literal | That unsigned type |
| | Any other pair of integer operands | `int` |
| `**` | Two `f32` / two `f64` | `f32` / `f64` |
| | An integer left operand and a non-negative integer literal exponent | `int` |
| | Any other numeric pair | `float` |
| `&`, <code>&#124;</code>, `^`, `<<`, `>>` | Two integer operands | `int` |
| `==`, `!=`, `<`, `<=`, `>`, `>=` | Two numeric operands | `bool` |

- `//` or `%` between two different unsigned types, or between an unsigned operand and an `int` value or a negative integer literal, is refused (`INCAN-T0001`).
- `&`, `|`, `^`, `<<` and `>>` with a float operand are refused (`INCAN-T0001`).
- `/` is true division. `//` rounds the quotient toward negative infinity. The result of `%` has the sign of the divisor, and `a == (a // b) * b + (a % b)`.
- `/`, `//` and `%` with a zero divisor raise `ZeroDivisionError`, for signed integer, unsigned integer and float operands alike.
- An `int` `//` whose quotient is outside the `int` range, the minimum `int` divided by `-1`, raises `ValueError`. The minimum `int` `% -1` is `0`.
- `**` binds tighter than a prefix `-` or `~` on its left and looser than one on its right: `-x ** 2` is `-(x ** 2)`, `~x ** 2` is `~(x ** 2)`, and `2 ** -1` is `2 ** (-1)`. The [operator table](language.md#operators) gives the precedence of every operator.
- `+`, `-` and `*` between two values of one type parameter are accepted. `/`, `//`, `%` and `**` between two values of one type parameter are refused (`INCAN-T0109`), unless the parameter is bounded by a trait that defines the operator's method (`__div__`, `__floordiv__`, `__mod__` or `__pow__`); the operator then resolves through that trait.

```incan
trait Remainder:
    def __mod__(self, other: Self) -> Self: ...

def modulo[T with Remainder](a: T, b: T) -> T:
    return a % b                                 # accepted

def divide[T](a: T, b: T) -> T:
    return a / b                                 # refused: INCAN-T0109

def main() -> None:
    half = 1 / 2                                 # accepted
    floor = -7 // 2                              # accepted
    rest = -7 % 3                                # accepted
    cube = 2 ** 3                                # accepted
    inverse = 2 ** -1                            # accepted
    exp = 3
    power: int = 2 ** exp                        # refused: 2 ** exp is a float
    size: u16 = 10
    part: u16 = size % 3                         # accepted
    other: u8 = 3
    mixed = size % other                         # refused: u16 and u8
```

## Compound assignment

- `x op= y`, with `op` one of `+`, `-`, `*`, `/`, `//`, `%`, `&`, `|`, `^`, `<<` and `>>`, reassigns `x`, which is a `mut` binding or a `static` of its module (`INCAN-T0001` otherwise). `**=` is not an operator (`INCAN-P0001`).
- `x op= y` has the assignability requirement of `x = x op y`: the result of `x op y` (see [Operators](#operators)) is assignable to the type of `x`, or the statement is refused (`INCAN-T0001`).

```incan
def main() -> None:
    mut x: int = 10
    x += 2                                       # accepted
    x /= 2                                       # refused: x / 2 is a float
    mut y: float = 10.0
    y /= 2                                       # accepted
    y %= 7                                       # accepted
    mut n: i8 = 10
    n += 1                                       # refused: n + 1 is an int
    following: Option[i8] = (n + 1).try_resize() # accepted
    mut s: f32 = 1.5
    s *= s                                       # accepted
    s *= 2.0                                     # refused: s * 2.0 is a float
    mut b: u8 = 7
    b //= 2                                      # accepted
```

## NaN and infinity

- A `float` value may be NaN or infinite.
- An `f32` or `f64` value is finite. A non-finite value that reaches an `f32` or `f64` at run time, from a `float`, from a Rust value or as the result of an operation, raises `ValueError`.

## See also

- [Choosing numeric types](../how-to/choosing_numeric_types.md)
- [Why numeric types work this way](../explanation/numeric_types.md)
