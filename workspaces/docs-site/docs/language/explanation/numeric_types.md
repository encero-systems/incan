# Why numeric types work this way

Incan's numeric design has two jobs that pull in different directions: ordinary Incan code should stay ergonomic, and boundary-heavy data/Rust code should be explicit enough that widths, precision, and lossy conversions are visible in source.

## The end state

The intended end state is a language where most application code can use `int`, `float`, and `decimal[p, s]` without ceremony, while schema and interop code can name exact widths without casts or guesswork.

That means Incan has ordinary numeric spellings, exact-width spellings, and data-oriented aliases. These are not three competing models. They are one numeric model seen from three practical contexts.

## Ordinary code keeps ordinary names

`int` and `float` remain the right defaults for code that Incan owns. They are easy to read, stable as user-facing names, and avoid forcing every loop counter, retry count, or timeout calculation to make a storage-width decision.

Exact-width types become useful when the width is part of the contract. A packet field, Arrow buffer, SQL column, Rust API parameter, or generated ABI surface is different from an ordinary local variable. In those cases the type annotation should say what the boundary says.

## Aliases are for shared vocabulary

Aliases such as `smallint`, `integer`, `bigint`, `hugeint`, `real`, `double`, `fp32`, and `fp64` exist because Incan is intended to work well in data and analytics settings. Those ecosystems already have vocabulary for fixed-width and floating-point schema fields.

The aliases are intentionally not nominal types. `integer` and `i32` are the same type after resolution. This keeps schema-shaped source readable without multiplying the semantic type universe. `int` and `float` are aliases in the same sense: they are the ordinary spellings of `i64` and `f64`, the two default widths.

## External data systems influenced the shape

Apache Arrow describes its columnar format around typed arrays and physical layouts; its type table includes integer bit width and signedness, floating-point precision, and decimal precision/scale as parameters. Substrait similarly treats type class, nullability, variation, and parameters as the components of a type, with examples such as `i8`, `fp32`, and `DECIMAL<10, 2>`.

Incan is not copying either system wholesale. Arrow is a memory format and Substrait is a relational algebra serialization format. The relevant lesson is narrower: analytics boundaries need width and scale to be representable in source without translation games.

## Lossless conversion is the implicit line

The conversion rule is deliberately simple: implicit numeric movement is allowed when it is exact or provably lossless within one family (signed integers, unsigned integers, binary floats) or from an unsigned integer to a wider signed one, and rejected when it may lose data. Integer-to-float movement is never implicit, even where every value would fit.

This admits common safe cases without user friction:

```incan
small: i8 = 120
wide: int = small
```

It rejects the cases reviewers need to notice:

```incan
wide: int = 240
small: i8 = wide
```

The second example might work for the value `240` if the target were unsigned, or might fail for other runtime values. The language should not hide that policy decision.

## Widening converts values, not types

A lossless widening is a conversion of one value, made where the value reaches its destination: a binding, an argument, a field, an `Option` payload, a union member. A `list[i8]` holds many values, and making it a `list[int]` would mean building a new list, so a numeric type inside a collection, tuple, `Option` or function type has to match exactly. Writing that conversion out (a comprehension, or a `resize()` per element) keeps the copy visible.

## Resize methods put data loss in source

Narrowing can be correct. It just needs to say what should happen when the value does not fit.

`try_resize()` says failure is data. `wrapping_resize()` says fixed-width wraparound is intended. `saturating_resize()` says clipping is intended. `resize()` says no data loss is allowed.

That makes code review sharper. A reviewer can accept or challenge the policy by reading the method name, rather than discovering it in generated Rust or runtime behavior.

## `float` is `f64`

`float` is the ordinary spelling of `f64`, and an alias is never a separate type, so `float` and `f64` have one value set: IEEE binary64 as Rust has it. Operations may produce NaN or infinity and carry them on, whichever spelling the code uses. Code that needs finite values checks for them (`is_finite()`), as it would for any IEEE float; a type spelling does not make that check for it.

`f32` is the one binary-float type distinct from `float`. It is a narrower storage format for boundaries, and it promises finite values. Generated code keeps that promise by checking an `f32` value where it enters or is observed: when it enters through a public function or Rust interop, when it is extracted from a field or collection index, when an `f32` call or arithmetic operation produces it, and before it is compared, formatted, or printed. Public and Rust-facing aggregates are not recursively scanned at ingress; each `f32` scalar is checked when Incan extracts or observes it. A failed check raises `ValueError`. An `f32` widens into `float` without loss, so that direction needs no check.

## Integer arithmetic keeps its type

Arithmetic over two values of one integer type yields that type, as it does in Rust: `i8 + i8` is an `i8` and `u16 * u16` is a `u16`. A result that silently became `int` would drop the width the code declared, and `n += 1` on an `i32` binding could not typecheck at all. An integer literal beside an exact-width value takes that value's type, as a literal at a destination does, so `n + 1` needs no suffix and `n + 300` for an `i8` is caught when the program is checked.

Operands of two different integer types have no result type until one of them is converted. Picking the wider type is not always possible (`i64` and `u64` hold different ranges), and a silent pick is exactly the kind of numeric policy the resize methods exist to put in source, so the program converts one operand with `resize()` or `try_resize()` first.

Comparison is different: it produces a `bool`, not a value of either type, so two integer types compare in the narrowest type that holds every value of both, and `small < count` needs no conversion. Only a pair that no integer type holds, such as `u128` beside a signed type, has to be converted first.

`/` stays true division, so it yields `float` whatever integer types it divides. `**` keeps an integer base's type only for a non-negative literal exponent; a computed exponent may be negative, which has no integer result, so that power is a `float`.

## Division keeps Python's meaning

`/` is true division, `//` floors toward negative infinity and `%` takes the sign of the divisor, as Python defines them, so arithmetic ported from Python keeps its results. Division by zero raises `ZeroDivisionError` for integers, unsigned ones included, and for floats too, instead of producing NaN or infinity. The message names the operation: `division by zero` for integer `/`, `integer division or modulo by zero` for integer `//` and `%`, and `float division by zero`, `float floor division by zero` or `float modulo` when an operand is a float.

## Rust interop follows the same rule

Rust APIs often encode numeric decisions in parameter types. If Rust expects `i64`, passing an Incan `i32` should be painless. If Rust expects `i32`, passing an Incan `int` should not silently downcast.

Keeping Rust interop on the same exact-or-lossless rule prevents a separate "interop cast system" from growing at the boundary. It also means code that typechecks for an Incan assignment is aligned with code that typechecks for a Rust scalar argument.

## Power groups the way Python groups it

`**` binds tighter than a prefix `-` or `~` on its left and looser than one on its right, so `-x ** 2` is `-(x ** 2)` and `2 ** -1` is `2 ** (-1)`. This is Python's grouping, and it matches mathematical notation, where −x² means the negation of x². Python code that writes `-x ** 2` for a negated square therefore computes the same value in Incan; raising a negative value to a power takes parentheses in both languages, as described in [Raise a negative value to a power](../how-to/choosing_numeric_types.md#raise-a-negative-value-to-a-power).

## Decimal is in scope, arithmetic is not yet

Decimal types are included because fixed-scale values are central to data, finance, and analytics code. Precision and scale belong in the type because they define what values can be represented.

Decimal arithmetic is a separate language-design problem. Addition, multiplication, division, rounding, overflow, scale propagation, and aggregation need explicit rules. The implemented surface therefore stops at decimal type syntax, literal validation, comparison, formatting, runtime representation, generated Rust, and display. Parsing decimal values from strings and rich decimal math stay library-owned until the language specifies those semantics.

## What this design does not claim

This design does not make every numeric operation maximally precise, does not define arbitrary-precision integers, and does not turn `usize` into a general positive integer type. It also does not claim that aliases are always better than canonical names. Integer overflow in general exact-width arithmetic is not yet a separately documented language contract, and neither is a shift by a negative amount or by at least the bit width of its left operand, or an integer literal beyond the `int` range where no wider type is expected. The range of an `isize` or `usize` literal is checked against the pointer width of the machine that runs the compiler, which can differ from the target's.

The rule of thumb is: use ordinary names for ordinary code, exact names for exact boundaries, aliases for schema vocabulary, and explicit resize methods whenever data loss is possible.
