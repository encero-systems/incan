# RFC 038: Variadic Positional Args and Keyword-Argument Capture (`*args` / `**kwargs`)

- **Status:** In Progress
- **Created:** 2026-03-07
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 035 (First-class named function references)
    - RFC 039 (`race` for awaitable concurrency)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/322
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

Add Python-style rest parameters (`*args` / `**kwargs`) to Incan function definitions and matching call-site unpacking for rest-aware callables:

- `*name: T` captures extra positional arguments and binds `name` as a `List[T]`
- `**name: V` captures extra named arguments and binds `name` as a `Dict[str, V]`
- `f(*xs)` expands a list-like value into a callee's positional rest capture when the callee signature carries rest metadata
- `f(**kw)` expands a dict-like value into a callee's keyword rest capture when the callee signature carries rest metadata

This is compile-time sugar. The surface syntax is ergonomic, but the lowered model stays explicit:

- a rest positional parameter lowers to an ordinary trailing `List[T]` parameter
- a rest keyword parameter lowers to an ordinary trailing `Dict[str, V]` parameter
- call sites that use the sugar are rewritten to construct those containers explicitly
- call-site unpacking is accepted only when the callee type/signature provides a statically known rest target

This RFC is not only about Python-style convenience. It is also a foundational library-design feature. It lets Incan express APIs that naturally accept a variable number of homogeneous inputs without proliferating fixed-arity helper families. A good example is the helper surface proposed in RFC 039, where a variadic `std.async.race(*arms: RaceArm[R])` is cleaner than a permanent `race2` / `race3` / `race4` ladder.

## Motivation

### Python-style APIs need a direct rest-parameter model

Many ergonomic APIs in Python rely on flexible call signatures:

- logging helpers that take any number of messages
- formatting utilities that accept many values
- configuration helpers that accept optional named tweaks without large "options models"

Incan already has named call arguments in some places and an AST/IR representation for them, but the language does not have a first-class way to:

- accept an unbounded number of positional arguments, or
- accept arbitrary, unknown named arguments

Adding `*args` / `**kwargs` provides a familiar, concise user experience while keeping Incan's runtime model explicit and static: a list is a list, a dict is a dict, and the types reflect that.

### Variadics are also about library architecture

Without variadics, APIs that are conceptually "one repeated thing" often degrade into fixed-arity ladders:

- `format2`, `format3`, ...
- `merge2`, `merge3`, ...
- `race2`, `race3`, ...

That is rarely the shape the user actually wants. The real abstraction is usually "zero or more values of a common packaged type".

This RFC gives Incan that abstraction directly.

### The important design insight: variadics are homogeneous

`*args` capture values of one element type:

```incan
def log(level: str, *msgs: str) -> None:
    ...
```

That means variadics are a good fit when repeated inputs can be packaged into one homogeneous type. For example, RFC 039 can use:

```incan
pub async def race[R](*arms: RaceArm[R]) -> R:
    ...
```

Each branch is first packaged as a `RaceArm[R]`, then the variadic parameter captures those arm values uniformly.

By contrast, variadics are not the right shape for an alternating heterogeneous API because the repeated units are not homogeneous until they are packaged. Like this for example:

```incan
race(awaitable_a, on_a, awaitable_b, on_b)
```

That distinction is important. This RFC is about giving Incan a clean homogeneous variadic model, not a magic "any sequence of argument shapes" feature.

## Goals

- Add `*name: T` and `**name: V` parameter forms.
- Add `f(*xs)` and `f(**kw)` call-site unpacking for calls to rest-aware callables.
- Specify them as compile-time sugar over explicit trailing container parameters.
- Preserve rest-parameter structure in callable metadata so named function values keep the same rest-call behavior as direct calls.
- Keep the semantics deterministic and type-directed.
- Preserve Python-like ergonomics without introducing runtime reflection.
- Enable cleaner library APIs that naturally take "zero or more packaged values".

## Non-Goals

- Dynamic runtime arity dispatch for unpacking into ordinary fixed parameters.
- C-style variadics or raw FFI variadics.
- Heterogeneous positional capture without packaging.
- An `Any` type for untyped keyword captures.
- A richer structured-options container for captured keyword arguments beyond `Dict[str, V]`.

## Guide-level explanation

### Variadic positional capture: `*args`

Use `*name: T` as the last positional-style parameter to accept any number of extra positional arguments. Inside the function, `name` is a `List[T]`.

```incan
def log(level: str, *msgs: str) -> None:
    for msg in msgs:
        println(f"[{level}] {msg}")

def main() -> None:
    log("info", "started", "listening", "ready")
    log("warn")  # ok: msgs is []
```

### Variadic keyword capture: `**kwargs`

Use `**name: V` as the final parameter to accept unknown named arguments. Inside the function, `name` is a `Dict[str, V]`.

```incan
def connect(host: str, port: int, **opts: str) -> None:
    if opts.contains("tls") and opts["tls"] == "true":
        println("TLS enabled")

def main() -> None:
    connect("localhost", 5432, tls="true", user="danny")
    connect("localhost", 5432)  # ok: opts is {}
```

This is especially valuable for boundary-style APIs that intentionally forward option bags to another system: readers, writers, HTTP clients, framework adapters, and plugin hooks.

A useful real-world analogy is Koheesio's `ExtraParamsMixin`, which separates declared model fields from pass-through extra options while keeping call sites ergonomic. This is quite a common pattern in Python libraries.

The important difference in Incan is that `**kwargs` remains explicit and typed:

- unknown named arguments are still rejected by default unless a function opts in with `**name: V`
- the captured values are still checked against `V` rather than falling back to an untyped "anything goes" bag

That makes `**kwargs` a good fit for intentional adapter boundaries without turning permissive extra-parameter capture into the default programming model.

### Mixed usage: `*args` + `**kwargs`

You can use both in one function. `*args` captures extra positional arguments; `**kwargs` captures extra named arguments.

```incan
def render(template: str, *values: str, **opts: str) -> str:
    return template  # placeholder
```

### Call-site unpacking

Use `*expr` at a call site to pass an existing list-like value into a positional rest parameter. Use `**expr` to pass an existing dict-like value into a keyword rest parameter.

```incan
def log(level: str, *msgs: str) -> None:
    for msg in msgs:
        println(f"[{level}] {msg}")

def main() -> None:
    parts = ["started", "listening"]
    log("info", *parts)
    log("info", "boot", *parts, "ready")
```

Keyword unpacking follows the same rule for functions that declare a keyword rest parameter:

```incan
def connect(host: str, **opts: str) -> None:
    ...

def main() -> None:
    defaults = {"tls": "true"}
    connect("localhost", **defaults, user="danny")
```

This RFC keeps unpacking static and rest-directed. The compiler must know that the callee has a matching rest parameter. A later RFC may explore fully dynamic Python-style unpacking into ordinary fixed parameters, where the runtime length or keys of the unpacked value determine which parameters are filled.

### Higher-order helper APIs

Variadics are especially useful when a library wants to accept any number of homogeneous packaged values:

```incan
from std.async import arm, race

pub async def fastest_text() -> str:
    return await race(
        arm(fetch_primary(), (value) => value),
        arm(fetch_replica(), (value) => value),
        arm(fetch_cache(), (value) => value),
    )
```

The repeated thing here is not "awaitable, callback, awaitable, callback". The repeated thing is `RaceArm[str]`. That is the kind of API variadics make elegant.

### Calls through variables

Rest metadata is part of a callable's type-level contract. A function value created from a rest-aware function keeps the same rest-call behavior as a direct call:

```incan
def log(level: str, *msgs: str) -> None:
    ...

def main() -> None:
    f = log
    f("info", "a", "b")
    f("info", *["a", "b"])
```

If a callable type is written without rest markers, the compiler treats it as an exact fixed-arity function type. Rest-call sugar is therefore available through function values only when the callable type preserves the rest structure.

## Reference-level explanation

### Definitions

This RFC introduces two new parameter kinds:

1. **Rest positional parameter**: `*name: T`: Binds `name` as `List[T]` within the function body
2. **Rest keyword parameter**: `**name: V`: Binds `name` as `Dict[str, V]` within the function body
3. **Positional unpack argument**: `*expr`: Supplies elements of a list-like value to the callee's rest positional parameter
4. **Keyword unpack argument**: `**expr`: Supplies entries of a dict-like value to the callee's rest keyword parameter

In both cases, the annotation specifies the element type (`T`) or value type (`V`), not the container type.

### Placement rules

Within a single parameter list:

- at most one `*name: T` parameter is allowed
- at most one `**name: V` parameter is allowed
- if present, `*name: T` must appear after all normal parameters
- if present, `**name: V` must be the last parameter
- if both are present, the order must be: normal params..., `*args`, `**kwargs`

Violations are compile-time errors.

### Call binding algorithm

Given:

```incan
def f(p1: A, p2: B, *rest: R, **kw: K) -> T: ...
```

Binding a call `f(<positional...>, <named...>)` proceeds as:

1. Bind normal parameters from positional arguments left-to-right until normal parameters are exhausted or positional arguments run out.
2. Bind normal parameters from named arguments by exact name match for any normal params not yet bound.
3. If a named argument targets a parameter already bound, emit an error.
4. Remaining positional arguments:
    - if `*rest` exists: append them to `rest` in order
    - otherwise: error for too many positional arguments
5. Positional unpack arguments:
    - if `*rest` exists and the unpacked expression is compatible with `List[R]`: extend `rest` in order
    - otherwise: emit a compile-time error
6. Remaining named arguments that do not match any normal parameter:
    - if `**kw` exists: insert them into `kw`
    - otherwise: error for unknown named argument
7. Keyword unpack arguments:
    - if `**kw` exists and the unpacked expression is compatible with `Dict[str, K]`: extend `kw`
    - otherwise: emit a compile-time error

### Type checking rules

- each extra positional argument bound into `*rest: R` must be type-compatible with `R`
- each extra named argument value bound into `**kw: K` must be type-compatible with `K`
- each `*expr` unpack argument must be type-compatible with `List[R]` for the resolved rest positional parameter
- each `**expr` unpack argument must be type-compatible with `Dict[str, K]` for the resolved rest keyword parameter
- `rest` is typechecked as `List[R]` within the function
- `kw` is typechecked as `Dict[str, K]` within the function
- callable values must preserve rest structure in their function type when they originate from a rest-aware declaration

### Lowering and runtime behavior

This feature is specified as pure compile-time lowering:

- functions defined with `*rest` and/or `**kw` are implemented as normal functions whose trailing parameters are explicit `List[...]` / `Dict[...]` values
- calls that use rest-capture sugar are rewritten by the compiler to construct those values at the call site
- unpack arguments are lowered into the same explicit list/dict construction path rather than into runtime reflection or dynamic dispatch

Conceptually:

```incan
log("info", "a", "b", "c")
```

lowers to:

```incan
log("info", ["a", "b", "c"])
```

and:

```incan
connect("localhost", 5432, tls="true", user="danny")
```

lowers to:

```incan
connect("localhost", 5432, {"tls": "true", "user": "danny"})
```

The backend can then emit standard Rust `Vec<T>` / `HashMap<String, V>` construction without needing true Rust variadics.

Mixed direct and unpacked rest arguments are flattened into the explicit container in source order:

```incan
log("info", "a", *more, "z")
```

lowers conceptually to a call whose final rest list is equivalent to:

```incan
["a"] + more + ["z"]
```

The exact generated Rust does not need to use an Incan `+` operation; it only needs to preserve the observable list order.

### Interaction with function values

RFC 035 makes named functions first-class values. RFC 038 extends callable metadata so a function value can preserve whether a parameter is ordinary, positional rest, or keyword rest.

A plain fixed-arity function type such as `(str, List[str]) -> None` does not imply rest-call behavior by itself. Function values that come from rest-aware declarations, and callable types that explicitly preserve rest markers, do imply rest-call behavior.

### Interaction with existing features

- **async/await**: no special interaction; captured list/dict values are ordinary values
- **traits/derives**: methods may also use `*` / `**` under the same rules
- **imports/modules**: no special interaction
- **Rust interop**:
    - the sugar should not be applied to external Rust calls unless the compiler has an Incan-level signature describing the trailing parameters as `List[...]` / `Dict[...]`
    - C-variadic interop is out of scope

## Design details

### Syntax

Add to function parameter grammar:

```text
param ::= IDENT ":" Type
        | "*" IDENT ":" Type
        | "**" IDENT ":" Type

call_arg ::= Expr
           | IDENT "=" Expr
           | "*" Expr
           | "**" Expr
```

Call-site unpacking is part of this RFC, but only for statically rest-aware callees.

### Semantics

Key invariants:

- the `*` / `**` marker determines how arguments are captured
- the annotation specifies element/value types, not container types
- binding is deterministic, compile-time, and independent of runtime reflection
- callable metadata preserves rest markers so function values can keep rest-aware call behavior
- unpacking is accepted only when it feeds a resolved rest parameter

### Compatibility and migration

This is intended to be non-breaking:

- `*` and `**` are new forms in parameter position
- existing valid programs should remain valid
- if the compiler tightens named-argument checking for ordinary calls to support a coherent rest-capture model, that change should be introduced carefully with good diagnostics

## Alternatives considered

### 1. Require explicit container types in annotations

Example:

```incan
def log(level: str, *msgs: List[str]) -> None:
    ...
```

Rejected because the `*` / `**` markers already imply the container kind. Requiring `List[...]` / `Dict[...]` as well is redundant and noisier than necessary.

### 2. Overload ordinary trailing `List[T]` / `Dict[str, V]` parameters with call-site magic

Example:

```incan
def log(level: str, msgs: List[str]) -> None:
    ...
```

with special treatment of `log("info", "a", "b")`.

Rejected because it hides important semantics. A list parameter should look like a list parameter.

### 3. Fixed-arity helper ladders

Rejected as the long-term design for APIs that are conceptually variadic.

This is acceptable as a temporary implementation convenience in some libraries, but it should not substitute for a proper language feature.

### 4. Heterogeneous variadics

Rejected for this RFC.

The homogeneous model is clearer, easier to typecheck, and already sufficient for many important APIs once repeated inputs are packaged into a common type.

## Drawbacks

- adds syntax, typing, and diagnostics complexity
- increases the number of ways to express APIs, which can fragment style
- requires callable metadata to carry more than a flat list of parameter types
- may encourage over-flexible APIs if used without discipline

## Layers affected

- **Language surface** — `*` and `**` parameter forms in function signatures must be supported and remain distinct from ordinary parameters.
- **Type system** — rest-parameter metadata, callable rest structure, binding rules for extra positional or named arguments, unpacking rules, and element or value type mismatches must be validated.
- **Execution handoff** — implementations may rewrite extra positionals into `List[...]` values and extra named arguments into `Dict[str, ...]` values, but the observable call semantics must match this RFC.
- **Formatter** — `*` and `**` markers on rest parameters should print predictably.
- **LSP** — rest parameter variables should display as `List[T]` and `Dict[str, V]` on hover and in completions.

## Implementation Plan

### Phase 1: Syntax, AST, and formatter

- Parse rest parameters and unpack call arguments.
- Preserve rest parameter and unpack argument kinds in the AST.
- Format rest markers and unpack arguments predictably.

### Phase 2: Typechecker and callable metadata

- Preserve rest parameter kinds in function and method metadata.
- Validate rest parameter placement and duplicate rest forms.
- Bind extra positional/named arguments and unpack arguments against the resolved rest targets.
- Preserve rest-aware callable behavior through first-class function values.

### Phase 3: Lowering, IR, and emission

- Lower rest parameters to explicit trailing list/dict parameters.
- Lower rest-aware calls to explicit list/dict construction in source order.
- Emit Rust that preserves ordinary fixed-arity calls, rest capture calls, and unpacked rest calls.

### Phase 4: Tooling, tests, and docs

- Add parser, formatter, typechecker, codegen snapshot, and integration coverage.
- Update user-facing language docs and release notes.
- Draft a follow-up RFC for fully dynamic Python-style unpacking into ordinary fixed parameters.

## Progress Checklist

### Spec / design

- [x] Link RFC 038 to issue #322.
- [x] Settle function-value behavior: rest metadata is preserved for rest-aware callable values.
- [x] Include static call-site unpacking in RFC 038.
- [x] Keep dynamic unpacking into ordinary fixed parameters for a follow-up RFC.

### Parser / AST / formatter

- [x] Parse `*name: T` and `**name: V` parameters.
- [x] Parse `*expr` and `**expr` call arguments.
- [x] Preserve rest and unpack kinds in AST nodes.
- [x] Format rest parameters and unpack arguments stably.

### Typechecker

- [x] Type rest parameters as `List[T]` and `Dict[str, V]` inside declarations.
- [x] Validate rest parameter placement and duplicates.
- [x] Validate extra direct positional/named arguments against rest element/value types.
- [x] Validate `*expr` and `**expr` against rest container types.
- [x] Preserve rest metadata through first-class function values.

### Lowering / IR / emission

- [x] Lower rest declarations to explicit trailing container parameters.
- [x] Lower rest-aware direct calls to explicit container arguments.
- [x] Lower unpack call arguments into the same explicit container construction path.
- [x] Emit correct Rust for direct rest calls, function-value rest calls, and unpacked rest calls.

### Tests

- [x] Parser tests for rest parameters and unpack arguments.
- [x] Formatter round-trip tests for rest syntax.
- [x] Typechecker tests for valid rest calls, invalid placement, invalid element/value types, and invalid unpack usage.
- [x] Codegen snapshot tests for rest capture, callable-value rest calls, and unpacked rest calls.
- [x] Integration tests for compiled rest calls.

### Docs

- [x] Update authored language reference/user docs.
- [x] Add release notes entry.
- [x] Draft follow-up RFC for dynamic unpacking into ordinary fixed parameters.

## Design Decisions

- Rest-call sugar applies through function values when the callable metadata preserves rest parameter structure.
- RFC 038 adds call-site unpacking for statically rest-aware callees: `f(*xs)` feeds the callee's positional rest parameter and `f(**kw)` feeds the callee's keyword rest parameter.
- Fully dynamic Python-style unpacking into ordinary fixed parameters is not part of RFC 038 and should be specified in a follow-up RFC.
- Captured keyword arguments use `Dict[str, V]`; richer structured-options containers are speculative and out of scope.
