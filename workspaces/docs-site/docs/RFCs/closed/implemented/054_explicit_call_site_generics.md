# RFC 054: Explicit call-site generic arguments for function and method calls

- **Status:** Implemented
- **Created:** 2026-04-11
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 005 (Rust interop)
    - RFC 028 (Overload-based dispatch)
- **Issue:** [#266](https://github.com/encero-systems/incan/issues/266)
- **RFC PR:** —
- **Written against:** v0.1
- **Shipped in:** v0.2

## Summary

This RFC adds explicit call-site generic arguments for function and method calls, enabling ergonomic syntax like `id[int](1)` and `session.read_csv[Order]("orders.csv")`. This capability is essential for typed session APIs that would otherwise require value markers to select row type parameters, which makes for a poor user experience that does not scale with wide schemas.

## Motivation

Typed session APIs may otherwise need value markers to select row type parameters (for example, passing a fake model instance just to choose `T`). This is poor DX and scales badly for wide schemas. Without explicit call-site generic arguments, APIs like `read_csv[T](...)` and `table[T](...)` cannot be expressed ergonomically in user code.

## Goals

- Enable explicit call-site generic arguments on function calls: `fn[T](...)`
- Enable explicit call-site generic arguments on method calls: `obj.method[T](...)`
- Provide clear diagnostics for generic arity mismatch
- Preserve explicit type arguments through lowering and emission (including Rust turbofish where needed)

## Non-Goals

- Dynamic/optional API entrypoints through overload-based dispatch (owned by RFC 028)
- Function-level default type parameters for call sites (e.g. `fn[T = ...](...)`)
- Broader generic syntax work beyond call sites (e.g., type aliases, let bindings)
- Implicit generic inference from assignment context as the primary mechanism
- Function overloading mechanics themselves (owned by RFC 028)

## Guide-level explanation

### Basic syntax

Explicit generic arguments are written in square brackets immediately after the function or method name, before the argument list:

```incan
def id[T](x: T) -> T:
    return x

x = id[int](1)
y = id[str]("hello")
```

### Method calls

The same syntax applies to method calls:

```incan
orders = session.read_csv[Order]("orders", "orders.csv")
lazy = session.table[Order]("orders")
```

### Multiple type parameters

For functions with multiple type parameters, list them in order:

```incan
def pair[A, B](a: A, b: B) -> (A, B):
    return (a, b)

p = pair[int, str](1, "hello")
```

### Single type parameter callers

For single-parameter generics, both fully inferred and fully explicit call sites are valid:

```incan
def map[A](xs: List[A], f: Fn[A, int]) -> List[int]:
    ...

result0 = map(xs, f)       # inferred A
result1 = map[int](xs, f)  # explicit A
```

### Multiple type parameter callers

For multi-parameter generics, calls are either fully inferred (no type-arg list) or arity-complete when explicit brackets are used:

```incan
def pair_map[A, B](xs: List[A], f: Fn[A, B]) -> List[B]:
    ...

out0 = pair_map(xs, f)             # fully inferred
out1 = pair_map[int, str](xs, f)   # fully explicit
out2 = pair_map[int, _](xs, f)     # A explicit, B inferred via placeholder
```

### Partial inference with `_`

`_` is a call-site inference placeholder inside bracketed type arguments:

- `_` counts toward generic arity.
- `_` may be mixed with explicit type arguments.
- `_` means "infer this slot" while other slots remain explicitly pinned.

### Error messages

When generic arity doesn't match, provide clear diagnostics:

```text
error: generic arity mismatch
  |
3 | x = id[int, str](1)
  |       ^^^^^^^^^ expected 1 type argument(s), got 2
  |
  = note: `id` is defined with 1 type parameter(s)
```

### Optional/dynamic route (owned by RFC 028)

This RFC defines the static route (`fn[T](...)`, `obj.method[T](...)`).

Optional/dynamic call ergonomics (for example an API pair like `read_csv(...)` and `read_csv[T](...)` other than just relying on inference) are provided by overload-based API design in RFC 028, not by function-level generic defaults in RFC 054.

Use overloading only when the no-type-arg call intentionally has a different API contract (for example different return type/behavior), not to emulate partial generic binding.

## Reference-level explanation

### Syntax extension

The grammar for function and method calls is extended to support optional generic arguments:

```text
call_expr:
    | ident generic_args? call_args
    | primary_expr '.' ident generic_args? call_args
```

Where `generic_args` is:

```text
generic_args:
    | '[' type_args ']'
```

And `type_args` is a comma-separated list of `type | _`, with at least one entry:

```text
type_args:
    | type_or_hole (',' type_or_hole)*

type_or_hole:
    | type
    | '_'
```

### Type checking

During type checking, explicit type arguments are applied before type inference:

1. For each explicit type argument, substitute the corresponding type parameter
2. For each `_` placeholder, keep that type-parameter slot open for inference
3. Infer remaining type parameters from argument types and expected types
4. Report arity mismatch errors when:
   - bracketed type-argument count (including `_`) does not match declared type-parameter count
   - explicit type arguments are provided for nongeneric callees

### Lowering and emission

Explicit type arguments must be preserved through lowering to IR and emission to Rust:

- For Incan output, preserve the call-site syntax
- For Rust output, use turbofish syntax where applicable: `function_name::<Type>(...)`
- For methods, preserve the receiver and use turbofish: `receiver.method_name::<Type>(...)`
- Rust emission may require shape-specific placement (inherent method vs trait method vs UFCS form).
- Even when call-site brackets are omitted in Incan source, generated Rust may still need explicit type ascription/turbofish in backend helper code when Rust inference is insufficient.

### Interaction with existing features

#### Methods in all contexts

Explicit type arguments work with methods defined in any context:

```incan
# Trait methods
trait Session:
    fn read_csv[T](path: str, source: str) -> T

# Class methods
class Session:
    def read_csv[T](path: str, source: str) -> T:
        ...

# Enum methods (RFC 050)
enum Result[T, E]:
    Ok[T]
    Err[E]
    fn map[U](self, f: Fn[T, U]) -> Result[U, E]:
        ...

# Type alias methods
type MyInt = int
def parse[T](s: str) -> T:
    ...

# Model methods
model Order:
    id: int
    total: float
    fn discount[T](self, rate: float) -> T:
        ...
```

`fn` in trait declarations and `def` in class/model bodies are intentionally different syntactic forms in Incan.

All of these support explicit call-site generics:

```incan
session = Session()
orders = session.read_csv[Order]("orders.csv")
typed = session.read_csv[Order]("orders", "orders.csv")
```

#### Function overloading route (RFC 028 follow-on)

For optional/dynamic API entrypoints, a follow-on overloading route (tracked under RFC 028 expansion) can define multiple call surfaces with different signatures:

```incan
def read_csv[T](path: str, source: str) -> T:
    """Read CSV with explicit type parameter"""
    ...

def read_csv(path: str, source: str) -> Json:
    """Read CSV without type parameter, returns Json"""
    ...
```

This approach is not required for explicit call-site generic argument syntax itself, but it is the intended path for dynamic/optional API entrypoints.

#### Generic functions in stdlib

Standard library functions with multiple type parameters:

```incan
def zip[A, B](as: List[A], bs: List[B]) -> List[(A, B)]:
    ...

zipped = zip[int, str](nums, strs)
```

## Design details

### Syntax

The syntax `fn[T](args)` places generic arguments between the identifier and the argument list. This is consistent with Rust's turbofish syntax and TypeScript's call-site generics.

### Semantics

Explicit type arguments are applied before inference. This means:

- Explicit arguments constrain inference
- Inference fills in remaining parameters
- If all generic slots are explicit, no generic-slot inference is needed for the callee; ordinary argument/return-type checking still applies.

### Compatibility

This change is backward compatible:

- Existing code without explicit type arguments continues to work
- Type inference behavior is unchanged for calls without explicit arguments
- No breaking changes to existing APIs

### Diagnostics

Error messages should include:

- Expected arity (number of type parameters)
- Actual arity (number of explicit arguments)
- Which parameters were inferred vs explicit (when helpful)

## Alternatives considered

1. Keep marker-value APIs  
    **Rejected** because it hardcodes poor ergonomics into read APIs and does not scale with wide schemas.

2. Infer generic arguments from assignment context only  
    **Rejected** because it is insufficient as an immediate unblock. Explicit call-site selection is still needed for APIs where the type cannot be inferred from arguments.

3. Add untyped read APIs  
    **Rejected** because it weakens typed pipeline contracts and sacrifices type safety.

4. Use angle brackets instead of square brackets  
    **Rejected** because square brackets are less ambiguous in the context of Incan's existing syntax and avoid confusion with comparison operators.

5. Function overloading (RFC 028)  
    Not selected as part of this RFC's implementation surface. Overloading remains the route for optional/dynamic API entrypoints.

## Drawbacks

- Adds syntax complexity for users to learn
- Requires changes across multiple compiler stages (parser, typechecker, lowering, emission)
- May expose edge cases in generic handling that were previously hidden

## Future work (out of scope for this RFC)

- Qualified callee forms in examples and diagnostics (`mod.fn[T](...)`, `Type::assoc[T](...)`) where applicable.
- Additional guide examples for chained calls with explicit generics (`a.b().c[T](...)`).
- Any surface-area expansion beyond the explicit call-site generic argument mechanism defined here.

## Implementation architecture

Pipeline impact matches **Layers affected** below: optional bracketed type arguments on calls; arity and inference ordering as in **Reference-level explanation**; implementations must keep explicit type arguments available through lowering so emission can render Incan call syntax and Rust turbofish (including cases where Rust needs help beyond Incan source).

## Layers affected

- **Parser / AST**: new syntax for generic arguments in call expressions; AST node extension
- **Typechecker / Symbol resolution**: validation of explicit type arguments; arity checking
- **IR Lowering**: preservation of explicit type arguments through IR
- **Emission**: turbofish syntax for Rust; call-site syntax for Incan
- **Formatter**: support for new syntax with appropriate spacing
- **LSP / Tooling**: completion for generic arguments; hover showing type parameter info

## Implementation Plan

### Phase 1: Parser + AST

- Parse optional bracketed type arguments on direct calls and method calls (`callee[T](...)`, `recv.m[T](...)`), including `type | _` entries.

### Phase 2: Typechecker

- Apply explicit type arguments before inference; keep `_` slots open; arity and nongeneric-callee diagnostics per **Design Decisions**.

### Phase 3: Lowering + Emission

- Retain explicit type arguments through IR; emit Incan call syntax and Rust turbofish (inherent / trait / UFCS shapes as needed).

### Phase 4: Formatter, LSP, Tests, Docs

- Formatter round-trip for call-site brackets; LSP completion/hover where applicable; parser, typechecker, and codegen tests; user-facing docs as needed.

## Implementation log

### Spec / design

- [x] Design locked in **Design Decisions** (static route vs RFC 028 overload boundary).

### Parser / AST

- [x] Call and method-call grammar with `call_site_type_args`.
- [x] Parser tests for explicit type arguments on calls and method calls (including `_` placeholders).

### Typechecker

- [x] Explicit function and method type arguments specialize generic parameters; arity enforcement.
- [x] `_` placeholders in bracket lists participate in arity; partial explicit + inference; diagnostics when a slot stays unresolved; nonempty brackets rejected on unsupported call forms (builtins, Rust imports, indirect calls, etc.).

### Lowering / IR + Emission

- [x] Explicit type arguments preserved for codegen; Rust turbofish coverage in tests (including mixed explicit + `_` via typechecker monomorph snapshot).

### Formatter

- [x] Format `Call` / `MethodCall` with nonempty `type_args` brackets.

### LSP / Tooling

- [x] Completion / hover for call-site generic arguments (`src/lsp/call_site_type_args.rs`, wired from `src/lsp/backend.rs`; `[` completion trigger).

### Tests

- [x] Typechecker tests: `explicit_call_type_args_*`, `explicit_method_type_args_*`, `_` / unsupported-call coverage.
- [x] Codegen tests: explicit function and method call type args; mixed explicit + `_` turbofish.

### Docs

- [x] RFC status and implementation plan updated on the implementation branch.

## Design Decisions

1. RFC 054 defines the static route: explicit call-site generic arguments (`fn[T](...)`, `obj.method[T](...)`).
2. Optional/dynamic API routes are owned by RFC 028 overloading, not by generic defaults in RFC 054.
3. Explicit type-argument arity diagnostics must report:
   - declared parameter count
   - provided argument count (including `_` placeholders)
   - whether the callee is nongeneric
