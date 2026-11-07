# RFC 155: Relaxed floating-point arithmetic

- **Status:** Planned
- **Created:** 2026-10-17
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 005 (Rust interop)
    - RFC 009 (float methods)
    - RFC 025 (multi-instantiation trait dispatch)
    - RFC 028 (trait-based operator overloading)
    - RFC 122 (construction-safe callables)
    - RFC 163 (target support)
- **Issue:** [#1175](https://github.com/encero-systems/incan/issues/1175)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

`std.math.relax(x)` wraps a value in a small library type, `Relaxed[T]`, whose `+`, `-`, `*` and `/` allow the compiler to regroup and combine them, which is what lets a numeric loop use SIMD. `float` stays strict, so only values an author has relaxed are affected, and that is visible in source. Results may differ from strict arithmetic in the last digits and may vary between targets, builds and runs. Separately, `mul_add` gives `float` and `f32` a fused multiply-add with one rounding, which is more exact rather than less.

## Core model

1. **Ordinary float operators never change.** No flag, block or dependency relaxes them.
2. **Relaxation belongs to a value.** `relax(x)` makes a `Relaxed` value, so a reviewer can see where relaxing starts and ends.
3. **`Relaxed` is library code.** It uses ordinary operator traits, and the compiler has no special knowledge of it.
4. **It is a permission, not a promise.** The compiler may optimize, and the result may vary, but no speedup is guaranteed.
5. **`mul_add` is a different contract.** It is exact and repeatable, and is never split into two steps.

## Motivation

Floating-point addition is not associative: `(a + b) + c` can round differently from `a + (b + c)`. A compiler must therefore add strictly in order, which rules out splitting a sum across SIMD lanes. C's answer is `-ffast-math`, a build flag that relaxes every float operation in the program, including accounting or validation code whose author never agreed to it.

A dot product of 16,384 doubles shows what is at stake:

| Form | Time per 1,000 elements | Result |
| --- | --- | --- |
| Strict `+` and `*` | 543 ns | 8197.140653354998 |
| Rust's algebraic operations | 129 ns | 8197.140653355003 |
| `mul_add` | 837 ns | 8197.140653355 |

These were measured on 2026-11-04 on one core of an Apple M5 Max, using Rust's operations directly with `rustc` 1.99 nightly at `opt-level=3` and `target-cpu=native`. Relaxing was about four times faster and changed the last digits. `mul_add` was slower than strict arithmetic in this loop, so it is for exactness, not speed.

Incan's numeric loops deserve this speed where an author wants it, without putting anyone else's arithmetic at risk.

## Goals

- Let an author opt in to relaxed arithmetic for chosen values, visibly in source.
- Keep formulas readable, with ordinary operators.
- Keep `float` and its operators strict.
- Provide fused multiply-add as its own exact operation.
- Keep relaxed arithmetic out of validation and construction.

## Non-Goals

- Global fast-math flags, or relaxing blocks or functions.
- Guaranteed vectorization or a promised speedup.
- Platform-independent relaxed results.
- Relaxed remainder.

## Guide-level explanation

`Relaxed` and `mul_add` are proposed by this RFC.

### Relax a loop

A search service ranks documents by the similarity of their embeddings:

```incan
from std.math import relax

def dot(left: list[float], right: list[float]) -> float:
    mut total = relax(0.0)
    for index in range(len(left)):
        total += relax(left[index]) * right[index]
    return total.value
```

`relax(x)` wraps a value, an operation with a `Relaxed` value on the left stays relaxed, and `.value` gives the `float` back. The function's signature is unaffected, so nothing about relaxation leaks to its callers.

Longer formulas stay readable. A polynomial evaluated by Horner's method:

```incan
def score(x: float, a: float, b: float, c: float) -> float:
    return ((relax(a) * x + b) * x + c).value
```

`Relaxed[f32]` works the same way, with `relax[f32](0.0)` to start from a literal. Mixing `Relaxed[f32]` with `Relaxed[float]` needs an explicit conversion, as `f32` and `float` do.

### What you give up

A relaxed result may differ from the strict one in rounding and in the sign of zero, and may differ between targets, builds and runs. That is fine for ranking documents and wrong for a ledger, and the author decides which applies. NaN and infinity flow through `Relaxed[float]` as they do through `float`. `Relaxed[f32]` keeps `f32`'s promise of finite values, so a non-finite result raises `ValueError`.

`Relaxed` has `+`, `-`, `*`, `/`, their compound forms and negation, and nothing else:

- `/` keeps Incan's rule: a zero divisor raises `ZeroDivisionError`. Dividing many values by one divisor gets the full speedup, because the check only has to run once. With a different divisor for each element, the check limits it.
- `%` is not available. Incan's `%` takes the divisor's sign, so `-7.0 % 3.0` is `2.0`, where Rust's relaxed remainder would give `-1.0`, so it would disagree before any relaxing happened.
- Comparisons go through `.value`, so a decision made on a relaxed result is visible.

### Not in validation

A registered check, a fieldmethod or a `from_underlying` hook must be deterministic under RFC 122, so `Relaxed` operations are refused there. An `Invoice` check comparing `total` with the sum of its lines could otherwise pass on one machine and fail on another.

### Fused multiply-add

`a.mul_add(b, c)` computes `a * b + c` with one rounding instead of two. It is exact, it never changes between runs, and it is allowed everywhere, including validation. Risk and ledger code uses it to get the exact rounding error of a product, the building block of compensated sums:

```incan
def product_with_error(a: float, b: float) -> tuple[float, float]:
    product = a * b
    error = a.mul_add(b, -product)
    return (product, error)
```

If `mul_add` were ever split into a multiply and an add, `error` would always be `0.0`, and the algorithm would break silently.

## Reference-level explanation

### `Relaxed`

1. `std.math` must provide `relax[T](value: T) -> Relaxed[T]` and the type `Relaxed[T]`, for `T` of `float` and `f32`. `Relaxed[T]` must hold one public field, `value: T`, and adopt `Copy`. `relax` is how a value is made, and the type name is for annotations.
2. `Relaxed` must be written in Incan, using RFC 028's operator traits and RFC 005 interop to Rust's algebraic operations. The compiler must have no knowledge of it beyond those general facilities.
3. It must adopt `Add`, `Sub`, `Mul` and `Div` for a right operand of `Relaxed[T]` or `T`, each with output `Relaxed[T]`, along with their compound-assignment forms and `Neg`. A zero divisor must raise `ZeroDivisionError`, as `/` does on `T`.
4. It must not adopt `Mod`, `Eq` or `Ord`. A `%` or comparison must get a diagnostic pointing at `.value`.
5. A left operand of `T` with a right operand of `Relaxed[T]` must be refused under RFC 028's rules, and the diagnostic must suggest swapping or wrapping the left operand.
6. A `Relaxed` value must hold nothing beyond its value, and its operations must add no allocation or call overhead compared with the Rust operations they use.

### Numerical contract

7. A relaxed result may differ from the strict operator's by rounding and by the sign of zero, and may vary between targets, builds and runs. It must not be undefined behavior for any input.
8. NaN and infinity must flow through `Relaxed[float]` as through `float`. A non-finite `Relaxed[f32]` result must raise `ValueError`, as `f32` does.
9. Ordinary `float` and `f32` operators must not be affected by `Relaxed` anywhere in a program, including through a dependency.

### `mul_add`

10. `float` and `f32` must have `mul_add(multiplier, addend)` with one rounding under IEEE 754's fusedMultiplyAdd. Operands must have the receiver's type, and an unsuffixed literal must take it.
11. `mul_add` must never be lowered to a separate multiply and add, on any target, including one without a hardware instruction.

### Construction safety

12. `Relaxed`'s operations must not be certified as construction-safe under RFC 122, so using them in a fieldmethod, `from_underlying` hook or registered check must be refused by that RFC's classification. `mul_add` must be certified.

### Conformance workloads

| Workload | Required evidence |
| --- | --- |
| Strictness | The same arithmetic on `float` gives identical results in a program with and without `Relaxed` elsewhere. |
| Speed | A `Relaxed` dot product matches the equivalent hand-written Rust loop on the same machine. |
| Operators | `+`, `-`, `*`, `/`, their compound forms and negation work with `Relaxed` and plain right operands; `%`, comparisons and a plain left operand are refused with their diagnostics. |
| Division | A zero divisor raises `ZeroDivisionError`, and a fixed-divisor loop keeps the speedup with its check in place. |
| Special values | NaN and infinity flow through `Relaxed[float]`, and a non-finite `Relaxed[f32]` raises `ValueError`. |
| Fused | `a.mul_add(b, -(a * b))` gives the exact rounding error on every supported target. |
| Construction | `Relaxed` in a registered check is refused, and `mul_add` is accepted. |

## Design details

### Why a type

The opt-in has to attach to something. A type keeps ordinary operators, so a formula still reads as a formula, while the wrapping shows exactly which values are affected. Rust's `Wrapping<T>` takes the same approach for integer overflow. Conversions stay at the edges of a calculation, so a function's signature need not change.

### Why division keeps its check, and why no remainder

A relaxed value is still an Incan value, so `/` keeps its `ZeroDivisionError`. Measured the same way as the table above, per 1,000 elements: dividing by a fixed divisor took 510 ns strict and 107 ns relaxed with the check in place, because the check moves out of the loop. Dividing by a different divisor each time took 606 ns strict and 617 ns relaxed with the check, and 126 ns relaxed without it. The check therefore costs nothing in the common fixed-divisor case and limits the varying one, which is the right trade for keeping one rule for `/`. Incan's `%` takes the divisor's sign where Rust's takes the dividend's, so a relaxed `%` would disagree with `%` before any relaxing happened, and no workload needs it.

### Reaching Rust's operations

Rust's algebraic operations and `mul_add` are inherent methods on its primitive types. `Relaxed` reaches them through interop, so this RFC needs inherent primitive methods to be reachable, which is a general capability and not specific to floats. It does not depend on `float` becoming an Incan-source type. `mul_add` joins the existing `float` methods under RFC 009.

### Prior art

[Rust](https://doc.rust-lang.org/std/primitive.f32.html#algebraic-operators) stabilized `algebraic_add` and its siblings in 1.98, permitting regrouping, reciprocal division and disregarding the sign of zero, with no undefined behavior, and [`mul_add`](https://doc.rust-lang.org/std/primitive.f64.html#method.mul_add) is fused. swift-numerics' `Relaxed` namespace has per-operation `sum`, `product` and `multiplyAdd`, documented as a license to optimize. Julia separates `fma`, which always fuses, from `muladd`, which may, and has `@fastmath` for an expression or block. Zig's `@setFloatMode(.optimized)` switches a whole block. Go lets implementations fuse operations across statements, a cautionary example of invisible contraction. C and C++ have global `-ffast-math`. Python added `math.fma` in 3.13 with no relaxed mode.

## Alternatives considered

- **A global flag:** too broad and invisible, and makes results depend on how a program was built.
- **A block or function decorator:** arithmetic would depend on a surrounding mode, which is `-ffast-math` in miniature, and it would need compiler support.
- **Per-operation methods, `algebraic_add`:** Rust's names are long, and formulas become nested calls. `a.algebraic_mul(x).algebraic_add(b)` reads worse the longer it gets.
- **Shorter method names, `fast_add`:** the same nesting, a name that says what is gained and not what is given up, and a clash with Rust's unsafe `fadd_fast`.
- **Dropping the zero check on relaxed division:** faster with a different divisor each time, but `/` would then mean two things.
- **Leaving division out and asking for a reciprocal:** loses a real speedup and makes authors write what the compiler can do.
- **A relaxed remainder:** see "Why division keeps its check, and why no remainder".
- **Spelling construction `Relaxed(x)`:** reads as a type, where `relax(x)` reads as the action it is.
- **Comparisons on `Relaxed`:** would hide a branch on a varying value, where `.value` makes it visible.
- **`Relaxed` allowed in validation:** the same check could pass on one machine and fail on another.

## Drawbacks

Wrapping values and unwrapping with `.value` adds words to a loop. A plain left operand is refused, so `2.0 * x` has to be written `x * 2.0`. `Relaxed[f32]` keeps its finiteness check on each result, which may limit how much of the speedup it gets. Relaxed results vary, and only application evidence can say a workload tolerates that. `mul_add` can be slower than strict arithmetic, especially on targets without a hardware instruction, under RFC 163.

## Layers affected

- **Standard library:** `std.math.relax` and `Relaxed`, written in Incan.
- **Interop:** reaching inherent methods on Rust primitive types.
- **Float methods:** `mul_add`, under RFC 009.
- **Construction safety:** certifying `mul_add` and not `Relaxed`'s operations.
- **Documentation:** the `std.math` reference and the numeric semantics page, stating what relaxing gives up.

## Design Decisions

- **Surface:** `std.math.relax(x)` makes a `Relaxed[T]`, a library type with ordinary operators. `float` and its operators stay strict, and there is no flag, block or decorator.
- **Operations:** `+`, `-`, `*`, `/`, compound forms and negation. `/` keeps Incan's `ZeroDivisionError`, remainder is left out because Incan's differs from Rust's, and comparisons go through `.value`.
- **`mul_add`:** an ordinary method on `float` and `f32`, exact and fused, and never described as relaxed.
- **Special values:** stated in Incan terms. NaN and infinity flow through `float`, and `f32` keeps its finiteness promise.
- **Construction:** `Relaxed` is refused under RFC 122, and `mul_add` is certified.
- **Reachability:** through interop to Rust's inherent methods, with no dependency on `float` becoming an Incan-source type.
