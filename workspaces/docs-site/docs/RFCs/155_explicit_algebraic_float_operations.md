# RFC 155: Explicit algebraic floating-point operations

- **Status:** Draft
- **Created:** 2026-10-17
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 005 (Rust interop)
    - RFC 085 (checked constraints)
    - RFC 122 (deterministic construction checks)
    - RFC 126 (execution evidence)
- **Issue:** [#1175](https://github.com/encero-systems/incan/issues/1175)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Propose explicit named algebraic operations on float, allowing relaxed arithmetic only at the marked operation. Ordinary operators retain their current semantics. Fused multiply-add is a separate numerical operation with a single rounding, rather than an alias for relaxed multiply followed by add. The recommended per-operation surface remains a draft choice; its exact placement and backend reachability require review.

## Motivation

Numeric loops sometimes permit reassociation or related algebraic transformations, while neighboring accounting or validation logic does not. A source-visible operation lets a reviewer inspect that boundary. A global compilation mode would let build settings change arithmetic in code whose author never opted in.

## Goals

- Keep relaxed arithmetic visible at each selected operation.
- State the numerical contract without promising a speedup.
- Distinguish algebraic relaxation from fused multiply-add.
- Preserve ordinary arithmetic and explicit governance of relaxed computations.

## Non-Goals

- Global fast-math flags or implicit relaxed arithmetic.
- A claim that ordinary binary floating-point arithmetic is exact real arithmetic.
- Guaranteed vectorization, platform-independent relaxed results, or benchmark improvements.
- Introducing new authored Rust merely to avoid an Incan authoring or interop limitation.

## Guide-level explanation

The following float methods are proposed. The application functions are complete; they require no implicit imports.

```incan
def ordinary_score(left: float, right: float, bias: float) -> float:
    return left * right + bias


def fused_score(left: float, right: float, bias: float) -> float:
    return left.mul_add(right, bias)


def relaxed_score(left: float, right: float, bias: float) -> float:
    return left.algebraic_mul(right).algebraic_add(bias)
```

These three forms do not promise identical rounding. Ordinary multiply and add can round separately. Fused multiply-add rounds the combined operation once. Algebraic operations permit the backend's documented relaxed transformations. None of these signatures promises exact real-number computation.

A deliberate use might be a scoring loop whose application tolerances are checked against representative data. This RFC does not declare a workload tolerant merely because it uses floats.

## Reference-level explanation

- Ordinary float operators must not acquire algebraic relaxation because another expression or dependency opts in.
- The candidate methods are `algebraic_add`, `algebraic_sub`, `algebraic_mul`, `algebraic_div`, and `algebraic_rem`, each taking a float operand and returning a float.
- Their numerical permissions and special-value behavior must match the selected Rust operation contract, documented at the Incan boundary.
- `mul_add(multiplier: float, addend: float) -> float` must preserve its fused, single-rounding contract; it must not be implemented as an ordinary separate multiply and add.
- Checked metadata and investigation views must preserve that the operation was relaxed or fused.
- A relaxed operation must not automatically qualify as reproducible deterministic validation under RFC 122. Any permitted use there must satisfy that RFC's construction-safety contract explicitly.
- Unsupported backend reachability must be tracked as an Incan limitation before introducing a removable workaround. Existing Rust crates and interop should be used directly where sufficient.

## Design details

Rust documents [algebraic float operations](https://doc.rust-lang.org/std/primitive.f64.html#algebraic-operators) and their per-operation permissions. Its [mul_add contract](https://doc.rust-lang.org/std/primitive.f64.html#method.mul_add) specifies a fused operation with one rounding; it may differ from the corresponding separate expressions. These are distinct contracts, not two names for a single fast-math setting.

The recommendation is named operations rather than a float mode, decorator, or block. Whether they are inherent methods on Incan float or functions in a numeric module remains open. The draft does not assume float has already become an ordinary Incan-authored wrapper, nor that inherent Rust primitive methods are already reachable through the current interop surface.

## Alternatives considered

- A global flag: too broad and invisible at the operation.
- A relaxed float type: visible in signatures but introduces conversions throughout otherwise ordinary code.
- A function decorator or block: easier to write, but makes individual arithmetic depend on a surrounding mode.
- Only Rust interop: useful where supported; it does not establish a consistent Incan numeric contract by itself.

## Drawbacks

Explicit operation names are verbose. Relaxed results can vary across targets and optimizations. Numerical suitability needs application evidence, and a source marker does not prove a useful performance improvement.

## Layers affected

- **Numeric public surface / checked metadata:** explicit operation contracts.
- **Interop or numeric lowering:** faithful access to supported backend operations.
- **Diagnostics / investigations:** visible numerical permissions and reproducibility boundaries.

## Unresolved questions

- Are named methods the right surface, or should a numeric module expose equivalent functions?
- Should mul_add land as an ordinary float method independently of algebraic relaxation?
- What exact backend special-value permissions and documentation are exposed for each operation?
- Which construction-safe or governed computation contexts may admit relaxed arithmetic?
- Which existing Incan or interop capability can express the operations without a new Rust shim?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
