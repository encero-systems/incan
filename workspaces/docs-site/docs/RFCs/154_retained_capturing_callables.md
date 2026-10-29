# RFC 154: Retained capturing callables

- **Status:** Draft
- **Created:** 2026-10-17
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 088 (lazy iterator adapters)
    - RFC 103 (protected captures and diagnostics)
    - RFC 115 (fallible iterator callbacks)
    - RFC 128 (callable types and invocation guarantees)
    - RFC 129 (binding and ownership)
    - RFC 143 (explicit cloning)
    - RFC 153 (async callback lifetimes)
- **Issue:** [#1985](https://github.com/encero-systems/incan/issues/1985)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Propose a uniform ownership contract for capturing closures retained by lazy adapters, collections, fields, returned values, and library boundaries. A supported slot must retain the callable and the captured values it needs until its last permitted invocation, preserving Incan's existing snapshot-at-construction behavior. It must diagnose an unsupported lifetime or ownership requirement rather than rejecting all captures because a slot happens to use a function pointer. This draft also covers the stored-slot need tracked by issue #1987.

## Motivation

An immediate Result combinator accepts a closure that reads a local, while a lazy iterator adapter currently refuses the corresponding callback. The application sees the same projection and a similar map call; the difference is how long the callback must survive. RFC 128 supplies callable typing, but explicitly leaves capture policy unchanged. This RFC owns that missing lifetime and storage contract rather than redesigning callable signatures.

## Goals

- Make retained callbacks useful across ordinary typed storage and package boundaries.
- Preserve capture values as observed when the closure is constructed.
- Explain whether retained captures can safely outlive their original scope.
- Keep synchronous and lazy execution timing distinct.
- Preserve efficient non-capturing function values without promising one backend representation.

## Non-Goals

- Implicit writable references to captured locals.
- Silently copying, cloning, or moving a value that ordinary ownership rules do not permit.
- Making every callable Send, Static, repeatable, or concurrently reusable.
- Changing RFC 128's `Callable[...]` annotations, shared signatures, or invocation markers.
- Making lazy adapters eager.

## Guide-level explanation

The iterator methods and closure syntax are existing. Accepting the retained capture in this example is the proposed change; the application function is complete.

```incan
def scaled(items: list[int], factor: int) -> list[int]:
    return items.iter().map((value) => value * factor).collect()
```

The adapter retains the projection until consumption. It must retain factor safely rather than ask the caller to lift this local multiplier into an unrelated named function.

Snapshot behavior matters even when consumption happens later:

```incan
from std.traits.callable import Callable

def snapshot_example(items: list[int]) -> list[int]:
    mut factor = 2
    projection: Callable[(int) -> int] = (value) => value * factor
    pending = items.iter().map(projection)
    factor = 9
    return pending.collect()
```

The proposed result uses 2 for every item. The body is still lazy: the input is not traversed when pending is constructed. Capturing factor and invoking the projection are separate events.

A typed callback field is another retained slot:

```incan
from std.traits.callable import Callable

model Adjustment:
    apply: Callable[(int) -> int]


def offset_adjustment(offset: int) -> Adjustment:
    return Adjustment(apply=(value) => value + offset)
```

The returned Adjustment must remain usable after offset_adjustment returns. The `Callable[...]` annotation describes the accepted call contract; this proposal removes the storage limitation for safely retainable captures. It does not reinterpret the field as a fresh function-pointer-only API.

### A returned closure has the same callable contract

```incan
from std.traits.callable import Callable

type Multiplier = Callable[(int) -> int]

def make_multiplier(factor: int) -> Multiplier:
    return (value) => value * factor

def example() -> int:
    times_three = make_multiplier(3)
    return times_three(14)  # 42, after the factory returned.
```

The alias describes how to call the returned value. The closure retains `factor`; returning it does not execute its body. An expanded return annotation is `Callable[(int) -> int]`, rather than a second bare arrow after the function's return arrow. Nested callable-valued parameters and results also use `Callable[...]`.

A compatible named function or a model/class explicitly adopting that same `Callable[...]` contract can be passed to the same annotated API. This does not erase its concrete identity or state. Signature compatibility alone never grants mutable receiver access, repeated consuming calls, capture retention, or concurrent reuse. Whether a callback executes immediately or later remains the consumer's contract.

## Reference-level explanation

- Callable-signature value annotations must follow RFC 128's mandatory `Callable[...]` spelling or a named alias. This typing rule accepts functions, closures, and nominal adopters; it does not settle the capture categories below.
- Capture reads must observe the captured local's value at closure construction, following existing Incan capture semantics.
- Every retained slot must preserve the callable's permitted invocation behavior and keep required captured values alive.
- A lazy adapter must not invoke its callback until its ordinary consumption operation requires it.
- Returning, storing, or passing a callable across a module boundary must not erase capture requirements from checking.
- Unsupported retention must produce a source diagnostic naming the value, slot, and failed ownership or lifetime obligation.
- A callable must not acquire mutation permission over a captured local merely because it is retained.
- A non-copyable or non-cloneable capture must not be duplicated implicitly to make retention succeed. Ordinary ownership rules still decide transfer and borrowing.
- Consuming an iterator in the same expression may prove a shorter lifetime; it must not change the snapshot value semantics.
- Captured values' secret protections must survive callback storage and diagnostic inspection.
- Implementation choices may specialize non-capturing values, but the public type contract must not expose arbitrary same-module storage exceptions.

## Design details

The recommendation is one semantic rule across Result combinators, iterator adapters, and user callbacks, with different required lifetimes rather than different definitions of capture. A short-lived borrow can satisfy an immediate call while failing an escaping slot; that difference should be explained concretely.

Issue [#1987](https://github.com/encero-systems/incan/issues/1987) proposes a boxed representation for kept slots. That is one implementation option, not this RFC's source contract. RFC 128's invocation guarantees and generic concrete callable types may allow a cheaper representation when the type remains known. Whether snapshot retention of a mutable aggregate requires an explicit clone or is rejected needs a deliberate rule; a scalar example cannot settle that.

## Alternatives considered

- Comprehensions only for capture-dependent pipelines: workable, but leaves callback-taking APIs inconsistent and excludes callback registries.
- Box every function value: uniform implementation, with potentially unnecessary allocation and indirection.
- Capture outer bindings by reference automatically: contradicts existing snapshot behavior and complicates mutation expectations.

## Drawbacks

Safe retention can require owned storage and allocation. Some non-cloneable captures cannot support repeatable callbacks. Uniform source typing still needs useful explanations for lifetime-dependent refusals.

## Layers affected

- **Typechecker / ownership:** retention requirements, captures, and invocation permissions.
- **Callable storage / emission:** representations for kept values without leaking backend restrictions.
- **Stdlib:** lazy adapter and cross-boundary callback contracts.
- **Diagnostics / metadata:** capture-safe contracts across library boundaries.

## Unresolved questions

- Which capture categories are copied, moved, borrowed, or require an explicit clone under the existing ownership model?
- How should mutable aggregate snapshots be expressed without introducing hidden sharing or copies?
- What annotation, if any, distinguishes an immediate callback parameter from a retained one in a public API?
- How are heterogeneous capturing values stored under one `Callable[...]` collection element?
- Which retention facts belong in exported callable metadata, and how do they interact with async invocation?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
