# Numeric semantics (reference)

This is the reference for Incan numeric type spellings, literal checking, assignment compatibility, explicit resizing, Rust interop adaptation, and numeric operator result types.

For task-oriented guidance, see [Choosing numeric types](../how-to/choosing_numeric_types.md). For the design rationale, see [Why numeric types work this way](../explanation/numeric_types.md).

## Numeric type families

| Family                 | Canonical types                                      | Notes                                                              |
| ---------------------- | ---------------------------------------------------- | ------------------------------------------------------------------ |
| Signed integers        | `i8`, `i16`, `i32`, `i64`, `i128`                    | Exact-width signed integers.                                       |
| Unsigned integers      | `u8`, `u16`, `u32`, `u64`, `u128`                    | Exact-width unsigned integers.                                     |
| Pointer-sized integers | `isize`, `usize`                                     | Platform-sized Rust integer types.                                 |
| Binary floats          | `float`, `f32`, `f64`                                | `float` is the broad IEEE type; exact-width `f32` and `f64` are finite-only. |
| Decimal values         | `decimal[p, s]`, `numeric[p, s]`, `decimal128[p, s]` | Fixed-precision decimal types with precision and scale parameters. |

## Canonical type table

| Incan type      | Rust representation             | Value range or shape                                       |
| --------------- | ------------------------------- | ---------------------------------------------------------- |
| `i8`            | `i8`                            | -128 to 127                                                |
| `i16`           | `i16`                           | -32,768 to 32,767                                          |
| `i32`           | `i32`                           | -2,147,483,648 to 2,147,483,647                            |
| `i64`           | `i64`                           | -9,223,372,036,854,775,808 to 9,223,372,036,854,775,807    |
| `i128`          | `i128`                          | -2^127 to 2^127 - 1                                        |
| `u8`            | `u8`                            | 0 to 255                                                   |
| `u16`           | `u16`                           | 0 to 65,535                                                |
| `u32`           | `u32`                           | 0 to 4,294,967,295                                         |
| `u64`           | `u64`                           | 0 to 18,446,744,073,709,551,615                            |
| `u128`          | `u128`                          | 0 to 2^128 - 1                                             |
| `isize`         | `isize`                         | Platform-sized signed integer                              |
| `usize`         | `usize`                         | Platform-sized unsigned integer                            |
| `float`         | `f64`                           | Broad 64-bit IEEE binary float, including NaN and infinity |
| `f32`           | `f32`                           | Finite 32-bit IEEE binary value                            |
| `f64`           | `f64`                           | Finite 64-bit IEEE binary value                            |
| `decimal[p, s]` | `incan_stdlib::num::Decimal128` | Base-10 fixed-scale value with precision `p` and scale `s` |

## Aliases

Aliases resolve to canonical types. They do not introduce distinct nominal types. The broad `float` type and the
exact-width `f64` type share a Rust carrier but have different value contracts, so `float` is not an alias for
`f64`.

| Alias              | Canonical type                                                          |
| ------------------ | ----------------------------------------------------------------------- |
| `byte`             | `u8`                                                                    |
| `short`            | `i16`                                                                   |
| `smallint`         | `i16`                                                                   |
| `integer`          | `i32`                                                                   |
| `int`              | `i64`                                                                   |
| `bigint`           | `i64`                                                                   |
| `long`             | `i64`                                                                   |
| `hugeint`          | `i128`                                                                  |
| `real`             | `f32`                                                                   |
| `fp32`             | `f32`                                                                   |
| `float`            | Broad binary float with an `f64` Rust carrier                           |
| `double`           | `f64`                                                                   |
| `fp64`             | `f64`                                                                   |
| `numeric[p, s]`    | `decimal[p, s]`                                                         |
| `decimal128[p, s]` | `decimal[p, s]` using the current 128-bit scaled runtime representation |

Example:

```incan
x: integer = 10
y: i32 = x
z: int = y
```

## Literal typing

| Literal form | Default type without stronger context | Contextual checks                                                                              |
| ------------ | ------------------------------------- | ---------------------------------------------------------------------------------------------- |
| `42`         | `int`                                 | Checked against the target integer range when assigned to or passed as an exact-width integer. |
| `1_000_000`  | `int`                                 | Separators do not affect the value.                                                            |
| `0.5`        | `float`                               | Checked as `f32` when the expected type is `f32`.                                              |
| `19.99d`     | Requires a decimal expected type      | Checked against the expected decimal precision and scale.                                      |

Integer literals are checked against concrete integer targets:

```incan
ok: u8 = 255
bad: u8 = 256
also_bad: usize = -1
```

Float literals assigned to `f32` must be representable as finite `f32` values.

```incan
ratio: f32 = 0.5
```

## Decimal types

Decimal types require two integer type arguments:

```incan
price: decimal[10, 2] = 19.99d
amount: numeric[12, 4] = 1000.2500d
```

Rules:

- `p` is precision and must be between `1` and `38`.
- `s` is scale and must be between `0` and `p`.
- `decimal`, `numeric`, and `decimal128` require exactly two integer type arguments.
- Bare `decimal`, bare `numeric`, and bare `decimal128` are not value types.
- Decimal literals use a trailing lowercase `d`.
- Decimal literals do not use exponent notation.
- Decimal literals must fit the target precision and scale.

The integer digit count must fit `p - s`, and the fractional digit count must fit `s`.

```incan
ok_money: decimal[10, 2] = 12345678.90d
ok_whole: decimal[5, 0] = 12345d
too_precise: decimal[10, 2] = 1.234d
too_large: decimal[7, 2] = 123456.78d
missing_shape: decimal = 1.00d
```

Decimal arithmetic is not defined by the language yet. The implemented decimal surface covers syntax, type checking, literal validation, formatting, Rust emission, display, and runtime representation.

## Assignment compatibility

Implicit numeric assignment is allowed when the conversion is exact or provably lossless.

| Source         | Target                                                          | Implicit? | Reason                                                   |
| -------------- | --------------------------------------------------------------- | --------- | -------------------------------------------------------- |
| `i8`           | `i16`, `i32`, `i64`, `i128`, `int`                              | Yes       | Every `i8` value fits the target.                        |
| `i32`          | `i64`, `i128`, `int`                                            | Yes       | Every `i32` value fits the target.                       |
| `u8`           | `u16`, `u32`, `u64`, `u128`, `i16`, `i32`, `i64`, `i128`, `int` | Yes       | Every `u8` value fits the target.                        |
| `u16`          | `u32`, `u64`, `u128`, `i32`, `i64`, `i128`, `int`               | Yes       | Every `u16` value fits the target.                       |
| `u32`          | `u64`, `u128`, `i64`, `i128`, `int`                             | Yes       | Every `u32` value fits the target.                       |
| `f32`          | `f64`, `float`                                                  | Yes       | The represented `f32` value can be represented as `f64`. |
| `i64`, `int`   | `i32`                                                           | No        | Values may be outside `i32`.                             |
| `i16`          | `u16`                                                           | No        | Negative values do not fit `u16`.                        |
| `f64`, `float` | `f32`                                                           | No        | Values may not be representable as `f32`.                |

Examples:

```incan
small: i8 = 120
wide: int = small
huge: i128 = wide

bits: u8 = 200
more_bits: u16 = bits

single: f32 = 1.25
double: float = single
```

Integer-to-float assignment is not currently an implicit assignment conversion.

## Resizing methods

Resize methods are contextual: the target type comes from the surrounding expected type.

| Method                | Return type      | Compile-time rule                                        | Runtime behavior                                   |
| --------------------- | ---------------- | -------------------------------------------------------- | -------------------------------------------------- |
| `resize()`            | Target type      | Only accepted for exact or provably lossless conversion. | No data loss.                                      |
| `try_resize()`        | `Option[target]` | Integer targets only.                                    | `Some(value)` if the value fits; `None` otherwise. |
| `wrapping_resize()`   | Target type      | Integer targets only.                                    | Rust-style integer cast wrapping or truncation.    |
| `saturating_resize()` | Target type      | Integer targets only.                                    | Clamps to the target integer minimum or maximum.   |

Examples:

```incan
small: i8 = 120
wide: int = small.resize()

incoming: i16 = 240
maybe: Option[i8] = incoming.try_resize()
wrapped: i8 = incoming.wrapping_resize()
capped: i8 = incoming.saturating_resize()
```

`resize()` is not a forced cast:

```incan
wide: int = 240
small: i8 = wide.resize()
```

The last example is rejected because not every `int` value fits `i8`.

## Rust interop numeric adaptation

Rust interop accepts exact primitive matches and provably lossless primitive widening. Narrowing is rejected.

| Incan source    | Rust target | Accepted? |
| --------------- | ----------- | --------- |
| `i32`           | `i32`       | Yes       |
| `i32`           | `i64`       | Yes       |
| `int` / `i64`   | `i32`       | No        |
| `u8`            | `i16`       | Yes       |
| `i16`           | `u16`       | No        |
| `f32`           | `f64`       | Yes       |
| `float` / `f64` | `f32`       | No        |

If a Rust API requires a narrowing conversion, apply an explicit resize policy before the call.

## Operator result types

| Operator                         | Result type rule                                                                            |
| -------------------------------- | ------------------------------------------------------------------------------------------- |
| `+`, `-`, `*`                    | Matching exact floats retain their width; other floating combinations produce `float`; integer-family operands produce `int`. |
| `/`                              | Matching exact floats retain their width; all other combinations produce `float`.           |
| `//`                             | Matching exact floats retain their width; integer-family operands produce `int`; otherwise `float`. |
| `%`                              | Matching exact floats retain their width; integer-family operands produce `int`; otherwise `float`. |
| `**`                             | Matching exact floats retain their width; `int ** <non-negative int literal>` produces `int`; otherwise `float`. |
| `==`, `!=`, `<`, `<=`, `>`, `>=` | `bool`.                                                                                     |

### Division

`/` is true division. Matching `f32` operands produce `f32`, and matching exact `f64` operands produce exact `f64`.
All other operand combinations produce `float`.

```incan
1 / 2
4 / 2
7.0 / 2
7 / 2.0
```

Division by zero currently panics with a `ZeroDivisionError: float division by zero`-style runtime message.

### Floor division

`//` floors toward negative infinity.

```incan
7 // 3
-7 // 3
7 // -3
-7 // -3
```

Floor division by zero currently panics with a `ZeroDivisionError: float division by zero`-style runtime message.

### Modulo

`%` uses Python-style modulo semantics. The remainder has the sign of the divisor and satisfies `a == (a // b) * b + (a % b)`.

```incan
7 % 3
-7 % 3
7 % -3
-7 % -3
```

Modulo by zero currently panics with a `ZeroDivisionError: float division by zero`-style runtime message.

### Power

`**` returns `int` only when the left operand is `int` and the exponent is a non-negative integer literal.

```incan
2 ** 3
2 ** 0
2 ** -1

exp = 3
2 ** exp
```

## Compound assignment

Compound assignment is typechecked as assignment of the operator result back to the left-hand binding.

```text
x <op>= y
x = x <op> y
```

The two forms are not exactly the same evaluation form, but they have the same assignability requirement.

```incan
mut x: int = 10
x += 2
x *= 3
x /= 2

mut y: float = 10.0
y /= 2
y %= 7
```

`x /= 2` is rejected when `x` is `int`, because `/` returns `float`.

Exact-width integer arithmetic results are ordinary `int` expressions today, so assigning an arithmetic result back to an exact-width binding requires an explicit policy when narrowing would be needed.

```incan
n: i8 = 10
maybe_next: Option[i8] = (n + 1).try_resize()
```

Matching exact-width floating operands retain their width for `+`, `-`, `*`, `/`, `//`, `%`, and `**`. Every exact
result is checked for finiteness when it is produced. Mixed `f32`/`f64`, `f32`/`float`, and `f32`/integer arithmetic
widens the concrete `f32` operand and produces ordinary `float`.

## NaN and infinity

Ordinary `float` follows IEEE behavior and can contain NaN or infinity. Exact-width `f32` and `f64` use Rust's
binary floating-point carriers but accept only finite values.

The compiler rejects non-finite exact literals. Generated code applies the same canonical `ValueError` check when a
direct exact scalar enters through a public function or Rust interop, when an exact scalar is extracted from a field or
collection index, when an exact call or arithmetic operation produces a result, and before an exact value is compared,
formatted, or printed. Crossing from ordinary `float` into an exact carrier performs the same check; ordinary `float`
itself remains unchanged. Public and Rust-facing aggregates are not recursively scanned at ingress; each exact scalar
is checked when Incan extracts or observes it.

Division by zero remains a `ZeroDivisionError` instead of producing NaN or infinity.

## Current limitations

- Decimal arithmetic is not defined yet.
- Integer overflow behavior for general exact-width arithmetic is not yet a separately documented language contract.
- Integer-to-float assignment is not an implicit conversion rule.
- Literal suffixes such as `42i32` or `1.0f32` are not part of the current syntax.
- Parsing decimal values from strings and rich decimal math should remain library-owned until the language specifies those semantics.
