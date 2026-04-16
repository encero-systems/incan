# RFC 036: user-defined decorators

- **Status:** Draft
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
- **Issue:** https://github.com/dannys-code-corner/incan/issues/328
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

Incan's decorator system currently consists entirely of compiler built-ins such as `@derive`, `@staticmethod`, `@rust.extern`, and `@route`. Those forms are compiler-recognized annotations rather than ordinary user-extensible language abstractions. Users cannot define their own decorators.

This RFC introduces user-defined decorators: any callable, whether a function or an object, that accepts a function and returns a value. The compiler desugars `@my_decorator def f(): ...` into `f = my_decorator(f)`, exactly as Python does. This unblocks `@cache`, `@retry`, `@validate`, `@app.get`, and other cross-cutting patterns that are natural in Python but still impossible in Incan.

## Motivation

### Decorators as markers vs decorators as wrappers

Today's `@route("/users")` is a compile-time marker. The compiler treats it as route-registration metadata and moves on. The handler function is otherwise unchanged. Users have no mechanism to attach runtime behavior to a function through an ordinary decorator surface.

In Python, decorators are wrappers. `@app.get("/users")` calls
`app.get("/users")(get_users)` at module load time. The result replaces
`get_users`. The framework intercepts the return value and serializes it. The
user just annotates functions and returns plain values.

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

Once user-defined decorators land, the web framework's `@app.get` pattern becomes implementable in pure Incan. Combined with RFC 027 (vocab registration) and RFC 031 (library system), a web library could further offer a declarative DSL form that desugars to the same decorator calls, with no additional compiler feature needed:

```incan
# Declarative DSL (library-defined via RFC 027 VocabDesugarer)
app my_app:
    route GET "/users/{id}" = get_user
    route POST "/users" = create_user

my_app.serve(port=8080)
```

This desugars to the `@app.get`/`@app.post` decorator form, which itself desugars via this RFC. The compiler provides the primitive; libraries provide the ergonomics.

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

**Stacking**: multiple decorators on the same function apply bottom-up. The
decorator written closest to `def` is applied first, and its result is passed up to the next:

```incan
@app.get("/users/{id}")
@cache(ttl=60)
async def get_user(id: int):
    ...
```

`@cache` wraps `get_user` first; `@app.get` then wraps the cached version.

**Compiler built-ins**: compiler-owned decorators such as `@derive`,
`@staticmethod`, and `@rust.extern` are resolved before desugaring and keep
their existing meaning. A decorator name that matches a built-in is handled by the compiler; everything else is treated as user-defined.

**Web routing** — with user-defined decorators, `App` can be implemented entirely in Incan:

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

`app.get("/path")` is a method that returns a decorator. That decorator
registers the route and returns the original function, or a response-serializing wrapper. No `Json(...)` wrapping is needed because the decorator owns serialization. No global `@route` is needed because routes are owned by the `app` they are registered with.

### Writing decorators

A decorator is any function that accepts a function and returns a value. The
`Callable[Params, R]` sugar from RFC 035 makes the type signature readable
without the verbosity of the arrow form:

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

A decorator factory is a function that takes configuration arguments and returns a decorator. The outer function captures the arguments in a closure; the inner `decorator` does the actual wrapping.

The three-level nesting is required because `@D(args)` evaluates `D(args)` before the decorated function exists. The `def` body is not yet available at that point. This means the function cannot be passed alongside the arguments in the same call; a factory that returns a callable is the only way to defer application until the function is ready. Without `@` syntax, you can write the flatter two-argument form directly, `greet = prefix_log(greet, label="greet")`, but that gives up decorator syntax entirely.

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

`prefix_log` is called at the decoration site (`@prefix_log(label="greet")`), capturing `label` from the arguments. It returns `decorator`, which is then applied to the function being decorated.

> **Note:** Both examples above are monomorphic — they only work on `Callable[int, str]` functions. A generic decorator that works on *any* function type requires type parameters on `decorator`. See [Unresolved question 1](#unresolved-questions).
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

**Decorator factory** — `D(args)` is a call expression evaluated at the decorator site. It must return a callable, which is then applied to `f`:

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

This means `D1` wraps `D2`'s result, which wraps `D3`'s result, which wraps the original `f`. Each step may change the type of `f`.

**Scope of desugaring** — user-defined decorators desugar on `def` and `async def` declarations. Class, model, and trait declarations are out of scope for this RFC (see [Unresolved questions](#unresolved-questions)).

### Execution order

Decorator applications at module scope are not statements — Incan does not allow statements at module scope. The compiler lifts them into a startup sequence that runs before the user's `main()`, in the order they appear in source. If decorators span multiple modules, the startup sequence respects the topological import order: a module's decorators run only after all modules it imports have finished their own startup sequences. Circular decoration across modules (A's decorator depends on B's object, B's decorator depends on A's object) is a compile error.

### Type checking

After desugaring, the typechecker treats `f = D(f)` as a regular call expression and assignment. Specifically:

1. `D` must be a callable. If it is not, the compiler emits `decorator 'D' is not callable`.
2. The argument type of `D`'s first parameter must be compatible with `f`'s declared type.
3. The return type of `D(f)` becomes the new type of `f` in the enclosing scope. If `D` returns the same function type it received, `f`'s type is unchanged. If the return type cannot be inferred, an explicit return type annotation on `D` is required.

For decorator factories, step 1 applies to `D(args)` — the factory call must return a callable — and then steps 2 and 3 apply to that callable applied to `f`.

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

No new syntax is introduced. `@name` and `@name(args)` already parse. The only change is that unknown decorator names no longer produce an error on `def`/`method` declarations — they desugar instead.

Class, model, and trait declarations continue to restrict decorators to compiler built-ins for now (see [Unresolved questions](#unresolved-questions)).

### Module-level initialisation

Incan currently allows only declarations at module scope, not statements. Decorator desugaring produces `f = D(f)` — a statement. The compiler lifts all such assignments into a startup sequence that runs before the user's `main()`. From the user's perspective, module-level decorator applications simply happen at program startup in declaration order.

### Startup ordering across modules

If `module_a` decorates with `module_b`'s `app` object, `module_b`'s startup sequence must complete before `module_a`'s. The compiler resolves this statically from the import graph — the same topological sort used for module compilation order. Circular decoration (module A decorates with module B's object, module B decorates with module A's object) is a compile error.

### Interaction with existing features

**`@derive`, `@staticmethod`, `@rust.extern`**: Compiler built-ins, unchanged. Recognised by name before desugaring runs.

**Closures**: Unaffected. Ordinary closures and named function references from
RFC 035 are both valid decorator arguments as long as they type-check as callables.

**`@route`**: Continues to work. A follow-up RFC (web routing redesign) can deprecate it in favour of `@app.get` / `@app.post`.

**RFC 027 (vocab) + RFC 031 (library)**: After those land, a library can register a DSL keyword that desugars into decorator calls. This RFC provides the decorator primitive; the vocab desugarer generates the `@app.get`-style calls; the library packages it all. The three compose cleanly.

### Compatibility / migration

Fully additive and non-breaking. Previously-invalid unknown decorators now desugar rather than error. All existing compiler built-in decorators are unaffected.

## Alternatives considered

**Compiler built-ins only**: Every new cross-cutting concern requires a compiler change. Does not scale.

**Macro system**: More powerful but requires a separate compilation step and a different mental model. Incan targets Python familiarity; decorator semantics are the right level.

**Type-erased decorators**: Simpler to implement, but loses static type safety at decorator boundaries. Rejected in favour of typed decorators with inference.

## Drawbacks

- **Module-level startup sequence** adds a new emission path to the compiler for lifting decorator applications out of module scope.
- **Generic decorator typing** is non-trivial for the initial implementation; decorators that work on any function type may require explicit type parameters where Python would not need them.
- **Startup ordering** across modules must be deterministic and correct — any implementation must respect the topological import order.

## Layers affected

**Prerequisites:** RFC 035 (first-class named function references) must land first.

- **Parser** — unknown `@decorator` names on `def`/method declarations currently error; the error must be removed and a desugaring pass added that rewrites `@D def f(): ...` to the assignment form.
- **Typechecker** — verify decorator callability and infer the post-decoration type of `f`; emit diagnostics for mismatched or non-callable decorators.
- **IR Lowering / Emission** — lift module-level decorator applications into a startup sequence that executes before `main()`, respecting the topological import order.
- **Stdlib (web)** — once the primitive lands, `App` and `router` can be re-implemented in Incan using `@app.get` / `@app.post`; the global `@route` can be deprecated in a follow-up.
- **LSP** — hover on a decorated binding should show the post-decoration type.

## Unresolved questions

1. **Generic decorators**: A decorator like `@logged` that works on *any* function type requires higher-order generics (`logged[P, R](func: Callable[P, R]) -> Callable[P, R]`). Initial implementation may require explicit type parameters. Full inference deferred to a generics RFC.

2. **Class and model decorators**: Should `@my_decorator class Foo: ...` be allowed? The same desugaring applies (`Foo = my_decorator(Foo)`). For now, type-declaration decorators remain compiler-built-ins only. Revisit once user-defined class decorators have clear use cases.

3. **`@route` deprecation timeline**: When does global `@route` get deprecated — in this RFC's scope, or deferred to the web routing redesign RFC?

4. **`@decorator` stdlib utility**: Decorator factories require three levels of `def` nesting, which is verbose. A stdlib utility `@decorator` could reduce this to two levels by automatically currying the `func` argument — a factory written as `def D(func, ...args)` would be transformed such that `D(args)` returns a partial application waiting for `func`, and `@D(args) def f()` completes it as `D(f, args)`. This is implementable as a stdlib decorator (meta: a decorator that makes decorators) but requires partial application semantics not yet defined in Incan. Deferred to a follow-up RFC, possibly in conjunction with RFC 038.

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
