# RFC 035: First-Class Named Function References

- **Status:** Implemented
- **Created:** 2026-03-06
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - [RFC 036] (User-defined decorators — depends on this RFC)
    - [RFC 005] (Rust interop — `@rust.extern` functions already passable via this mechanism)
- **Issue:** #169
- **RFC PR:** —
- **Target version:** 0.2
- **Shipped in:** v0.2

## Summary

Incan closures (`(x) => expr`) are first-class values. This RFC made named functions defined with `def` first-class as well: a function name in value position is a valid expression, matching Python’s ergonomics (`sorted(items, key=my_func)` without an unnecessary closure wrapper).

## Motivation

### The workaround was unnecessary ceremony

In Python, functions are values. You pass them by name, store them in variables, and put them in lists without any wrapper:

```python
def double(x):
    return x * 2

result = list(map(double, items))   # direct reference
```

Before this RFC, Incan required a closure even when it added no logic:

```incan
def double(x: int) -> int:
    return x * 2

result = items.map((x) => double(x))   # unnecessary indirection
```

Named function references remove that ceremony.

### It is a prerequisite for user-defined decorators

RFC 036 (User-defined decorators) desugars `@D def f(): ...` into `f = D(f)`. For this to work, `f` must be passable as a value to `D`. Named function references make that expression valid.

### The type system already wanted this shape

The typechecker already resolved a named function identifier to `ResolvedType::Function(params, ret)` with `IdentKind::Value`. The implementation work closed the pipeline: parser-time `Callable` desugaring, lowering, emission, and explicit rejection of out-of-scope generic function references.

## Guide-level explanation (how users think about it)

A function name can appear anywhere a value of its type is expected:

```incan
def double(x: int) -> int:
    return x * 2

def apply(f: Callable[int, int], x: int) -> int:
    return f(x)

# Pass by name — no closure wrapper needed
result = apply(double, 5)      # → 10

# Store in a variable
transform = double
result = transform(5)          # → 10

# Put in a list
ops = [double, (x) => x + 1]   # mix of named and anonymous
```

Function types can be written in two equivalent forms:

```incan
# Arrow form (canonical)
f: (int) -> int
g: (int, str) -> bool
h: () -> bool

# Callable sugar (desugars to arrow form)
f: Callable[int, int]          # single param — equivalent to (int) -> int
g: Callable[(int, str), bool]  # multiple params as tuple — equivalent to (int, str) -> bool
h: Callable[(), bool]          # no params — equivalent to () -> bool
```

`Callable[Params, R]` always takes **exactly two** type arguments: the first is either a single type (one parameter) or a parenthesized tuple of types (zero or multiple parameters), and the second is the return type. Both forms are interchangeable in every position where a function type is accepted.

When the enclosing function has a bounded type parameter, `Callable` composes with it naturally — `T` is already known and bounded at the function level:

```incan
def apply_all[T with Loggable](items: List[T], f: Callable[T, str]) -> List[str]:
    return items.map(f)
```

### Interaction with methods

Instance methods are not directly passable as unbound values in this RFC (that would require a `self`-binding mechanism). Static methods (decorated with `@staticmethod`) are passable because they have no receiver:

```incan
class MathUtils:
    @staticmethod
    def square(x: int) -> int:
        return x * x

transform = MathUtils.square     # static method — passable as value
result = transform(4)            # → 16
```

## Reference-level explanation (precise rules)

### Expression context

A bare identifier that resolves to a `def`-declared function is valid as an expression when it appears in a value position — i.e., any position where the expected type is not a call target:

- As a function argument: `apply(double, 5)`
- In an assignment: `transform = double`
- In a collection literal: `[double, triple]`
- As the right-hand side of a `const`: `const TRANSFORM: (int) -> int = double`
- As a return value: `return double`

A function name in call position (`double(5)`) continues to be a call expression, not a reference followed by a call. The distinction is syntactic: `f(args)` is always a call; `f` without following `(args)` is always a reference.

### Type

The type of a named function reference is the function's signature expressed as a function type:

```incan
def foo(x: int, y: str) -> bool   →   type of `foo` as value: (int, str) -> bool
                                                          or: Callable[(int, str), bool]
```

Both spellings are the same type. The parser desugars `Callable[Params, R]` to `Type::Function` immediately, so the AST and IR only see the arrow-shaped function type.

Type parameters are not supported on function references in this RFC (a generic function reference requires a separate design). Generic functions are rejected in value position with a dedicated typechecker diagnostic; passing by name remains available for monomorphic functions and for wrapping generic callables in closures at the call site.

### Closures and named references are interchangeable

A `(int) -> int` parameter accepts both:

```incan
apply(double, 5)            # named function reference
apply((x) => x * 2, 5)     # anonymous closure
```

Both lower to the same IR type (`IrType::Function { params: [IrType::Int], ret: IrType::Int }`).

### What is NOT covered

- **Generic function references**: `my_generic_func[T]` as a value — deferred.
- **Unbound method references**: `MyClass.instance_method` without a receiver — deferred.
- **Partial application**: `double` partially applied to one argument — not in scope.

## Design details

### Syntax

No new syntax. The change is in the compiler's treatment of an identifier that resolves to a function: it is now valid in value position.

### Lowering

When the lowerer encounters `Expr::Ident(name)` and the resolved type is `ResolvedType::Function(...)`, it emits `IrExprKind::Ident(name)` with `IrType::Function { params, ret }` — the same as any other value identifier. No special IR node is needed.

### Emission

In Rust, a named function in value position is a valid function pointer expression. `IrExprKind::Ident(name)` where `name` is a function emits as just the identifier: `double`. Rust's type system handles the coercion from function item type to `fn(i64) -> i64` at the call site.

### Type annotation syntax

The function type `(int, str) -> bool` is already in the parser and typechecker. For struct fields and `const` bindings, the parser accepts this form in type annotation position via `Type::Function`.

### `Callable[Params, R]` sugar

`Callable[Params, R]` always has exactly **two** type arguments. The first argument describes the parameters; the second is the return type:

| Sugar                    | Arrow form       |
| ------------------------ | ---------------- |
| `Callable[(), R]`        | `() -> R`        |
| `Callable[A, R]`         | `(A) -> R`       |
| `Callable[(A, B), R]`    | `(A, B) -> R`    |
| `Callable[(A, B, C), R]` | `(A, B, C) -> R` |

The parenthesized tuple form `(A, B)` is required when there are zero or two-or-more parameters; a bare type `A` is shorthand for a single-parameter callable. Passing a non-tuple, non-type first argument is a parse error.

The parser performs the desugaring immediately — `Callable[...]` never appears in the AST. Downstream stages (typechecker, lowerer, emitter, formatter, LSP) operate on `Type::Function` like any other function type annotation.

### Interaction with existing features

**Closures**: No change. `IrExprKind::Closure` remains the IR node for anonymous functions. Named references emit as `IrExprKind::Ident`.

**`@rust.extern` functions**: Extern functions are valid function references. Passing one as a value generates the same Rust identifier expression.

**Async functions**: An `async def` function referenced as a value has type `(params) -> Future[R]` from Rust's perspective. In Incan's type system, async functions have the same signature as sync functions — the `async` modifier is part of the calling convention, not the type. Passing an async function as a value works at the Rust level because Rust treats async functions as returning `impl Future`. The Incan type system does not need to represent this differently for Phase 1.

**Decorators (RFC 036)**: The decorator desugaring `f = D(f)` requires `f` to be a valid expression in the `D(f)` call. This RFC makes that possible.

### Compatibility / migration

Fully additive. Code that previously required a closure wrapper can use a direct reference; the wrapper form remains valid.

## Alternatives considered

**Require explicit reference syntax (`&double` or `func(double)`)**: Rejected. Python doesn't require this and Incan aims for Python ergonomics. The bare name is unambiguous: `double` is a reference; `double(x)` is a call.

**Use a `Callable` protocol instead of function types**: Rejected as a replacement — `(params) -> ret` is the canonical form. However, `Callable[Params, R]` is accepted as syntactic sugar that desugars to the arrow form during parsing. `Callable` always takes exactly two type arguments (params type or tuple, plus return type), which keeps it unambiguous and composable without introducing a separate type system concept.

## Drawbacks

Minimal. The change is small and well-scoped. The main implementation risk was call site detection: the lowerer must correctly distinguish `f(args)` (call) from `f` (reference) in all contexts. This is a syntactic distinction so the parser handles it naturally.

## Layers affected

- **Parser** (`crates/incan_syntax/`) — desugar `Callable[T1, ..., R]` to `(T1, ...) -> R` during type parsing; `Callable` never reaches the AST as a distinct node.
- **IR Lowering** (`src/backend/ir/lower/`) — when lowering `Expr::Ident` where the resolved type is `ResolvedType::Function(...)`, emit `IrExprKind::Ident(name)` with the corresponding `IrType::Function`.
- **Typechecker** (`src/frontend/typechecker/`) — function symbols resolve to `IdentKind::Value` and `ResolvedType::Function`; generic functions in value position emit `generic_function_reference` and do not typecheck as references.
- **IR Emission** (`src/backend/ir/emit/`) — `IrExprKind::Ident` with `IrType::Function` emits as a plain Rust identifier.

## Implementation Plan

### Phase 1: Parser + AST

- Desugar `Callable[Params, R]` to `Type::Function` during type expression parsing.
- Parser tests for desugaring and invalid `Callable` arity.

### Phase 2: Typechecker

- Value-position resolution for monomorphic `def` symbols.
- Explicit diagnostic for generic function references (out of scope for this RFC).

### Phase 3: Lowering + Emission

- Lower function-typed identifiers to `IrExprKind::Ident` with function IR type.
- Emit plain Rust identifiers for function values; codegen snapshot coverage.

### Phase 4: Examples + Docs

- Advanced example and docs-site updates (closures, callable reference, book chapter, release notes).

## Implementation log

### Spec / design

- [x] Deferred items recorded under **Design Decisions** (generic refs, async surface typing, inline `Callable` bounds).

### Parser / AST

- [x] Parser: desugar `Callable[...]` to `Type::Function`.
- [x] Parser unit tests: zero/one/multi-param sugar and arity error.

### Typechecker

- [x] Function identifier in value positions typechecks against `(params) -> ret`.
- [x] Error: `generic_function_reference` for generic `def` in value position.

### Lowering / IR

- [x] Lower `Expr::Ident` with `ResolvedType::Function` to `IrExprKind::Ident` with `IrType::Function`.

### Emission

- [x] Emit correct Rust for function-valued identifiers (fn item / pointer coercion).

### Tests

- [x] Codegen snapshot: `function_references.incn`.
- [x] Example: `examples/advanced/function_references`.

### Docs

- [x] Docs-site: closures, `callable.md`, book ch. 3, RFC 035 text, v0.2 release notes.

## Design Decisions

1. **Generic function references** (`map(my_generic_func, items)` when `my_generic_func` is generic): **Deferred** to a follow-up RFC on inference/monomorphisation. **Shipped behaviour (v0.2):** using a generic function name in value position is a type error with hint to wrap in a closure (e.g. `(x) => id(x)`).

2. **Async function references** — surface type for `async def foo()` used as a value: **Decision for Phase 1 / v0.2:** keep the Incan signature shape aligned with sync functions (`() -> T` at the Incan type level); treat async as calling-convention detail. **Follow-up:** revisit when async traits or async fn pointers need explicit `Future` in the surface type system.

3. **Trait bounds on `Callable` type parameters** — two cases:

   - **Bound on the outer function's type parameter:** **Supported.** When `T` is declared on the enclosing function with `with Loggable`, using it in `Callable[T, str]` is the idiomatic pattern:

     ```incan
     def apply_all[T with Loggable](items: List[T], f: Callable[T, str]) -> List[str]:
         return items.map(f)
     ```

   - **Inline bound inside `Callable` (`Callable[T with Loggable, str]`):** **Deferred** to a separate RFC on higher-rank polymorphism / generic callable types (quantification, inference, monomorphisation).

--8<-- "_snippets/rfcs_refs.md"
