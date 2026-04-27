# RFC 038: Variadic Args and Unpacking (`*args` / `**kwargs`)

- **Status:** Implemented
- **Created:** 2026-03-07
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 035 (First-class named function references)
    - RFC 039 (`race` for awaitable concurrency)
- **Issue:** [#83](https://github.com/dannys-code-corner/incan/issues/83)
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** 0.3

## Summary

Add Python-style rest parameters and unpacking to Incan as one static, type-directed binding model: functions may capture extra positional and keyword arguments with `*args` and `**kwargs`, calls may unpack positional and keyword values with `f(*xs)` and `f(**kw)`, and collection literals may spread existing lists and dictionaries with `[*xs]` and `{**kw}` while lowering to explicit typed containers rather than runtime reflection.

## Core model

- `*name: T` captures extra positional arguments and binds `name` as a `List[T]`
- `**name: V` captures extra named arguments and binds `name` as a `Dict[str, V]`
- `f(*xs)` expands an ordinary list or statically shaped positional value into the call
- `f(**kw)` expands an ordinary dictionary or statically shaped keyword value into the call
- `[*xs]` expands ordinary lists or statically shaped positional values into a list literal
- `{**kw}` expands ordinary dictionaries or statically shaped keyword values into a dict literal

This is compile-time sugar. The surface syntax is ergonomic, but the lowered model stays explicit:

- a rest positional parameter lowers to an ordinary trailing `List[T]` parameter
- a rest keyword parameter lowers to an ordinary trailing `Dict[str, V]` parameter
- rest-aware call sites are rewritten to construct those containers explicitly
- fixed-parameter unpacking is rewritten to an ordinary ordered call after the compiler proves the unpacked shape
- collection-literal spread is rewritten to explicit list or dictionary construction in source order
- call-site unpacking is accepted only when the callee type/signature and unpacked value shape provide a deterministic compile-time binding plan

Collection-literal spread applies to ordinary runtime list and dictionary literals. Const frozen collection initializers remain direct-entry-only until const emission can represent spread entries without changing frozen-storage semantics.

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

### Call-site unpacking is part of the same feature

Python users do not experience `*args`, `**kwargs`, `f(*xs)`, `f(**kw)`, `[*xs]`, and `{**kw}` as unrelated features. They are one unpacking model with three related contexts:

- function definitions may capture extra positional or named arguments
- function calls may unpack existing positional or named values
- list and dictionary literals may build new containers by spreading existing containers

Incan should keep that conceptual model while making each destination explicit. The implementation may still phase the work, but the RFC should define the full unpacking surface instead of splitting fixed-parameter or collection-literal unpacking into separate follow-up RFCs.

## Goals

- Add `*name: T` and `**name: V` parameter forms.
- Add `f(*xs)` and `f(**kw)` call-site unpacking for calls to rest-aware callables.
- Add `f(*xs)` and `f(**kw)` call-site unpacking for ordinary fixed-parameter callables when the compiler can prove the unpacked shape.
- Add list literal spread with `[*xs]` and mixed forms such as `[head, *tail, last]`.
- Add dict literal spread with `{**kw}` and mixed forms such as `{"accept": "json", **headers}`.
- Specify them as compile-time sugar over explicit trailing container parameters.
- Preserve rest-parameter structure in callable metadata so named function values keep the same rest-call behavior as direct calls.
- Keep the semantics deterministic and type-directed.
- Preserve Python-like ergonomics without introducing runtime reflection.
- Enable cleaner library APIs that naturally take "zero or more packaged values".

## Non-Goals

- Dynamic runtime arity dispatch for arbitrary `List[T]` / `Dict[str, T]` values when the compiler cannot prove the needed length or keys.
- C-style variadics or raw FFI variadics.
- Heterogeneous positional capture without packaging.
- An `Any` type for untyped keyword captures.
- A richer structured-options container for captured keyword arguments beyond `Dict[str, V]`.
- Set literal spread. This RFC covers list and dictionary literal spread because they correspond directly to positional and keyword unpacking.
- Treating `[**xs]` as valid syntax. `**` is keyword or mapping unpacking, not sequence expansion.

### Why fixed-parameter unpacking belongs here

It is reasonable to ask whether `f(*pair)` for `def f(a: int, b: int)` belongs with rest parameters. It does. Python presents rest capture and call-site unpacking through the same `*` / `**` spelling because both are parts of call binding.

The static typing problems are different, so the rules must be explicit. Rest-directed unpacking has a known destination container. If a function declares `*items: T`, then `f(*xs)` only needs to prove that `xs` is compatible with `List[T]`. If a function declares `**labels: T`, then `f(**kw)` only needs to prove that `kw` is compatible with `Dict[str, T]`.

Fixed-parameter unpacking has no such destination container. The compiler must prove the unpacked value's arity, parameter order, named-key set, duplicate bindings, defaults, and per-field types before it can lower the call. Lists do not normally carry length in their type, and ordinary dictionaries do not normally carry a statically known key set. That means fixed-parameter unpacking needs stricter shape-proof rules, not a separate RFC.

Collection-literal spread follows the same destination rule. `[*xs]` is list spread because `*` expands positional values into a sequence destination. `{**kw}` is dictionary spread because `**` expands mapping entries into a mapping destination. `[**xs]` must remain invalid because `**` has no coherent list destination.

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

This is especially valuable for boundary-style APIs that intentionally forward option bags to another system: readers, writers, HTTP clients, framework adapters, and plugin hooks. Python libraries commonly separate declared parameters from pass-through extra options for this kind of adapter boundary.

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

Use `*expr` at a call site to pass an existing ordinary list value into a positional rest parameter. Use `**expr` to pass an existing ordinary dictionary value into a keyword rest parameter.

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

Call-site unpacking also applies to fixed parameters when the compiler can prove the unpacked shape:

```incan
def point(x: int, y: int) -> str:
    return f"{x},{y}"

def main() -> str:
    xy: tuple[int, int] = (3, 4)
    return point(*xy)
```

For keyword unpacking into fixed parameters, the compiler must know the available keys and their value types. Inline dictionary literals with string literal keys are the minimum accepted shape:

```incan
def route(path: str, method: str) -> str:
    return f"{method} {path}"

def main() -> str:
    return route(**{"path": "/status", "method": "GET"})
```

Ordinary `Dict[str, T]` values are still valid for feeding `**kwargs`; they are not enough by themselves to prove that fixed parameters such as `path` and `method` are present unless the compiler can preserve a statically known key shape for that value.

### List and dictionary literal spread

Use `*expr` inside a list literal when the destination is a new list:

```incan
def main() -> int:
    middle = [2, 3]
    values = [1, *middle, 4]
    return len(values)
```

The result preserves source order. Direct elements are inserted where they appear, and spread values contribute their elements at that position. `[*items]` therefore creates a shallow list copy, and `[head, *tail]` is the literal-spread form of prefixing an existing list.

Use `**expr` inside a dictionary literal when the destination is a new dictionary:

```incan
def main() -> int:
    defaults = {"accept": "json"}
    headers = {**defaults, "trace": "enabled"}
    return len(headers)
```

Dictionary spread preserves source order for insertion. If the same key appears more than once, the later entry wins, matching ordinary dictionary construction and `**kwargs` capture behavior.

The markers are destination-specific. `[*items]` is valid because a list is a sequence destination. `{**headers}` is valid because a dictionary is a mapping destination. `[**items]` is invalid even if `items` is a list, because `**` expands mappings into keyword or dictionary destinations, not sequences.

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

This RFC introduces two new parameter kinds and four unpacking forms:

1. **Rest positional parameter**: `*name: T`: Binds `name` as `List[T]` within the function body
2. **Rest keyword parameter**: `**name: V`: Binds `name` as `Dict[str, V]` within the function body
3. **Positional unpack argument**: `*expr`: Supplies elements of an ordinary list or statically shaped positional value to the call
4. **Keyword unpack argument**: `**expr`: Supplies entries of an ordinary dictionary or statically shaped keyword value to the call
5. **List spread element**: `*expr`: Supplies elements of an ordinary list or statically shaped positional value to a list literal
6. **Dictionary spread entry**: `**expr`: Supplies entries of an ordinary dictionary or statically shaped keyword value to a dictionary literal

For rest parameters, the annotation specifies the element type (`T`) or value type (`V`), not the container type.

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

Binding a call `f(<args...>)` proceeds by building one deterministic binding plan across ordinary positional arguments, ordinary named arguments, positional unpack arguments, and keyword unpack arguments.

For positional arguments:

1. Positional call items are processed left to right.
2. Direct positional arguments bind ordinary positional parameters left-to-right until those parameters are exhausted.
3. Surplus direct positional arguments are appended to `*rest` if present; otherwise they are errors.
4. `*expr` may bind ordinary fixed positional parameters only when `expr` has a statically known ordered shape. The minimum accepted shapes are fixed-length tuples and list literals.
5. A shaped `*expr` is expanded element by element. Elements bind remaining fixed positional parameters first, then extend `*rest` if present.
6. Homogeneous `List[T]` values with no known length are valid for extending `*rest` only when no fixed positional parameter still needs to be bound. They must not be used to fill a fixed number of ordinary parameters unless the language gains a separate way to prove list length statically.

For named arguments:

1. Direct named arguments bind ordinary parameters by exact name.
2. A direct named argument that targets an already bound parameter is a duplicate-argument error.
3. Unknown direct named arguments are inserted into `**kw` if present; otherwise they are errors.
4. `**expr` may bind ordinary fixed named parameters only when `expr` has a statically known key set and value types.
5. If a keyword rest parameter exists, keys that are not consumed by fixed parameters may extend that keyword rest dictionary when their values are compatible with `K`.
6. Ordinary `Dict[str, T]` is valid for extending `**kw`. It is not valid for proving that required fixed parameters are present unless a future explicit runtime-checking rule is accepted.

If an unpacked value would bind a parameter that is already bound, the compiler reports a duplicate-argument error. If a required fixed parameter remains unbound after all direct and unpacked arguments are processed, the compiler reports a missing-argument error.

### Collection literal binding

List literal binding must evaluate direct elements and spread elements from left to right:

1. A direct element contributes one value to the resulting list.
2. A `*expr` element contributes zero or more values at its source position.
3. Every contributed value must be compatible with the list element type.
4. `**expr` must not be accepted in a list literal.

Dictionary literal binding must evaluate direct entries and spread entries from left to right:

1. A direct key-value entry inserts or overwrites one key in the resulting dictionary.
2. A `**expr` entry contributes zero or more key-value pairs at its source position.
3. Every contributed key must be compatible with the dictionary key type.
4. Every contributed value must be compatible with the dictionary value type.
5. Duplicate keys are resolved by source order: later entries overwrite earlier entries.
6. `*expr` must not be accepted in a dictionary literal.

Const frozen collection initializers are not part of collection spread in this RFC implementation. They continue to require direct entries so const evaluation and backend frozen-storage emission stay aligned.

### Type checking rules

- each extra positional argument bound into `*rest: R` must be type-compatible with `R`
- each extra named argument value bound into `**kw: K` must be type-compatible with `K`
- each `*expr` unpack argument must be type-compatible with `List[R]` for the resolved rest positional parameter
- each `**expr` unpack argument must be type-compatible with `Dict[str, K]` for the resolved rest keyword parameter
- each `*expr` that binds fixed parameters must have a statically known ordered shape with per-position types compatible with the target parameters
- each `**expr` that binds fixed parameters must have a statically known key set with per-key value types compatible with the target parameters
- each `*expr` inside a list literal must be compatible with the list element type, either as a homogeneous ordinary list value or as a statically shaped positional value whose elements are each compatible
- each `**expr` inside a dictionary literal must be compatible with the dictionary key and value types, either as a homogeneous `Dict[K, V]` or as a statically shaped mapping whose keys and values are each compatible
- `rest` is typechecked as `List[R]` within the function
- `kw` is typechecked as `Dict[str, K]` within the function
- callable values must preserve rest structure in their function type when they originate from a rest-aware declaration

### Lowering and runtime behavior

This feature is specified as pure compile-time lowering:

- functions defined with `*rest` and/or `**kw` are implemented as normal functions whose trailing parameters are explicit `List[...]` / `Dict[...]` values
- calls that use rest-capture sugar are rewritten by the compiler to construct those values at the call site
- unpack arguments that feed rest parameters are lowered into the same explicit list/dict construction path rather than into runtime reflection or dynamic dispatch
- unpack arguments that feed fixed parameters are lowered to ordinary positional Rust arguments after binding is resolved
- list literal spread is lowered to explicit list construction and extension in source order
- dictionary literal spread is lowered to explicit dictionary construction and insertion/extension in source order

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
- **collection literals**: list and dictionary literals gain spread elements that follow the same positional-vs-mapping marker distinction as call arguments
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

list_elem ::= Expr
            | "*" Expr

dict_entry ::= Expr ":" Expr
             | "**" Expr
```

Call-site unpacking is part of this RFC for both statically rest-aware callees and fixed-parameter callees whose unpacked value shapes are statically provable.

Collection-literal spread is part of this RFC for list and dictionary literals. `*expr` is valid only where the destination is sequence-like. `**expr` is valid only where the destination is mapping-like.

### Semantics

Key invariants:

- the `*` / `**` marker determines how arguments are captured
- the annotation specifies element/value types, not container types
- binding is deterministic, compile-time, and independent of runtime reflection
- callable metadata preserves rest markers so function values can keep rest-aware call behavior
- unpacking is accepted only when the compiler can resolve every destination it feeds
- fixed-parameter unpacking requires shape proof; rest-parameter unpacking requires container compatibility
- collection-literal spread preserves source order and does not introduce a new runtime container protocol

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

### 5. Keep collection-literal spread separate

Rejected because list and dictionary literal spread are the collection-building counterpart to call-site unpacking. Splitting them would leave users with `f(*xs)` but no direct way to build `[prefix, *xs]`, and with `f(**kw)` but no direct way to build `{"trace": "on", **kw}`. The implementation can phase these surfaces, but the RFC should define the full unpacking model.

### 6. Permit `[**xs]` as a list flattening shortcut

Rejected because it makes the marker carry different meanings in different contexts. `**` means mapping or keyword expansion. List destinations must use `*`, so `[**xs]` remains invalid even when `xs` is a list.

## Drawbacks

- adds syntax, typing, and diagnostics complexity
- increases the number of ways to express APIs, which can fragment style
- requires callable metadata to carry more than a flat list of parameter types
- requires list and dictionary literal lowering to handle mixed direct and spread elements
- may encourage over-flexible APIs if used without discipline

## Layers affected

- **Language surface** — `*` and `**` parameter forms, call arguments, and collection-literal spread elements must be parsed distinctly from ordinary expressions.
- **Type system** — rest-parameter metadata, callable rest structure, binding rules for extra positional or named arguments, fixed-parameter unpack shape proof, collection spread typing, and element or value type mismatches must be validated.
- **Execution handoff** — implementations may rewrite extra positionals into `List[...]` values, extra named arguments into `Dict[str, ...]` values, fixed-parameter unpacking into ordinary ordered calls, and collection spread into explicit list/dict construction, but the observable semantics must match this RFC.
- **Formatter** — `*` and `**` markers on rest parameters, call arguments, and collection literal spread entries should print predictably.
- **LSP** — rest parameter variables should display as `List[T]` and `Dict[str, V]` on hover and in completions, and diagnostics/completions should understand valid unpacking contexts.

## Implementation Plan

### Phase 1: Syntax, AST, and formatter

- Parse rest parameters and unpack call arguments.
- Parse list spread elements and dictionary spread entries.
- Preserve rest parameter and unpack argument kinds in the AST.
- Format rest markers, unpack arguments, and collection spread entries predictably.

### Phase 2: Typechecker and callable metadata

- Preserve rest parameter kinds in function and method metadata.
- Validate rest parameter placement and duplicate rest forms.
- Bind extra positional/named arguments and unpack arguments against the resolved rest targets.
- Bind fixed parameters from unpack arguments when the unpacked value shape is statically known.
- Typecheck list and dictionary literal spread in source order.
- Preserve rest-aware callable behavior through first-class function values.

### Phase 3: Lowering, IR, and emission

- Lower rest parameters to explicit trailing list/dict parameters.
- Lower rest-aware calls to explicit list/dict construction in source order.
- Lower fixed-parameter unpacking to ordinary ordered calls after binding is resolved.
- Lower list and dictionary literal spread to explicit container construction in source order.
- Emit Rust that preserves ordinary fixed-arity calls, rest capture calls, unpacked rest calls, fixed-parameter unpacking, and collection literal spread.

### Phase 4: Tooling, tests, and docs

- Add parser, formatter, typechecker, codegen snapshot, and integration coverage.
- Update user-facing language docs and release notes.
- Document the rest-directed subset and the full unpacking north star without splitting the design across RFCs.

## Implementation Log

### Spec / design

- [x] Link RFC 038 to issue #83.
- [x] Settle function-value behavior: rest metadata is preserved for rest-aware callable values.
- [x] Include static call-site unpacking in RFC 038.
- [x] Fold fixed-parameter call-site unpacking into RFC 038 instead of using a follow-up RFC.
- [x] Include list and dictionary literal spread in RFC 038.

### Parser / AST / formatter

- [x] Parse `*name: T` and `**name: V` parameters.
- [x] Parse `*expr` and `**expr` call arguments.
- [x] Preserve rest and unpack kinds in AST nodes.
- [x] Format rest parameters and unpack arguments stably.
- [x] Parse and preserve `*expr` list literal spread.
- [x] Parse and preserve `**expr` dictionary literal spread.
- [x] Reject `**expr` in list literals and `*expr` in dictionary literals.
- [x] Format collection literal spread stably.

### Typechecker

- [x] Type rest parameters as `List[T]` and `Dict[str, V]` inside declarations.
- [x] Validate rest parameter placement and duplicates.
- [x] Validate extra direct positional/named arguments against rest element/value types.
- [x] Validate `*expr` and `**expr` against rest container types.
- [x] Validate `*expr` against statically known ordered shapes for fixed positional parameters.
- [x] Validate `**expr` against statically known key shapes for fixed named parameters.
- [x] Diagnose duplicate and missing fixed-parameter bindings across direct and unpacked arguments.
- [x] Typecheck `*expr` list literal spread against the list element type.
- [x] Typecheck `**expr` dictionary literal spread against the dictionary key/value types.
- [x] Preserve rest metadata through first-class function values.

### Lowering / IR / emission

- [x] Lower rest declarations to explicit trailing container parameters.
- [x] Lower rest-aware direct calls to explicit container arguments.
- [x] Lower unpack call arguments into the same explicit container construction path.
- [x] Emit correct Rust for direct rest calls, function-value rest calls, and unpacked rest calls.
- [x] Lower fixed-parameter unpacking to ordinary ordered calls.
- [x] Lower list literal spread to explicit list construction and extension.
- [x] Lower dictionary literal spread to explicit dictionary construction and insertion/extension.

### Tests

- [x] Parser tests for rest parameters and unpack arguments.
- [x] Formatter round-trip tests for rest syntax.
- [x] Typechecker tests for valid rest calls, invalid placement, invalid element/value types, and invalid unpack usage.
- [x] Codegen snapshot tests for rest capture, callable-value rest calls, and unpacked rest calls.
- [x] Integration tests for compiled rest calls.
- [x] Typechecker and codegen tests for fixed-parameter unpacking once shape proof lands.
- [x] Parser, formatter, typechecker, codegen, and integration tests for list and dictionary literal spread.

### Docs

- [x] Update authored language reference/user docs.
- [x] Add release notes entry.
- [x] Keep the full unpacking design in RFC 038.

## Design Decisions

- Rest-call sugar applies through function values when the callable metadata preserves rest parameter structure.
- RFC 038 adds call-site unpacking for statically rest-aware callees: `f(*xs)` feeds the callee's positional rest parameter and `f(**kw)` feeds the callee's keyword rest parameter.
- RFC 038 also owns fixed-parameter call-site unpacking: `f(*xs)` and `f(**kw)` may bind ordinary parameters when the compiler can prove the unpacked shape.
- RFC 038 owns list and dictionary literal spread: `[*xs]` spreads sequence values into list literals, `{**kw}` spreads mapping values into dictionary literals, and `[**xs]` remains invalid.
- Captured keyword arguments use `Dict[str, V]`; richer structured-options containers are speculative and out of scope.
