# Numeric semantics (reference)

This is the reference for Incan numeric types, numeric literals, decimal types, assignment compatibility, resize methods, numeric arguments to Rust functions, arithmetic operator results, compound assignment, and non-finite values.

For task-oriented guidance, see [Choosing numeric types](../how-to/choosing_numeric_types.md). For the design rationale, see [Why numeric types work this way](../explanation/numeric_types.md).

In the examples on this page, a trailing comment gives the outcome of its line:

- `# 0.5: float`: the line is accepted; the expression or binding has the value `0.5` and the type `float`.
- `# refused, INCAN-T0001: …`: `incan check` refuses the line with that diagnostic code. `INCAN-T0001` is the general type-checking code; `INCAN-T0109` is the code for arithmetic on a type parameter.
- `# fails at runtime: …`: the line checks and builds, and the program stops with that message when the line runs.

## Numeric types

### Type families

| Family                 | Types                                                | Notes                                                                              |
| ---------------------- | ---------------------------------------------------- | ---------------------------------------------------------------------------------- |
| Signed integers        | `i8`, `i16`, `i32`, `i64`, `i128`                    | Exact-width signed integers.                                                       |
| Unsigned integers      | `u8`, `u16`, `u32`, `u64`, `u128`                    | Exact-width unsigned integers.                                                     |
| Pointer-sized integers | `isize`, `usize`                                     | Integers as wide as a pointer on the target platform.                              |
| Binary floats          | `float`, `f32`, `f64`                                | `float` is the broad IEEE 754 type; `f32` and `f64` hold finite values only.        |
| Decimal values         | `decimal[p, s]`, `numeric[p, s]`, `decimal128[p, s]` | Base-10 fixed-point types with precision `p` and scale `s`.                        |

On this page, *integer-family* means `int` and every signed, unsigned and pointer-sized integer type; *float-family* means `float`, `f32` and `f64`; *exact float* means `f32` or `f64`.

### Canonical types

| Incan type         | Rust interop type | Values                                                               |
| ------------------ | ----------------- | -------------------------------------------------------------------- |
| `i8`               | `i8`              | -128 to 127                                                          |
| `i16`              | `i16`             | -32,768 to 32,767                                                    |
| `i32`              | `i32`             | -2,147,483,648 to 2,147,483,647                                      |
| `i64`              | `i64`             | -9,223,372,036,854,775,808 to 9,223,372,036,854,775,807              |
| `i128`             | `i128`            | -2^127 to 2^127 - 1                                                  |
| `u8`               | `u8`              | 0 to 255                                                             |
| `u16`              | `u16`             | 0 to 65,535                                                          |
| `u32`              | `u32`             | 0 to 4,294,967,295                                                   |
| `u64`              | `u64`             | 0 to 18,446,744,073,709,551,615                                      |
| `u128`             | `u128`            | 0 to 2^128 - 1                                                       |
| `isize`            | `isize`           | The target platform's pointer-sized signed range                     |
| `usize`            | `usize`           | The target platform's pointer-sized unsigned range                   |
| `float`            | `f64`             | 64-bit IEEE 754 binary values, including NaN and positive and negative infinity |
| `f32`              | `f32`             | Finite 32-bit IEEE 754 binary values                                 |
| `f64`              | `f64`             | Finite 64-bit IEEE 754 binary values                                 |
| `decimal[p, s]`    | none              | Base-10 values with at most `p` digits, `s` of them after the point  |
| `decimal128[p, s]` | none              | Same value shape as `decimal[p, s]`                                  |

The Rust interop type is the Rust parameter type that accepts a value of the Incan type as an exact match (see [Rust interop arguments](#rust-interop-arguments)). Decimal types have no Rust interop type.

### Aliases

An alias resolves to its canonical type. It does not introduce a distinct type: a value declared with an alias is a value of the canonical type.

| Alias           | Canonical type  |
| --------------- | --------------- |
| `byte`          | `u8`            |
| `short`         | `i16`           |
| `smallint`      | `i16`           |
| `integer`       | `i32`           |
| `int`           | `i64`           |
| `bigint`        | `i64`           |
| `long`          | `i64`           |
| `hugeint`       | `i128`          |
| `real`          | `f32`           |
| `fp32`          | `f32`           |
| `double`        | `f64`           |
| `fp64`          | `f64`           |
| `numeric[p, s]` | `decimal[p, s]` |

`float` is not an alias of `f64`. Both have a 64-bit binary representation, but `float` admits NaN and infinity and `f64` does not; the two are assignable to each other under the rules in [Assignment compatibility](#assignment-compatibility).

`decimal128[p, s]` is not an alias of `decimal[p, s]`. It is a separate constructor with the same precision and scale rules, and the type checker keeps it as its own type, so a `decimal128[p, s]` value is not assignable to `decimal[p, s]`, nor the reverse.

```incan
x: integer = 10   # 10: i32
y: i32 = x        # 10: i32
z: int = y        # 10: int
```

## Numeric literals

| Literal form | Examples               | Type when there is no expected type | Check when the expected type is numeric                                                                  |
| ------------ | ---------------------- | ----------------------------------- | -------------------------------------------------------------------------------------------------------- |
| Integer      | `42`, `1_000_000`      | `int`                               | For an integer type, the value must lie in that type's range.                                            |
| Float        | `0.5`, `1e5`, `2.5e-3` | `float`                             | For `f32`, the value must be finite with magnitude at most 3.4028235e38 (the largest finite `f32`). For `f64`, it must be finite. For `float`, there is no check. |
| Decimal      | `19.99d`, `12345d`     | Not specified                       | For a decimal type, the value must fit its precision and scale (see [Decimal types](#decimal-types)).    |

Rules:

- Numeric literals are written in decimal digits. There are no hexadecimal, octal, or binary forms.
- `_` may separate digits. Each `_` must have a digit on both sides, and it does not change the value.
- A float literal has a point followed by at least one digit (`0.5`), an exponent (`1e5`), or both.
- A decimal literal is digits with an optional fractional part, followed by a lowercase `d`.
- The decimal `d` is the only literal suffix. `42i32` and `1.0f32` are not literals.
- A float literal is not accepted where an integer-family type is expected.
- A leading `-` is the prefix minus operator. The range check applies to the negated value, so `-128` fits `i8`.
- The range check for an `isize` or `usize` literal uses the pointer width of the machine that runs the compiler.

```incan
ok: u8 = 255              # 255: u8
low: i8 = -128            # -128: i8
bad: u8 = 256             # refused, INCAN-T0001: 256 is outside 0..=255
also_bad: usize = -1      # refused, INCAN-T0001: -1 is outside the usize range
ratio: f32 = 0.5          # 0.5: f32
too_big: f32 = 1e39       # refused, INCAN-T0001: not a finite f32 value
```

## Decimal types

A decimal type takes two type arguments, precision `p` and scale `s`: `decimal[p, s]`, `numeric[p, s]` (an alias of `decimal[p, s]`), or `decimal128[p, s]`.

Rules:

- `p` and `s` are integer literals.
- `p` is between `1` and `38`, inclusive.
- `s` is between `0` and `p`, inclusive.
- Bare `decimal`, `numeric`, and `decimal128` are not types, and an annotation that uses one is refused.
- A decimal literal whose expected type is a decimal type has at most `p - s` digits before the point, at most `s` digits after it, and at most `p` digits in total. Every written digit counts, including a leading `0` before the point and trailing zeros after it.
- A decimal literal in exponent form (`1e3d`) is refused at a decimal type.

Each rule violation is refused with `INCAN-T0001`.

```incan
price: decimal[10, 2] = 19.99d             # 19.99: decimal[10, 2]
amount: numeric[12, 4] = 1000.2500d        # 1000.2500: decimal[12, 4]
ok_money: decimal[10, 2] = 12345678.90d    # 12345678.90: decimal[10, 2]
ok_whole: decimal[5, 0] = 12345d           # 12345: decimal[5, 0]
too_precise: decimal[10, 2] = 1.234d       # refused, INCAN-T0001: 3 digits after the point, scale is 2
too_large: decimal[7, 2] = 123456.78d      # refused, INCAN-T0001: 6 digits before the point, at most 5
exponent_form: decimal[10, 2] = 1e3d       # refused, INCAN-T0001: not a plain decimal literal
missing_shape: decimal = 1.00d             # refused, INCAN-T0001: bare decimal is not a type
```

Operations on decimal values are listed under [Not defined or refused](#not-defined-or-refused).

## Assignment compatibility

A value of numeric type `S` is assignable to numeric type `T`, with no conversion written in source, when one of these holds:

- `S` and `T` are the same type.
- `S` and `T` are both signed integer types, or both unsigned integer types, and `T` is at least as wide as `S`.
- `S` is an unsigned integer type, `T` is a signed integer type, and `T` is strictly wider than `S`.
- `S` and `T` are both float-family types and `T` is at least as wide as `S`. `float` and `f64` both count as 64 bits wide.

Pointer-sized types are assignable only to themselves. No integer-family type is assignable to a float-family type, and no float-family type is assignable to an integer-family type. These rules apply to values; a literal is checked against its expected type as described in [Numeric literals](#numeric-literals).

| Source           | Target                                                              | Assignable? | Reason                                                                          |
| ---------------- | ------------------------------------------------------------------- | ----------- | ------------------------------------------------------------------------------- |
| `i8`             | `i16`, `i32`, `i64`/`int`, `i128`                                   | Yes         | Every `i8` value fits the target.                                               |
| `i16`            | `i32`, `i64`/`int`, `i128`                                          | Yes         | Every `i16` value fits the target.                                              |
| `i32`            | `i64`/`int`, `i128`                                                 | Yes         | Every `i32` value fits the target.                                              |
| `i64`/`int`      | `i128`                                                              | Yes         | Every `i64` value fits the target.                                              |
| `u8`             | `u16`, `u32`, `u64`, `u128`, `i16`, `i32`, `i64`/`int`, `i128`      | Yes         | Every `u8` value fits the target.                                               |
| `u16`            | `u32`, `u64`, `u128`, `i32`, `i64`/`int`, `i128`                    | Yes         | Every `u16` value fits the target.                                              |
| `u32`            | `u64`, `u128`, `i64`/`int`, `i128`                                  | Yes         | Every `u32` value fits the target.                                              |
| `u64`            | `u128`, `i128`                                                      | Yes         | Every `u64` value fits the target.                                              |
| `f32`            | `f64`, `float`                                                      | Yes         | Every finite `f32` value is a finite 64-bit value.                              |
| `f64`            | `float`                                                             | Yes         | Every `f64` value is a `float` value.                                           |
| `float`          | `f64`                                                               | Yes         | Checked at runtime: a NaN or infinite value fails with `ValueError: non-finite float cannot initialize exact f64`. |
| `i64`/`int`      | `i8`, `i16`, `i32`                                                  | No          | Values may be outside the target range.                                         |
| `i16`            | `u16`                                                               | No          | A signed type is never assignable to an unsigned type.                          |
| `u64`            | `i64`/`int`                                                         | No          | An unsigned type needs a strictly wider signed target.                          |
| `isize`, `usize` | Any other numeric type                                              | No          | Pointer-sized types are assignable only to themselves.                          |
| Any other integer type | `isize`, `usize`                                              | No          | Pointer-sized types are assignable only to themselves.                          |
| `f64`, `float`   | `f32`                                                               | No          | Values may not be representable as `f32`.                                       |
| Any integer-family type | `float`, `f32`, `f64`                                        | No          | Integer and float families do not mix.                                          |

```incan
small: i8 = 120
wide: int = small          # 120: int
huge: i128 = wide          # 120: i128
bits: u8 = 200
more_bits: u16 = bits      # 200: u16
single: f32 = 1.25
widened: float = single    # 1.25: float
```

```incan
wide: int = 240
narrow: i32 = wide         # refused, INCAN-T0001: int is not assignable to i32
as_float: float = wide     # refused, INCAN-T0001: int is not assignable to float
```

## Resize methods

A resize method converts a numeric value to the type the call site expects. The method takes no arguments and no type arguments. The target is the expected type at the call site, such as the annotated type of the binding that receives the result. A resize call with no expected type is refused.

| Method                | Expected type    | Accepted source and target                                          | Result                                                                                                     |
| --------------------- | ---------------- | ------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- |
| `resize()`            | `T`              | Any numeric source whose type is assignable to `T` (see [Assignment compatibility](#assignment-compatibility)). | The same value as a `T`.                                                  |
| `try_resize()`        | `Option[T]`      | Integer-family source, integer-family `T`.                          | `Some(value)` when the value lies in `T`'s range; `None` otherwise.                                         |
| `wrapping_resize()`   | `T`              | Integer-family source, integer-family `T`.                          | The value reduced modulo 2^N, where N is `T`'s bit width, read in `T`'s signed or unsigned range.           |
| `saturating_resize()` | `T`              | Integer-family source, integer-family `T`.                          | The value clamped to `T`'s minimum or maximum.                                                             |

A call outside the accepted source and target is refused with `INCAN-T0001`. `resize()` is not a cast: it refuses every target a plain assignment would refuse.

```incan
small: i8 = 120
wide: int = small.resize()                     # 120: int
incoming: i16 = 240
maybe: Option[i8] = incoming.try_resize()      # None: Option[i8]
wrapped: i8 = incoming.wrapping_resize()       # -16: i8
capped: i8 = incoming.saturating_resize()      # 127: i8
```

```incan
wide: int = 240
small: i8 = wide.resize()                      # refused, INCAN-T0001: int is not assignable to i8
approx: float = wide.resize()                  # refused, INCAN-T0001: int is not assignable to float
```

## Rust interop arguments

A numeric argument to a Rust function is accepted when its Incan type is the parameter's Rust interop type (see [Canonical types](#canonical-types)) or is assignable to it under [Assignment compatibility](#assignment-compatibility). Every other numeric argument is refused, including every narrowing and every conversion between integer and float families.

| Incan argument  | Rust parameter | Accepted? |
| --------------- | -------------- | --------- |
| `i32`           | `i32`          | Yes       |
| `i32`           | `i64`          | Yes       |
| `int` / `i64`   | `i32`          | No        |
| `u8`            | `i16`          | Yes       |
| `i16`           | `u16`          | No        |
| `isize`         | `isize`        | Yes       |
| `i64`           | `isize`        | No        |
| `f32`           | `f64`          | Yes       |
| `float`         | `f64`          | Yes       |
| `float` / `f64` | `f32`          | No        |
| `int`           | `f64`          | No        |

To pass a value that needs narrowing, see [Match Rust numeric parameters instead of relying on casts](../how-to/choosing_numeric_types.md#match-rust-numeric-parameters-instead-of-relying-on-casts).

## Arithmetic operators

### Result types

The result type of `a op b` is the first row of its operator that matches the operand types.

| Operator                         | Operands                                                                                     | Result type                 |
| -------------------------------- | -------------------------------------------------------------------------------------------- | --------------------------- |
| `+`, `-`, `*`                    | Both the same exact float (`f32` and `f32`, or `f64` and `f64`)                              | That type                   |
|                                  | Both integer-family                                                                          | `int`                       |
|                                  | Otherwise (at least one float-family operand)                                                | `float`                     |
| `/`                              | Both the same exact float                                                                    | That type                   |
|                                  | Otherwise                                                                                    | `float`                     |
| `//`, `%`                        | Both the same unsigned type, or one unsigned operand and one non-negative integer literal    | That unsigned type          |
|                                  | Two different unsigned types                                                                 | Refused, `INCAN-T0001`      |
|                                  | One unsigned operand and one `int` operand that is not a non-negative integer literal        | Refused, `INCAN-T0001`      |
|                                  | Both the same exact float                                                                    | That type                   |
|                                  | Both integer-family                                                                          | `int`                       |
|                                  | Otherwise                                                                                    | `float`                     |
| `**`                             | Both the same exact float                                                                    | That type                   |
|                                  | Integer-family base and a non-negative integer literal exponent (optionally parenthesized)   | `int`                       |
|                                  | Otherwise                                                                                    | `float`                     |
| `&`, `|`, `^`, `<<`, `>>`        | Both integer-family                                                                          | `int`                       |
|                                  | A float-family operand                                                                       | Refused, `INCAN-T0001`      |
| `==`, `!=`, `<`, `<=`, `>`, `>=` | Any two numeric operands                                                                     | `bool`                      |

Consequences of the table:

- An integer result is `int` whatever the operand widths: `i8 + i8` is `int`, and so is `u8 + u8`.
- A mixed exact-float operation (`f32` with `f64`, `f32` with `float`, `f32` with an integer, `f64` with `float`) produces `float`.
- An `int` result assigned to a narrower binding follows [Assignment compatibility](#assignment-compatibility): `next: i8 = n + 1` is refused with `INCAN-T0001` for `n: i8`.

```incan
x: i8 = 10
x + x         # 20: int
b: u8 = 7
b // 2        # 3: u8
b % 2         # 1: u8
b + 1         # 8: int
a: f32 = 1.5
a + a         # 3.0: f32
a + 1.0       # 2.5: float
a * 2         # 3.0: float
```

```incan
b: u8 = 7
c: u16 = 2
b // c        # refused, INCAN-T0001: two different unsigned types
n: int = 2
b % n         # refused, INCAN-T0001: an int operand must be a non-negative integer literal
```

### Operators on type parameters

For two values of the same type parameter `T`:

- When `T`'s bound trait defines the operator's hook, the operator resolves through that hook and has the hook's return type.
- Otherwise, `+`, `-`, and `*` are accepted with result type `T`, and the operator becomes a bound on `T`.
- Otherwise, `/`, `//`, `%`, and `**` are refused with `INCAN-T0109`. Their hooks are `__div__`, `__floordiv__`, `__mod__`, and `__pow__`.

```incan
def add[T](a: T, b: T) -> T:
    return a + b      # accepted: T

def modulo[T](a: T, b: T) -> T:
    return a % b      # refused, INCAN-T0109
```

For the ways to write such a function, see [Divide, take a remainder, or raise a power in generic code](../how-to/choosing_numeric_types.md#divide-take-a-remainder-or-raise-a-power-in-generic-code).

### Division

`/` is true division. It produces `float`, except that two operands of the same exact float produce that type.

A zero divisor, integer or float (including `0.0` and `-0.0`), fails at runtime with `ZeroDivisionError: float division by zero`. `/` never produces NaN or infinity from a zero divisor.

```incan
1 / 2         # 0.5: float
4 / 2         # 2.0: float
7.0 / 2       # 3.5: float
7 / 2.0       # 3.5: float
1 / 0         # fails at runtime: ZeroDivisionError: float division by zero
0.0 / 0.0     # fails at runtime: ZeroDivisionError: float division by zero
```

### Floor division

`//` rounds the quotient toward negative infinity.

- Integer-family operands (other than the unsigned cases) produce `int`. A zero divisor fails at runtime with `ZeroDivisionError: float division by zero`. A quotient outside the `int` range (the minimum `int` value divided by `-1`) fails at runtime with `ValueError: integer floor division result overflows Incan int`.
- When either operand is float-family, the result is the floor of the float quotient. A zero divisor fails at runtime with `ZeroDivisionError: float division by zero`.
- Unsigned operands that keep their unsigned type (see [Result types](#result-types)) use unsigned integer division. A zero divisor fails at runtime with the Rust panic message `attempt to divide by zero`, not with `ZeroDivisionError`.

```incan
7 // 3        # 2: int
-7 // 3       # -3: int
7 // -3       # -3: int
-7 // -3      # 2: int
7.5 // 2      # 3.0: float
7 // 0        # fails at runtime: ZeroDivisionError: float division by zero
```

### Modulo

`%` produces a remainder with the sign of the divisor. For integer-family operands, `a == (a // b) * b + (a % b)`.

- Integer-family operands (other than the unsigned cases) produce `int`. A zero divisor fails at runtime with `ZeroDivisionError: float division by zero`. The minimum `int` value `% -1` is `0`.
- When either operand is float-family, the result is a float remainder with the sign of the divisor. A zero divisor fails at runtime with `ZeroDivisionError: float division by zero`.
- Unsigned operands that keep their unsigned type use unsigned remainder. A zero divisor fails at runtime with the Rust panic message `attempt to calculate the remainder with a divisor of zero`, not with `ZeroDivisionError`.

```incan
7 % 3         # 1: int
-7 % 3        # 2: int
7 % -3        # -2: int
-7 % -3       # -1: int
-7.5 % 2      # 0.5: float
7 % 0         # fails at runtime: ZeroDivisionError: float division by zero
```

### Power

`**` produces `int` only when the base is integer-family and the exponent is a non-negative integer literal. Two operands of the same exact float produce that type. Every other combination, including a negative literal exponent and any exponent held in a variable, produces `float`. `**` performs no zero check: with a `float` result, a zero base and a negative exponent produce infinity.

`**` binds tighter than a prefix `-` or `~` on its left and looser than one on its right: `-x ** 2` is `-(x ** 2)`, `~x ** 2` is `~(x ** 2)`, and `2 ** -1` is `2 ** (-1)`. The [operator table](language.md#operators) lists the full precedence order.

```incan
base = 2
base ** 3       # 8: int
base ** 0       # 1: int
base ** -1      # 0.5: float, grouped as base ** (-1)
exp = 3
base ** exp     # 8.0: float
-base ** 2      # -4: int, grouped as -(base ** 2)
(-base) ** 2    # 4: int
zero = 0
zero ** -1      # inf: float
```

## Compound assignment

The numeric compound operators are `+=`, `-=`, `*=`, `/=`, `//=`, `%=`, `&=`, `|=`, `^=`, `<<=`, and `>>=`. There is no `**=`.

A field or index target (`obj.field op= y`, `items[i] op= y`) is checked as `target = target op y`, with the result type from [Result types](#result-types).

A name target is a local binding declared with `mut`, or a module `static` of the same module. For numeric operands, `x op= y` takes its result type from the operand families alone: `float` when `op` is `/=` or either operand is float-family, and `int` otherwise; a float-family operand with `&=`, `|=`, `^=`, `<<=`, or `>>=` is refused. The exact-float and unsigned rows of [Result types](#result-types) do not apply. The statement is accepted when that result type is assignable to the type of `x` under [Assignment compatibility](#assignment-compatibility), and refused with `INCAN-T0001` otherwise. For a name target, as a result:

- `/=` is refused on every integer-family binding.
- Every numeric compound operator is refused on a binding of an integer type other than `i64`/`int` and `i128`, because `int` is not assignable to it.
- Every numeric compound operator is refused on an `f32` binding, because `float` is not assignable to `f32`.
- A float-family operand is refused on an integer-family binding.

```incan
mut x: int = 10
x += 2        # 12: int
x *= 3        # 36: int
x //= 5       # 7: int
mut y: float = 10.0
y /= 4        # 2.5: float
y %= 2        # 0.5: float
```

```incan
mut x: int = 10
x /= 2        # refused, INCAN-T0001: the float result is not assignable to int
mut n: i32 = 1
n += 1        # refused, INCAN-T0001: the int result is not assignable to i32
mut s: f32 = 1.0
s *= 2.0      # refused, INCAN-T0001: the float result is not assignable to f32
```

## NaN and infinity

`float` follows IEEE 754: its arithmetic can produce NaN and infinity, except that a zero divisor in `/`, `//`, or `%` fails as described in [Division](#division). `f32` and `f64` hold finite values only:

- A float literal whose expected type is `f32` or `f64` must be finite (see [Numeric literals](#numeric-literals)).
- At runtime, a NaN or infinite value that would become an `f32` or `f64` value fails with `ValueError: non-finite float cannot initialize exact f32` (or `exact f64`). The check applies when an exact float enters through a public function or a Rust call, when one is read from a field or a collection element, when a call or arithmetic operation produces one, when a `float` value is assigned to an exact float, and before an exact float is compared, formatted, or printed. A value of ordinary `float` is never checked.
- Values inside aggregates that arrive from public or Rust-facing code are not scanned on arrival; each exact float is checked when it is read from the aggregate.

```incan
big: f64 = 1e308
wide = big * 10.0         # inf: float
exact = big * big         # fails at runtime: ValueError: non-finite float cannot initialize exact f64
too_big: f64 = 1e999      # refused, INCAN-T0001: not a finite f64 value
```

## Not defined or refused

- Arithmetic operators (`+`, `-`, `*`, `/`, `//`, `%`, `**`) on decimal values are refused with `INCAN-T0001`.
- Comparison of decimal values is not specified.
- Conversion between a decimal type and any other numeric type is not defined.
- Assigning a decimal value that is not a literal to a decimal type of different precision or scale is not specified.
- Parsing decimal values from strings, rounding, and decimal math functions are not defined by the language.
- A decimal literal whose expected type is not a decimal type is not specified.
- An integer literal whose expected type is `float`, `f32`, or `f64` is not specified.
- An integer literal outside the `int` range with no wider expected type is not specified.
- Integer overflow in `+`, `-`, `*`, and `**` is not specified, for `int` and for every exact-width integer type. Floor division overflow is defined in [Floor division](#floor-division).
- A shift (`<<`, `>>`) by a negative amount, or by at least the bit width of its left operand, is not specified.
- An integer-family value is never converted to a float-family type implicitly, and `resize()` refuses that conversion.
- Literal type suffixes other than the decimal `d` (`42i32`, `1.0f32`) are not part of the syntax.
