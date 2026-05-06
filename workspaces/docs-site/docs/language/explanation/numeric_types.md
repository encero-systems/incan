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

The aliases are intentionally not nominal types. `integer` and `i32` are the same type after resolution. This keeps schema-shaped source readable without multiplying the semantic type universe.

## External data systems influenced the shape

Apache Arrow describes its columnar format around typed arrays and physical layouts; its type table includes integer bit width and signedness, floating-point precision, and decimal precision/scale as parameters. Substrait similarly treats type class, nullability, variation, and parameters as the components of a type, with examples such as `i8`, `fp32`, and `DECIMAL<10, 2>`.

Incan is not copying either system wholesale. Arrow is a memory format and Substrait is a relational algebra serialization format. The relevant lesson is narrower: analytics boundaries need width and scale to be representable in source without translation games.

## Lossless conversion is the implicit line

The conversion rule is deliberately simple: implicit numeric movement is allowed when it is exact or provably lossless, and rejected when it may lose data.

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

## Resize methods put data loss in source

Narrowing can be correct. It just needs to say what should happen when the value does not fit.

`try_resize()` says failure is data. `wrapping_resize()` says fixed-width wraparound is intended. `saturating_resize()` says clipping is intended. `resize()` says no data loss is allowed.

That makes code review sharper. A reviewer can accept or challenge the policy by reading the method name, rather than discovering it in generated Rust or runtime behavior.

## Rust interop follows the same rule

Rust APIs often encode numeric decisions in parameter types. If Rust expects `i64`, passing an Incan `i32` should be painless. If Rust expects `i32`, passing an Incan `int` should not silently downcast.

Keeping Rust interop on the same exact-or-lossless rule prevents a separate "interop cast system" from growing at the boundary. It also means code that typechecks for an Incan assignment is aligned with code that typechecks for a Rust scalar argument.

## Decimal is in scope, arithmetic is not yet

Decimal types are included because fixed-scale values are central to data, finance, and analytics code. Precision and scale belong in the type because they define what values can be represented.

Decimal arithmetic is a separate language-design problem. Addition, multiplication, division, rounding, overflow, scale propagation, and aggregation need explicit rules. The implemented surface therefore stops at decimal type syntax, literal validation, formatting, runtime representation, generated Rust, and display.

## What this design does not claim

This design does not make every numeric operation maximally precise, does not define arbitrary-precision integers, and does not turn `usize` into a general positive integer type. It also does not claim that aliases are always better than canonical names.

The rule of thumb is: use ordinary names for ordinary code, exact names for exact boundaries, aliases for schema vocabulary, and explicit resize methods whenever data loss is possible.
