# RFC 073: Dynamic Call-Site Unpacking for Fixed Parameters

- **Status:** Draft
- **Created:** 2026-04-24
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 038 (Variadic positional args and keyword-argument capture)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

Allow `f(*xs)` and `f(**kw)` to bind ordinary fixed parameters when the callee has no matching rest parameter, using Python-style argument expansion rules adapted to Incan's static type system.

## Motivation

RFC 038 deliberately limits call-site unpacking to rest-aware callees. That keeps the first implementation simple and statically explicit, but it leaves a common ergonomic gap: callers often hold a tuple/list of fixed positional values or a dictionary of fixed keyword values and want to pass those values to an ordinary function without declaring a rest parameter on the callee. This RFC proposes the broader call-binding model as a separate feature so its arity, duplicate-binding, typing, and diagnostic rules can be reviewed without weakening RFC 038's rest-parameter contract.

## Goals

- Permit `*expr` to satisfy ordinary positional parameters when the compiler can prove the unpacked shape.
- Permit `**expr` to satisfy ordinary named parameters when the compiler can prove the available keys.
- Define deterministic duplicate-binding and missing-argument diagnostics for mixed ordinary arguments and unpacking.
- Preserve RFC 038 behavior for callees that do declare `*args` or `**kwargs`.
- Keep generated code explicit: fixed-parameter calls still lower to ordinary positional Rust arguments after binding is resolved.

## Non-Goals

- This RFC does not add dynamic runtime reflection over arbitrary dictionaries.
- This RFC does not make ordinary `Dict[str, T]` values acceptable as `**kwargs` for fixed parameters unless key presence can be checked statically or through an explicitly accepted runtime-checking rule.
- This RFC does not change the type of `*args` or `**kwargs` rest bindings from RFC 038.
- This RFC does not introduce keyword-only parameters.

## Guide-level explanation

Users may unpack a statically shaped positional value into fixed parameters:

```incan
def point(x: int, y: int) -> str:
    return f"{x},{y}"

def main() -> str:
    xy: tuple[int, int] = (3, 4)
    return point(*xy)
```

Users may also unpack a statically shaped keyword value into fixed named parameters:

```incan
def user(name: str, age: int) -> str:
    return f"{name}:{age}"

def main() -> str:
    fields = {name="Ada", age=37}
    return user(**fields)
```

The exact shaped-key syntax in the second example is illustrative. This RFC must settle which existing or new type surface can represent statically known keyword keys before it can move beyond Draft.

## Reference-level explanation

For a call expression, the compiler must build one ordered binding plan across ordinary positional arguments, ordinary named arguments, positional unpack arguments, and keyword unpack arguments.

`*expr` may bind ordinary fixed positional parameters when `expr` has a statically known ordered shape. The minimum accepted shape is `tuple[T1, T2, ...]`; accepting homogeneous `List[T]` for a fixed number of parameters is an unresolved question because list length is not normally a static type fact.

`**expr` may bind ordinary fixed named parameters when `expr` has a statically known mapping from string keys to value types. The compiler must reject an unpacked mapping when a required key is absent, when an unknown key cannot be captured by a `**kwargs` rest parameter, or when a value type is incompatible with the fixed parameter it binds.

If a parameter is bound more than once, the compiler must report a duplicate-argument error. This includes combinations such as `f(1, *xy)` where `*xy` also supplies the first parameter, and `f(name="Ada", **fields)` where `fields` also supplies `name`.

If unpacking cannot be proven statically but the implementation chooses to support runtime-checked unpacking for a particular shaped mapping type, the compiler must make that behavior explicit in diagnostics and lowering. Silent best-effort binding is not permitted.

## Design details

### Syntax

No new call syntax is required beyond RFC 038: this RFC extends where existing `*expr` and `**expr` call arguments are legal.

### Semantics

Call binding is evaluated left to right for duplicate detection, but final emission still follows the callee's declared parameter order. Rest-aware callees keep RFC 038 behavior: surplus positional arguments and `*expr` values flow into `*args`, and surplus named arguments and `**expr` values flow into `**kwargs`. For callees without rest parameters, unpacked values may bind fixed parameters only when their shape is accepted by this RFC.

### Interaction with existing features

Function values must preserve enough callable metadata for the same binding rules to apply when the callee is a variable. Generic functions should infer type parameters from the fixed parameters that unpacked values bind. Rust interop calls should not accept dynamic unpacking unless the Rust metadata surface provides an equivalent fixed-parameter binding contract.

### Compatibility / migration

This is additive. Calls rejected under RFC 038 because the callee lacks a matching rest parameter may become valid when the unpacked expression has a statically accepted shape.

## Alternatives considered

- Keep unpacking rest-only forever. This is simpler, but it leaves Incan noticeably less ergonomic than Python for fixed-arity forwarding helpers.
- Permit `Dict[str, T]` for all `**expr` fixed-parameter binding with runtime missing-key checks. This is flexible but weakens static guarantees and makes ordinary calls capable of failing at runtime for shape reasons.
- Require users to spell every forwarded argument explicitly. This is predictable but creates boilerplate at API boundaries.

## Drawbacks

The feature adds a second call-binding mode beyond RFC 038 and therefore increases diagnostic and lowering complexity. It may also require a new statically shaped record/map surface if existing dictionary types cannot express known keyword keys cleanly.

## Layers affected

- **Parser / AST**: no new syntax is required if RFC 038 call-argument nodes are reused.
- **Typechecker / Symbol resolution**: call binding must accept unpacking into fixed parameters only when the unpacked value has an accepted static shape, and must report duplicate, missing, unknown, and incompatible argument diagnostics.
- **IR Lowering**: fixed-parameter unpacking must lower to ordinary ordered call arguments after binding is resolved.
- **Emission**: generated Rust calls must remain ordinary fixed-arity calls; runtime unpack blocks are allowed only for explicitly accepted runtime-checked cases.
- **Formatter**: no new formatting beyond RFC 038 should be required.
- **LSP / Tooling**: signature help and diagnostics should explain which fixed parameters an unpack expression binds.

## Unresolved questions

- What type surface represents a statically known keyword mapping: model literals, a new record-like literal, a typed dict form, or something else?
- Should homogeneous `List[T]` ever be accepted for fixed-parameter `*expr`, or should only fixed-length tuple shapes be accepted?
- Should any runtime-checked `**dict` mode exist, or should fixed-parameter keyword unpacking remain strictly static?
- How should default parameters interact with `**expr` when a key is absent?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
