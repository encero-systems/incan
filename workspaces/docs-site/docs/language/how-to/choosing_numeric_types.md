# Choosing numeric types

This how-to guide helps you choose numeric annotations in new Incan code.

For exact rules, see [Numeric semantics](../reference/numeric_semantics.md). For the design rationale, see [Why numeric types work this way](../explanation/numeric_types.md).

!!! note "Coming from Python?"
    Python usually lets you start with `int` and think about range only when a library boundary complains. Incan has more numeric spellings because it needs to model Rust APIs, data files, database schemas, analytics engines, and fixed-scale decimal values without hidden casts.

    Start with `int` and `float` for ordinary Incan-owned logic, then choose a narrower, wider, unsigned, decimal, or schema-shaped alias only when the representation is part of the contract you are writing against.

## Use `int` and `float` for ordinary Incan-owned code

Use `int` for ordinary whole-number logic and `float` for ordinary binary floating-point logic.

```incan
retries: int = 3
timeout_seconds: float = 2.5
```

Do not pick `i8`, `i16`, `u8`, or `u16` just because today's values are small. Pick exact widths when the width is part of an external contract or storage format.

## Match exact widths at external boundaries

Use exact-width types when a Rust API, data file, wire format, FFI boundary, or schema owns the representation.

```incan
model EncodedHeader:
    version: u8
    flags: u16
    payload_len: u32
```

Convert inward if the rest of the program does not need the exact width.

```incan
model RawEvent:
    sensor_id: u32
    sequence: u64

model Event:
    sensor_id: int
    sequence: int
```

## Use schema-shaped aliases in schema-shaped code

Use aliases when they keep a database, analytics, or interchange schema readable.

```incan
model WarehouseRow:
    id: bigint
    category_id: integer
    priority: smallint
    score: double
```

Aliases canonicalize to exact Incan types.

| Alias | Canonical type |
| ----- | -------------- |
| `smallint`, `short` | `i16` |
| `integer` | `i32` |
| `int`, `bigint`, `long` | `i64` |
| `hugeint` | `i128` |
| `real`, `fp32` | `f32` |
| `float`, `double`, `fp64` | `f64` |
| `numeric[p, s]` | `decimal[p, s]` |

Use canonical names when the exact width is the important thing. Use aliases when matching source vocabulary matters more.

## Match Rust numeric parameters instead of relying on casts

When a Rust function expects a primitive width, annotate the Incan value at that width near the call boundary.

```incan
from rust::metrics_core import record_code

code: i32 = 200
record_code(code)
```

Lossless widening is accepted.

```incan
from rust::metrics_core import record_total

count32: i32 = 100
record_total(count32)
```

Narrowing is rejected. Pick a policy before calling the Rust function.

```incan
from rust::metrics_core import record_code

count: int = 200
maybe_code: Option[i32] = count.try_resize()

match maybe_code:
    case Some(code): record_code(code)
    case None: println("count is outside i32")
```

## Use decimals for fixed-scale values

Use `decimal[p, s]` or `numeric[p, s]` when base-10 precision and scale are part of the value.

```incan
unit_price: decimal[12, 2] = 19.99d
tax_rate: numeric[6, 4] = 0.0825d
```

Precision is the total digit budget. Scale is the fractional digit budget.

```incan
ok: decimal[6, 2] = 1234.56d
bad_scale: decimal[6, 2] = 123.456d
bad_precision: decimal[6, 2] = 12345.67d
```

Decimal arithmetic is not general language behavior yet. Use decimals today for typed boundaries, literal validation, formatting, generated Rust, and display.

## Pick a resize policy before narrowing

Use `resize()` only when the conversion is lossless.

```incan
small: i8 = 120
wide: int = small.resize()
```

Use `try_resize()` when a value may not fit and failure should be data.

```incan
incoming: int = 240
maybe_small: Option[i8] = incoming.try_resize()
```

Use `wrapping_resize()` only when modulo-width behavior is intended.

```incan
raw: u16 = 258
byte: u8 = raw.wrapping_resize()
```

Use `saturating_resize()` when clipping to the target range is intended.

```incan
sample: i16 = 500
clipped: i8 = sample.saturating_resize()
```

## Avoid unsigned integers as validation

Unsigned types describe representation. They are not a replacement for checking user input or business rules.

```incan
def set_retry_count(count: int) -> Result[int, str]:
    if count < 0:
        return Err("retry count cannot be negative")
    return Ok(count)
```

Use unsigned types when the boundary is genuinely unsigned.

```incan
model EncodedHeader:
    byte_len: u32
    checksum: u64
```

## Review checklist

1. Does another system own the width or precision?
2. Does this type appear in a public API, file format, Rust call, or schema?
3. If the conversion can lose data, is the policy explicit in source?
4. Would `int`, `float`, or `decimal[p, s]` be clearer for Incan-owned logic?
5. If using `usize`, is the value actually tied to a Rust or platform-sized API?
