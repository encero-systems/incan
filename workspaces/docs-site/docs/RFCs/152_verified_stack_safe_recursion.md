# RFC 152: Verified stack-safe self-recursion

- **Status:** Draft
- **Created:** 2026-10-17
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 036 (declaration decorators)
    - RFC 094 (scoped cleanup obligations)
    - RFC 126 (investigation evidence)
    - RFC 129 (binding and ownership)
    - RFC 131 (diagnostic provenance)
- **Issue:** [#1290](https://github.com/encero-systems/incan/issues/1290)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Propose `@stack_safe` for direct self-recursive functions and methods. Every recursive call must be verifiably in tail position, and accepted recursion must execute through explicit loop semantics without recursive stack growth. A non-tail call is a compile-time error, not an optimization silently skipped. This guarantee is local to the annotated recursion and does not promise termination or bounded behavior in arbitrary callees.

## Motivation

A recursive traversal may work on a small fixture and overflow on production input. Backend optimization is insufficient as a language contract. Authors need a declaration that either guarantees the bounded recursive frame behavior or explains exactly which call prevents it.

## Goals

- Check direct self-recursion at Incan source locations.
- Guarantee bounded stack use for the accepted recursive transitions.
- Preserve evaluation order, effects, parameter binding, and cleanup.
- Explain unsupported recursive shapes before execution.

## Non-Goals

- Mutual recursion, recursion through function values, or dynamic dispatch optimization.
- Automatically decorating every recursive function.
- Proving termination, numeric safety, or bounded heap use.
- Discarding pending resource cleanup to manufacture a tail position.

## Guide-level explanation

The decorator is proposed. This complete application function computes a greatest common divisor for non-negative inputs:

```incan
@stack_safe
def gcd(a: int, b: int) -> int:
    if b == 0:
        return a
    return gcd(b, a % b)
```

The recursive transition first evaluates both new arguments using the current values, then starts the next iteration. It must not assign `a = b` before computing `a % b`.

This related shape must be rejected:

```incan
@stack_safe
def recursive_sum(n: int) -> int:
    if n <= 0:
        return 0
    return n + recursive_sum(n - 1)
```

Addition remains after the call returns. The diagnostic should point to that recursive call and explain the pending operation. It must not accept the decorator while leaving stack growth unchanged.

## Reference-level explanation

- Every direct recursive call to the resolved annotated declaration must be checked. Spelling an alias must not evade the check.
- Each accepted transition must have no required work after its callee completes, except behavior the compiler proves equivalent without retaining recursive frames.
- Successful verification must guarantee execution without stack growth caused by those self-calls, independently of optimization level.
- Recursive argument expressions must evaluate exactly once in ordinary call order, before parameter rebinding.
- Default arguments, keyword binding, ownership transfers, and effects must retain ordinary call semantics.
- A call with pending scoped cleanup must not be treated as tail merely because it appears in a return statement.
- Unsupported or ambiguous recursion must be rejected when it prevents establishing the guarantee; the compiler must not silently weaken it.
- Diagnostics and investigation metadata must identify the verified loop transformation without fabricating a physical recursive stack trace.

## Design details

Scala's [tail-recursion annotation](https://docs.scala-lang.org/tour/annotations.html) provides the relevant prior art: the annotation requests verification of a guarantee, rather than a suggestion to an optimizer. The candidate name `stack_safe` emphasizes the user outcome; `tail_recursive` is an alternative with a narrower apparent promise.

The recommendation is to reject active resource scopes whose per-frame cleanup cannot be preserved with bounded storage. A syntactic `return self_call(...)` inside such a scope can still have cleanup after the call, as RFC 094 makes explicit. Async functions and overridable method recursion need separate eligibility rules before this draft can be accepted.

## Alternatives considered

- Rely on backend tail-call optimization: does not supply the declaration's guarantee.
- Rewrite all recursion without annotation: loses the explicit contract and still needs proof of semantic equivalence.
- Offer only a warning: reports risk without establishing bounded recursive stack growth.

## Drawbacks

The accepted subset can be narrower than an author expects. The name may imply that unrelated helper calls are safe; diagnostics and documentation must state the exact guarantee. Investigation views need to distinguish source recursion from physical loop execution.

## Layers affected

- **Typechecker / control-flow analysis:** recursive identity and tail-position verification.
- **IR / execution:** explicit loop behavior preserving argument and cleanup semantics.
- **Diagnostics / investigation tooling:** proof boundaries and truthful execution presentation.

## Unresolved questions

- Is `stack_safe` or `tail_recursive` the clearer public name?
- Should annotating a function with no recursive calls be an error or a warning?
- Which methods qualify when overrides or receiver changes can affect dispatch?
- Are async functions excluded, or can a bounded future-state guarantee be stated and verified?
- Which cleanup-bearing scopes can be proven equivalent, and which must be refused?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
