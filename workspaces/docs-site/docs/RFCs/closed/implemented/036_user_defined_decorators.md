# RFC 036: user-defined decorators

- **Status:** Implemented
- **Created:** 2026-03-06
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 035 (First-class named function references — **prerequisite**)
    - RFC 005 (Rust interop — foundation for `@rust.extern`)
    - RFC 023 (Compilable stdlib — where `@route` and `@rust.extern` were first systematised)
    - RFC 024 (Extensible derive protocol — compiler built-in decorator counterpart)
    - RFC 026 (Superseded — see RFC 043 for Rust trait surface on wrappers)
    - RFC 027 (incan-vocab — library vocabulary registration, enables DSL decorators)
    - RFC 031 (Library system — enables decorator libraries to ship as `pub::` packages)
    - RFC 037 (Native web and HTTP stdlib redesign — consumer of `@app.get` / `@app.post`)
    - RFC 084 (RHS partial callable presets — future decorator factory ergonomics)
- **Issue:** [#170](https://github.com/dannys-code-corner/incan/issues/170), [#640](https://github.com/dannys-code-corner/incan/issues/640)
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** v0.3

## Summary

Incan's decorator system currently consists entirely of compiler built-ins such as `@derive`, `@staticmethod`, `@rust.extern`, and `@route`. Those forms are compiler-recognized annotations rather than ordinary user-extensible language abstractions. Users cannot define their own decorators.

This RFC introduces user-defined decorators: callable-shaped functions and objects that accept a function and return a value. The compiler gives `@my_decorator def f(): ...` the same binding meaning as `f = my_decorator(f)` without admitting arbitrary module-level statement execution. This unblocks `@cache`, `@retry`, `@validate`, `@app.get`, and other cross-cutting patterns that are natural in Python but still impossible in Incan.

## Motivation

### Decorators as markers vs decorators as wrappers

Today's `@route("/users")` is a compile-time marker. The compiler treats it as route-registration metadata and moves on. The handler function is otherwise unchanged. Users have no mechanism to attach runtime behavior to a function through an ordinary decorator surface.

In Python, decorators are wrappers. `@app.get("/users")` calls `app.get("/users")(get_users)` at module load time. The result replaces `get_users`. The framework intercepts the return value and serializes it. The user just annotates functions and returns plain values.

The consequence in Incan is that the framework currently leaks into the handler:

```incan
# Today: user does the framework's job
@route("/users/{id}")
async def get_user(id: int) -> Json[User]:
    return Json(find_user(id))   # user manually wraps
```

With user-defined decorators, `@app.get` owns the transformation:

```incan
# Goal: decorator owns serialisation
@app.get("/users/{id}")
async def get_user(id: int) -> User:
    return find_user(id)         # just return the value
```

### This is not just a web problem

The same gap affects every cross-cutting concern a library author might want to express:

```incan
@cache(ttl=60)
def expensive_query(id: int) -> Result:
    ...

@retry(attempts=3, on=NetworkError)
async def call_external_api(url: str) -> Response:
    ...

@validate
def create_user(payload: CreateUser) -> User:
    ...
```

None of these can be written today.

### The connection to the RFC tree

Once user-defined decorators land, the web framework's `@app.get` pattern becomes expressible through ordinary decorator syntax instead of through a global compiler-owned marker. Combined with RFC 027 (vocab registration) and RFC 031 (library system), a web library could further offer a declarative DSL form that desugars to the same decorator calls, with no additional surface syntax needed:

```incan
# Declarative DSL (library-defined via RFC 027 VocabDesugarer)
app my_app:
    route GET "/users/{id}" = get_user
    route POST "/users" = create_user

my_app.serve(port=8080)
```

This desugars to the `@app.get`/`@app.post` decorator form, which itself desugars via this RFC. The compiler provides the decorator primitive; libraries provide the ergonomics. Whether route registration can be implemented wholly in ordinary Incan code, or needs stdlib/compiler metadata support for compile-time route collection, is left to RFC 037.

## Goals

- Allow user-defined decorators on `def`, `async def`, and methods.
- Preserve the existing behavior of compiler-owned decorators such as `@derive`, `@staticmethod`, `@classmethod`, `@requires`, `@rust.extern`, and `@route`.
- Desugar user-defined decorators to ordinary callable application before type checking.
- Apply stacked decorators bottom-up, matching Python's decorator ordering.
- Type-check decorator application through the ordinary callable and assignment rules.
- Allow decorator calls to change the visible callable type of the decorated binding.
- Keep decorator semantics compile-time and declaration-oriented; the language must not introduce arbitrary module-level statement execution or module-initialization side effects for decorators.
- Provide the primitive needed for library-owned patterns such as `@app.get`, `@cache`, `@retry`, and `@validate`.

## Non-Goals

- User-defined decorators on classes, models, traits, newtypes, enums, fields, aliases, or module declarations.
- Replacing or removing `@route` in this RFC.
- Defining the full `std.web` routing redesign; that belongs to RFC 037.
- Defining partial application or decorator factory currying; RFC 084 covers partial callable presets.
- Defining a macro system.
- Introducing type-erased decorator calls.

## Guide-level explanation (how users think about it)

### Using decorators

Applying a decorator is a single-line annotation above a `def`. The decorator can be a plain name or a call expression. The compiler handles both forms:

```incan
@logged                      # plain: decorator is a callable
def greet(x: int) -> str:
    return "Hello " + str(x)

@prefix_log(label="greet")   # factory: call returns a callable
def greet(x: int) -> str:
    return "Hello " + str(x)
```

The name `greet` is rebound to whatever the decorator returns. From the call site, nothing changes. `greet` is still called as `greet(42)`.

**Stacking**: multiple decorators on the same function apply bottom-up. The decorator written closest to `def` is applied first, and its result is passed up to the next:

```incan
@app.get("/users/{id}")
@cache(ttl=60)
async def get_user(id: int):
    ...
```

`@cache` wraps `get_user` first; `@app.get` then wraps the cached version.

**Compiler built-ins**: compiler-owned decorators such as `@derive`, `@staticmethod`, `@classmethod`, `@requires`, `@rust.extern`, and `@route` are resolved before desugaring and keep their existing meaning. A decorator name that matches a built-in is handled by the compiler; everything else is treated as user-defined.

**Web routing** — with user-defined decorators, `App` can expose ordinary decorator syntax:

```incan
from std.web import App

app = App()

@app.get("/")
async def index():
    return {"message": "Hello World"}

@app.get("/users/{id}")
async def get_user(id: int):
    return find_user(id)

@app.post("/users")
async def create_user(body: CreateUser):
    return save_user(body)

app.run(port=8080)
```

`app.get("/path")` is a method-shaped decorator factory. That decorator records route metadata and returns the original function, or a response-serializing wrapper. No `Json(...)` wrapping is needed because the decorator owns serialization. No global `@route` is needed because routes are owned by the `app` they are registered with. The exact route collection and runtime handoff model belongs to RFC 037.

### Writing decorators

A decorator is any function that accepts a function and returns a value. The `Callable[Params, R]` sugar from RFC 035 makes the type signature readable without the verbosity of the arrow form:

```incan
def logged(func: Callable[int, str]) -> Callable[int, str]:
    def wrapper(x: int) -> str:
        print("calling with " + str(x))
        result = func(x)
        print("returned " + result)
        return result
    return wrapper
```

`logged` takes a function of type `(int) -> str` and returns a new function of the same type that adds logging around the original call.

A decorator factory is a function-shaped value that takes configuration arguments and returns a decorator. The outer function captures the arguments in a closure-like preset; the inner `decorator` does the actual wrapping.

The three-level nesting is required because `@D(args)` resolves the decorator factory before the decorated function binding exists. The `def` body is not yet available at that point. This means the function cannot be passed alongside the arguments in the same call; a factory that returns a callable is the only way to defer application until the function is ready. Without `@` syntax, you can write the flatter two-argument form directly, `greet = prefix_log(greet, label="greet")`, but that gives up decorator syntax entirely.

```incan
def prefix_log(label: str):
    def decorator(func: Callable[int, str]) -> Callable[int, str]:
        def wrapper(x: int) -> str:
            print("[" + label + "] calling")
            result = func(x)
            print("[" + label + "] returned: " + result)
            return result
        return wrapper
    return decorator
```

`prefix_log` is resolved at the decoration site (`@prefix_log(label="greet")`), capturing `label` from the arguments. It returns `decorator`, which is then applied to the function being decorated.

> **Note:** Both examples above are monomorphic — they only work on `Callable[int, str]` functions. A generic decorator that works on more than one function type may use ordinary generic callable signatures where the current type system can express the parameter and return relationship. More advanced parameter-pack-style callable polymorphism remains outside this RFC.
> **Compared to Python:** In Python, the standard practice is to apply `@functools.wraps(func)` to the inner wrapper function so that introspection tools see the original function's `__name__`, `__doc__`, and signature instead of the wrapper's. In Incan, this is unnecessary — the compiler tracks the binding statically. `greet` is always `greet` in the symbol table regardless of what the decorator returns at runtime. There is no equivalent of `functools.wraps` in Incan and no need for one.

## Reference-level explanation (precise rules)

### Desugaring

Decorator desugaring is a compile-time rewrite that happens after parsing and before type checking. The compiler recognises compiler built-in decorators (`@derive`, `@staticmethod`, `@rust.extern`, etc.) by name first; anything not matching a built-in is treated as a user-defined decorator and desugared.

**Plain decorator** — `D` is an expression that must resolve to a callable:

```incan
@D
def f(params) -> R:
    body
```

The name `f` is first bound to the function definition, then immediately rebound to the result of calling `D` with that function as its sole argument. Semantically equivalent to:

```incan
def f(params) -> R:
    body
f = D(f)
```

**Decorator factory** — `D(args)` is a decorator factory expression resolved at the declaration site. It must return a callable-shaped value, which is then applied to `f`:

```incan
@D(args)
def f(params) -> R:
    body
```

Equivalent to:

```incan
def f(params) -> R:
    body
f = D(args)(f)
```

`args` may be any expression, including keyword arguments (`@retry(attempts=3, on=NetworkError)`).

**Stacked decorators** — multiple decorators apply bottom-up. The decorator written closest to `def` is applied first, its result becomes the input to the next decorator up, and so on:

```incan
@D1
@D2
@D3
def f(params) -> R:
    body
```

Equivalent to:

```incan
def f(params) -> R:
    body
f = D3(f)
f = D2(f)
f = D1(f)
```

This means `D1` wraps `D2`'s result, which wraps `D3`'s result, which wraps the original `f`. Each step may change the callable type of `f`.

**Scope of desugaring** — user-defined decorators desugar on `def`, `async def`, and method declarations. Class, model, trait, newtype, enum, field, alias, and module declarations are out of scope for this RFC.

### Binding and module order

Decorator desugaring is a compile-time declaration rewrite, not arbitrary module-level runtime execution. Incan still does not allow ordinary statements at module scope.

For each decorated declaration, the compiler must produce a binding equivalent to first binding the undecorated function and then replacing that binding with the result of the decorator application. Every later reference to the name in the same module, every export of that name, and every import of that name from another module observes the post-decoration binding. The original undecorated function is not separately addressable unless the decorator itself preserves or returns it. Decorator application must not be implemented as arbitrary user-code execution during module initialization.

Within one module, decorated declarations are processed in source order for dependency and diagnostic purposes. Across modules, decorator dependency analysis follows the ordinary import graph. If a decorator expression depends on a symbol from another module, that module must be available according to the same topological order used for normal declaration checking. Cycles in decorator dependencies that prevent a decorated binding from being resolved are compile errors.

### Type checking

After desugaring, the typechecker treats `f = D(f)` as a regular call expression and assignment. Specifically:

1. `D` must be a callable. If it is not, the compiler emits `decorator 'D' is not callable`.
2. The argument type of `D`'s first parameter must be compatible with `f`'s declared type.
3. The return type of `D(f)` must itself be callable and becomes the new callable type of `f` in the enclosing scope. If `D` returns the same function type it received, `f`'s type is unchanged. If the return type cannot be inferred, an explicit return type annotation on `D` is required.

For decorator factories, step 1 applies to `D(args)` — the factory expression must produce a callable-shaped value — and then steps 2 and 3 apply to that callable applied to `f`.

### v0.3 amendment: generic decorator factories

Issue #640 was accepted as an implementation amendment to this RFC because it naturally extends decorator factories rather than introducing a separate decorator model. A decorator factory may be generic over the decorated function type and return `((F) -> F)`, letting libraries write one registration helper instead of one helper per callable signature:

```incan
pub def registered[F](function_ref: str) -> ((F) -> F):
    return (func) => func

@registered("inql.functions.col")
pub def col(name: str) -> ColumnExpr:
    return ColumnExpr(name=name)
```

The compiler infers `F` from the decorated function when applying the produced decorator. If inference needs an explicit call-site type, the decorator factory call accepts the same bracketed type-argument syntax as ordinary generic calls:

```incan
@registered[(str) -> ColumnExpr]("inql.functions.col")
pub def col(name: str) -> ColumnExpr:
    return ColumnExpr(name=name)
```

This amendment preserves RFC 036's binding contract: later references, exports, imports, checked API metadata, and editor surfaces observe the concrete decorated function signature unless the decorator intentionally returns a different callable shape.

Python decorators can replace a function binding with an arbitrary object. Incan intentionally does not copy that dynamic part of Python's model: user-defined function and method decorators are callable-to-callable transforms. Python's `Callable[[A, B], R]` corresponds to Incan's `(A, B) -> R`; `=>` is only for closure expressions, not callable types. The common generic registry shape is `(F) -> F`; wrappers that intentionally change the callable signature should spell both the source callable type and replacement callable type explicitly.

### Async decorators

A decorator applied to an `async def` receives an async function value. The decorator is responsible for preserving async semantics correctly — typically by defining an `async def wrapper(...)` internally. The compiler does not automatically lift a synchronous wrapper to async; a sync decorator applied to an async function produces a sync-typed result, which is likely a type error at the call site.

### Errors and diagnostics

|               Situation                |                     Diagnostic                      |
| -------------------------------------- | --------------------------------------------------- |
| Decorator is not callable              | `decorator 'X' is not callable`                     |
| Decorator argument type mismatch       | `decorator 'X' expects a function of type …, got …` |
| Decorator factory returns non-callable | `'X(args)' does not return a callable`              |
| Compiler built-in used on wrong target | Existing compiler diagnostics (unchanged)           |

## Design details

### Syntax

RFC 036 originally required no new decorator syntax beyond `@name` and `@name(args)`. The v0.3 implementation amendment also accepts explicit generic call-site arguments on decorator factory calls, as in `@name[T](args)`, using the same type-argument syntax as ordinary generic calls. Unknown decorator names no longer produce an error on `def`, `async def`, or method declarations — they desugar instead.

Method decorator signatures use reference callable parameters for receivers. Immutable method receivers are written as `&Owner`, and mutable method receivers are written as `&mut Owner`, for example `(&Box, int) -> str` and `(&mut Counter, int) -> int`.

Class, model, trait, newtype, enum, field, alias, and module declarations continue to restrict decorators to compiler built-ins where such decorators are supported.

### Module-level declarations

Incan currently allows only declarations at module scope, not statements. Decorator desugaring is therefore represented as a declaration-level binding transformation rather than as a user-visible top-level assignment statement. Backends may emit helper functions, wrapper values, or other implementation artifacts, but they must preserve the compile-time binding contract: the exported and imported name is the decorated value.

### Ordering across modules

If `module_a` decorates with `module_b`'s `app` object, `module_b`'s exported binding for `app` must be resolved before `module_a`'s decorated binding can be checked. The compiler resolves this statically from the import graph and decorator dependency graph. Circular decoration that prevents either module from resolving its decorated binding is a compile error.

### Interaction with existing features

**`@derive`, `@staticmethod`, `@classmethod`, `@requires`, `@rust.extern`, `@route`**: Compiler built-ins, unchanged. Recognised by name before desugaring runs.

**Closures**: Unaffected. Ordinary closures and named function references from RFC 035 are both valid decorator arguments as long as they type-check as callables.

**`@route`**: Continues to work as a compiler-owned decorator. This RFC does not deprecate or remove it. The desired end-state is for route registration to be expressible through ordinary decorator syntax such as `@app.get` / `@app.post`, but whether the existing global `@route` can be fully re-expressed in ordinary Incan code belongs to RFC 037 or a dedicated web-routing transition RFC.

**RFC 084 (partial callable presets)**: Decorator factories may become less verbose once RHS partial callable presets are available. This RFC does not depend on partials and does not define an `@decorator` stdlib helper.

**RFC 027 (vocab) + RFC 031 (library)**: After those land, a library can register a DSL keyword that desugars into decorator calls. This RFC provides the decorator primitive; the vocab desugarer generates the `@app.get`-style calls; the library packages it all. The three compose cleanly.

### Compatibility / migration

Fully additive and non-breaking. Previously-invalid unknown decorators on functions and methods now desugar rather than error. All existing compiler built-in decorators are unaffected.

## Alternatives considered

**Compiler built-ins only**: Every new cross-cutting concern requires a compiler change. Does not scale.

**Macro system**: More powerful but requires a separate compilation step and a different mental model. Incan targets Python familiarity; decorator semantics are the right level.

**Type-erased decorators**: Simpler to implement, but loses static type safety at decorator boundaries. Rejected in favour of typed decorators with inference.

## Drawbacks

- **Declaration-level rebinding** adds compiler complexity because the post-decoration binding must replace the original function binding for later references, exports, imports, metadata, and LSP queries.
- **Generic decorator typing** is non-trivial for the initial implementation; decorators that work on any function type may require explicit type parameters where Python would not need them.
- **Decorator dependency ordering** across modules must be deterministic and correct — any implementation must respect import order and reject cycles that prevent decorated bindings from resolving.

## Layers affected

**Prerequisites:** RFC 035 (first-class named function references) has landed.

- **Parser** — unknown `@decorator` names on `def`, `async def`, and method declarations must remain in the AST as user-defined decorator candidates instead of being rejected as unknown compiler decorators.
- **Typechecker** — verify decorator callability and infer the post-decoration type of `f`; emit diagnostics for mismatched or non-callable decorators.
- **IR Lowering / Emission** — lower decorated declarations so the emitted binding has the post-decoration value and later references, exports, and imports observe that decorated binding.
- **Stdlib (web)** — once the primitive lands, `App` and `router` can expose ordinary decorator syntax using `@app.get` / `@app.post`; the exact route collection and global `@route` transition are deferred.
- **LSP** — hover on a decorated binding should show the post-decoration type.

## Implementation Plan

### Phase 1: Parser, AST, and Decorator Classification

- Keep parsed decorator syntax unchanged, but stop rejecting unknown decorators on functions, async functions, and methods during builtin decorator validation.
- Preserve compiler-owned decorators through the existing decorator registry and diagnostics.
- Add explicit rejection for user-defined decorators on unsupported declaration targets.
- Add focused parser/typechecker tests that prove functions and methods can carry unknown decorator candidates while unsupported targets still reject them.

### Phase 2: Typechecker Binding Semantics

- Type-check user-defined decorator expressions as callable values applied to the decorated function binding.
- Apply stacked decorators bottom-up and update the visible binding type after each decorator application.
- Support decorator factories by checking the factory expression first and then checking the returned callable-shaped value against the decorated binding.
- Emit targeted diagnostics for non-callable decorators, argument mismatches, and factory results that are not callable.
- Preserve compiler-owned decorator behavior, including `@route`, `@rust.extern`, `@staticmethod`, `@classmethod`, and `@requires`.

### Phase 3: Lowering and Emission

- Lower decorated function and method declarations so generated code exposes the post-decoration binding.
- Ensure later references, exports, imports, checked API metadata, and emitted code observe the decorated binding rather than a stale undecorated function type.
- Avoid introducing arbitrary module-level statement execution or module-initialization side effects.
- Add codegen snapshot and integration coverage for plain decorators, decorator factories, stacked decorators, and methods.

### Phase 4: Tooling, Docs, Versioning, and Closeout

- Update LSP/checked metadata surfaces so hover and API metadata report post-decoration binding types where those types are known.
- Update authored user-facing documentation for decorators and callable references.
- Add a release note entry for RFC 036 and bump the active dev version.
- Run focused verification during development and the repo-level gate before closeout.

## Implementation log

### Spec / RFC

- [x] Resolve RFC 036 open questions and move settled answers into Design Decisions.
- [x] Move RFC 036 to In Progress before implementation starts.
- [x] Keep the checklist current as implementation slices land.

### Parser / AST

- [x] Preserve unknown decorator syntax on functions, async functions, and methods.
- [x] Keep compiler-owned decorator parsing and formatting unchanged.
- [x] Reject user-defined decorators on unsupported declaration targets.
- [x] Add parser/typechecker coverage for valid and invalid decorator targets.

### Typechecker

- [x] Classify decorators as compiler-owned or user-defined after import/alias resolution.
- [x] Type-check plain user-defined decorators as callable application to the decorated function value.
- [x] Type-check decorator factories as factory expression plus returned callable application.
- [x] Apply stacked decorators bottom-up and update the visible binding type after each step.
- [x] Preserve builtin decorator behavior and diagnostics.
- [x] Add diagnostics for non-callable decorators, type mismatches, and non-callable factory results.

### Lowering / Emission

- [x] Lower decorated functions and methods to post-decoration bindings.
- [x] Ensure later references, exports, imports, and emitted code observe the decorated type.
- [x] Avoid arbitrary module-level statement execution or module-initialization side effects.
- [x] Add codegen snapshot coverage for plain decorators, immutable methods, and mutable methods.

### Tooling / Metadata

- [x] Update checked API metadata for post-decoration function and method types where known.
- [x] Update LSP hover behavior for decorated bindings where known.
- [x] Preserve decorator metadata for existing docs/tooling consumers.

### Docs / Release

- [x] Update authored docs for user-defined decorators.
- [x] Add a release notes entry for RFC 036 / #170.
- [x] Bump the active dev version from `0.3.0-dev.33`.

### Verification

- [x] Run focused parser/typechecker tests for decorator target and diagnostic behavior.
- [x] Run focused lowering/codegen snapshot tests for decorated bindings.
- [x] Run docs verification for edited docs-site content.
- [x] Run `make fmt`.
- [x] Run `make pre-commit`.

## Design Decisions

1. **Generic decorators**: RFC 036 allows generic decorators where the current type system can express the decorator's callable signature and infer or check the application. More advanced parameter-pack-style callable polymorphism remains outside this RFC.

2. **Methods are in scope; type declarations are not**: User-defined decorators are valid on functions, async functions, immutable methods, and mutable methods. Class, model, trait, newtype, enum, field, alias, and module decorators remain compiler-built-ins only where supported. User-defined class and model decorators can be revisited once clear use cases exist.

3. **`@route` stays compiler-owned for now**: `@route` displays as a decorator but receives special compiler treatment today. This RFC keeps it working unchanged while making route-style APIs expressible through ordinary decorator syntax. Any deprecation or re-expression of global `@route` belongs to RFC 037 or a dedicated web-routing transition RFC.

4. **`@decorator` utility is deferred to RFC 084 or later**: Decorator factories require three levels of `def` nesting, which is verbose. A stdlib utility `@decorator` could reduce this to two levels by automatically presetting or currying the decorated function argument, but that relies on partial callable semantics. RFC 084 is the planned foundation for that ergonomics work; RFC 036 does not define the utility.

5. **Decorator application is compile-time binding semantics**: User-defined decorators are specified as compile-time declaration rewrites. Implementations must not model them as arbitrary runtime top-level statements. The observable contract is that later references, exports, imports, metadata, and editor tooling see the post-decoration binding.
