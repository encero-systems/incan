# RFC 035: First-Class Named Function References

- **Status:** Planned
- **Created:** 2026-03-06
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** 
    - [RFC 036] (User-defined decorators — depends on this RFC)
    - [RFC 005] (Rust interop — `@rust.extern` functions already passable via this mechanism)
- **Issue:** #180
- **RFC PR:** —
- **Written against:** v0.1
- **Shipped in:** —

## Summary

Incan closures (`(x) => expr`) are first-class values today. Named functions defined with `def` are not — a function name in a non-call position is currently not a valid expression. This RFC makes named functions passable as values, matching Python's behaviour: `sorted(items, key=my_func)` works without wrapping in a closure.

## Motivation

### The workaround is unnecessary ceremony

In Python, functions are values. You pass them by name, store them in variables, and put them in lists without any wrapper:

```python
def double(x):
    return x * 2

result = list(map(double, items))   # direct reference
```

In Incan today, you must wrap in a closure even though the closure adds nothing:

```incan
def double(x: int) -> int:
    return x * 2

result = items.map((x) => double(x))   # unnecessary indirection
```

This is pure ceremony. The closure carries no additional logic — it only exists because `double` cannot appear in value position directly.

### It is a prerequisite for user-defined decorators

RFC 036 (User-defined decorators) desugars `@D def f(): ...` into `f = D(f)`. For this to work, `f` must be passable as a value to `D`. Without named function references, decorator desugaring cannot be expressed.

### It is already half-implemented

The typechecker already resolves a named function identifier to `ResolvedType::Function(params, ret)` with `IdentKind::Value` — meaning the type system already understands that a function name is a value. The gap is in the lowering and emission stages, which do not yet handle a function-typed identifier in a non-call position.

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

Both spellings are the same type. The typechecker normalises `Callable[Params, R]` to the arrow form during type resolution — they are never distinct in the symbol table or IR.

Type parameters are not yet supported on function references (a generic function reference `foo[T]` requires a separate RFC). In this RFC, all referenced functions must be monomorphic at the reference site.

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

The function type `(int, str) -> bool` is already in the parser and typechecker. For struct fields and `const` bindings, the parser must accept this form in type annotation position — which it does today via `Type::Function`.

### `Callable[Params, R]` sugar

`Callable[Params, R]` always has exactly **two** type arguments. The first argument describes the parameters; the second is the return type:

| Sugar                    | Arrow form       |
| ------------------------ | ---------------- |
| `Callable[(), R]`        | `() -> R`        |
| `Callable[A, R]`         | `(A) -> R`       |
| `Callable[(A, B), R]`    | `(A, B) -> R`    |
| `Callable[(A, B, C), R]` | `(A, B, C) -> R` |

The parenthesized tuple form `(A, B)` is required when there are zero or two-or-more parameters; a bare type `A` is shorthand for a single-parameter callable. Passing a non-tuple, non-type first argument is a parse error.

The parser performs the desugaring immediately — `Callable[...]` never appears in the AST. The typechecker, lowerer, emitter, formatter, and LSP require no changes beyond what the arrow form already requires. `Callable` is registered as a known generic type alias in the type resolver, not as a separate AST node type.

### Interaction with existing features

**Closures**: No change. `IrExprKind::Closure` remains the IR node for anonymous functions. Named references emit as `IrExprKind::Ident`.

**`@rust.extern` functions**: Extern functions are valid function references. Passing one as a value generates the same Rust identifier expression.

**Async functions**: An `async def` function referenced as a value has type `(params) -> Future[R]` from Rust's perspective. In Incan's type system, async functions have the same signature as sync functions — the `async` modifier is part of the calling convention, not the type. Passing an async function as a value works at the Rust level because Rust treats async functions as returning `impl Future`. The Incan type system does not need to represent this differently for Phase 1.

**Decorators (RFC 036)**: The decorator desugaring `f = D(f)` requires `f` to be a valid expression in the `D(f)` call. This RFC makes that possible.

### Compatibility / migration

Fully additive. Code that currently works is unaffected. Code that previously required a closure wrapper can now use a direct reference — but the wrapper form remains valid.

## Alternatives considered

**Require explicit reference syntax (`&double` or `func(double)`)**: Rejected. Python doesn't require this and Incan aims for Python ergonomics. The bare name is unambiguous: `double` is a reference; `double(x)` is a call.

**Use a `Callable` protocol instead of function types**: Rejected as a replacement — `(params) -> ret` is the canonical form. However, `Callable[Params, R]` is accepted as syntactic sugar that desugars to the arrow form during parsing. `Callable` always takes exactly two type arguments (params type or tuple, plus return type), which keeps it unambiguous and composable without introducing a separate type system concept.

## Drawbacks

Minimal. The change is small and well-scoped. The main implementation risk is in call site detection: the lowerer must correctly distinguish `f(args)` (call) from `f` (reference) in all contexts. This is a syntactic distinction so the parser handles it naturally.

## Layers affected

- **Parser** (`crates/incan_syntax/`) — desugar `Callable[T1, ..., R]` to `(T1, ...) -> R` during type parsing; `Callable` never reaches the AST as a distinct node.
- **IR Lowering** (`src/backend/ir/lower/`) — when lowering `Expr::Ident` where the resolved type is `ResolvedType::Function(...)`, emit `IrExprKind::Ident(name)` with the corresponding `IrType::Function`. Currently this path may fall through to an error or produce an incorrect IR node.
- **Typechecker** (`src/frontend/typechecker/`) — already returns `IdentKind::Value` for function symbols; verify that a function identifier in argument position type-checks correctly against a `(params) -> ret` parameter type.
- **IR Emission** (`src/backend/ir/emit/`) — `IrExprKind::Ident` with `IrType::Function` must emit as a plain Rust identifier; verify no special-casing incorrectly wraps or calls it.

## Design Decisions

1. Generic function references
    **Question:** `map(my_generic_func, items)` — when `my_generic_func` is generic, which monomorphisation is chosen?
    **Decision:** **Deferred to a future RFC.** Generic function references require type inference improvements and are out of scope for Phase 1. In Phase 1, only monomorphic (non-generic) functions can be referenced as values.
    **Rationale:** This is a complex problem that requires careful design around type inference and monomorphisation. It's better to ship Phase 1 with this limitation and address generics in a follow-up RFC.

2. Async function references
    **Question:** Should `async def foo()` referenced as a value have type `() -> T` or `() -> Future[T]` in Incan's surface type system?
    **Decision:** For Phase 1, treat async function references as having type `() -> T` (the `async` modifier is an implementation detail, not a surface type difference).
    **Rationale:** This keeps the type system simple and matches Python's behavior where async functions are callable values. The async semantics are handled at the call site, not in the function reference type. This can be revisited when async traits or async function pointers are needed.

3. Trait bounds on `Callable` type parameters
    **Question:** How should trait bounds inside `Callable` type parameters be handled?
    **Decision:** Two distinct cases with different complexity:
        - **Bound on the outer function's type parameter (works today):** When `T` is declared on the enclosing function with `with Loggable`, using it in `Callable[T, str]` works naturally — `T` is already a known, bounded type variable. This is the idiomatic pattern and requires no new design.
        ```incan
        def apply_all[T with Loggable](items: List[T], f: Callable[T, str]) -> List[str]:
            return items.map(f)
        ```
        - **Inline bound inside `Callable` itself (`Callable[T with Loggable, str]`):** Here `T` is not declared elsewhere — the `Callable` type would be introducing a universally quantified bound type variable inline. This is equivalent to Rust's higher-rank trait bounds (`for<T: Loggable> fn(T) -> str`). It raises unresolved questions about quantification (universal vs existential), call-site type inference, and monomorphisation. This is **explicitly deferred** and should be addressed in a separate RFC on higher-rank polymorphism or generic callable types.

    **Rationale:** The first case is already supported and is the idiomatic pattern. The second case is significantly more complex and should be addressed in a separate RFC focused on higher-rank trait bounds.

## Implementation Plan

### Phase 1: Parser + Type Annotation Support

- [ ] Verify `Callable[Params, R]` syntax is parsed and desugared to arrow form `(Params) -> R`
- [ ] Ensure function type annotations `(params) -> ret` are accepted in all type annotation positions
- [ ] Add parser tests for `Callable` syntax and arrow form in various contexts

### Phase 2: Typechecker Validation

- [ ] Verify function identifiers in value position resolve to `ResolvedType::Function`
- [ ] Ensure function identifiers in argument position type-check against `(params) -> ret` parameters
- [ ] Add typechecker tests for function references in various contexts (arguments, assignments, collections)
- [ ] Verify async function references have correct type (surface type, not Future)

### Phase 3: IR Lowering

- [ ] Modify lowering to handle `Expr::Ident` where resolved type is `ResolvedType::Function`
- [ ] Emit `IrExprKind::Ident(name)` with `IrType::Function { params, ret }` for function references
- [ ] Ensure function references are distinguished from function calls (`f` vs `f(args)`)
- [ ] Add lowering tests for function references

### Phase 4: Code Generation

- [ ] Verify `IrExprKind::Ident` with `IrType::Function` emits as plain Rust identifier
- [ ] Ensure no special-casing incorrectly wraps or calls the identifier
- [ ] Verify generated Rust code compiles and runs correctly
- [ ] Add codegen snapshot tests for function references

### Phase 5: Integration Tests + Examples

- [ ] Add integration tests demonstrating function references in practice
- [ ] Create example project showing function references with `map`, `apply`, etc.
- [ ] Test async function references
- [ ] Test function references in collections and assignments
- [ ] Verify no regressions in existing code

### Phase 6: Documentation

- [ ] Update language docs to document function references
- [ ] Add examples to docs-site
- [ ] Update release notes for v0.2
- [ ] Document `Callable` type sugar syntax

## Progress Checklist

### Parser / AST

- [ ] `Callable[Params, R]` syntax parses and desugars to arrow form
- [ ] Function type annotations accepted in all positions
- [ ] Parser tests for function types

### Typechecker

- [ ] Function identifiers resolve to `ResolvedType::Function`
- [ ] Type-checking against function parameters works
- [ ] Async function references have correct surface type
- [ ] Typechecker tests for function references

### Lowering / IR

- [ ] Function references emit `IrExprKind::Ident` with `IrType::Function`
- [ ] Distinguish `f` (reference) from `f(args)` (call)
- [ ] Lowering tests for function references

### Emission

- [ ] Function references emit as plain Rust identifiers
- [ ] No incorrect wrapping or calling
- [ ] Codegen snapshot tests
- [ ] Integration tests pass

### Stdlib / Runtime

- [ ] No stdlib changes required (pure language feature)
- [ ] Async function references work correctly

### Tests

- [ ] Parser unit tests for `Callable` syntax
- [ ] Parser unit tests for arrow form
- [ ] Typechecker unit tests: function references as arguments
- [ ] Typechecker unit tests: function references in assignments
- [ ] Typechecker unit tests: function references in collections
- [ ] Typechecker unit tests: async function references
- [ ] Typechecker unit tests: error cases (generic functions)
- [ ] Codegen snapshot tests for function references
- [ ] Integration tests: function references in practice
- [ ] Integration tests: async function references
- [ ] Integration tests: no regressions

### Docs

- [ ] Update language docs
- [ ] Add examples to docs-site
- [ ] Release notes entry for v0.2
