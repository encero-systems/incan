# RFC 052: Module Static Storage

- **Status:** Implemented
- **Created:** 2026-04-07
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 008 (const bindings)
    - RFC 017 (validated newtypes with implicit coercion)
    - RFC 023 (compilable stdlib and Rust module binding)
    - RFC 033 (ctx keyword)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/242
- **RFC PR:** https://github.com/dannys-code-corner/incan/pull/243
- **Written against:** v0.2
- **Shipped in:** v0.2

## Summary

This RFC proposes `static` as a new module-level declaration form for process-lifetime mutable storage owned by a module. `const` remains the immutable top-level binding form. `static` is intentionally Python-like: it exposes one live module-scoped binding initialized once at module load time, supports ordinary mutable values, and may be exported when shared mutable module state is part of the intended API.

## Core model

1. `const` remains the immutable top-level declaration form and does not change in this RFC.
2. `static` declares one mutable storage cell owned by the defining module and initialized once, eagerly, when the module is initialized.
3. Reads of a `static` expose the live stored value rather than a cloned copy; assignment to the static name mutates the storage cell rather than rebinding a local variable.
4. `static` is for module-owned runtime state, not configuration singletons and not compile-time constants.
5. This RFC introduces module static storage, not a full synchronization, atomics, or read-only view surface.

## Motivation

Incan can already express immutable top-level values via `const`, ordinary local mutable variables, and stateful instance fields on `class` values. It cannot currently express one narrow but important category of state: library-owned mutable storage that persists across calls without being threaded through every API explicitly.

That gap is small in syntax but real in effect. A library can easily need a monotonic id allocator, a module-local registry, a cache, or a one-time scratch store. Today the author must either thread an explicit state object through the public API or drop into Rust interop for something that is conceptually simpler than the surrounding Incan code.

That fallback is the wrong showcase boundary for the language. RFC 023 explicitly frames `@rust.extern` as a narrow leaf mechanism and says it should be applied at the "smallest possible set of primitives" where the host boundary is genuinely irreducible. A module-local counter or registry is not conceptually irreducible runtime I/O; it is a missing language/runtime storage capability.

RFC 033 does not solve this problem. `ctx` is a typed configuration surface that produces a "set-once singleton" with environment-aware initialization. That is the right tool for application configuration and the wrong tool for mutable library runtime state.

## Goals

- Introduce `static` as a first-class module-level declaration for mutable persistent storage.
- Keep the mental model simple: `const` is immutable, `static` is mutable module-owned state.
- Support ordinary library use cases such as counters, registries, and caches without Rust fallback.
- Preserve clear ownership of the module binding while still allowing intentional shared mutable exports through `pub static`.
- Leave room for future synchronization primitives without making this RFC depend on them.

## Non-Goals

- Replacing `const` or redefining its semantics.
- Reusing `ctx` for general mutable runtime state.
- Introducing a full atomics, locks, or thread-local storage surface in the same RFC.
- Introducing class-owned static storage or class variables. That carries separate inheritance, member lookup, and shadowing semantics and should be handled by a dedicated follow-up RFC if Incan wants it.
- Turning Incan into a language that encourages arbitrary ambient global mutation as the default design style.
- Defining distributed, cross-process, or persisted storage semantics.

## Guide-level explanation

### A module-owned counter

```incan
type StoreId = newtype int

static next_store_id: StoreId = StoreId(0)

def allocate_store_id() -> StoreId:
    current = next_store_id
    next_store_id = StoreId(current.0 + 1)
    return current
```

`next_store_id` is not a local variable and not a `const`. It is one storage cell owned by the module. Each call to `allocate_store_id()` sees the updated value from prior calls.

This example uses explicit newtype construction and unwrapping so the RFC does not silently depend on implicit coercion or numeric operator lifting for newtypes. If RFC 017 lands with typed-initializer coercion, `static next_store_id: StoreId = 0` may also become valid, but that is not required by this RFC.

### A module-local registry

```incan
static registered_names: list[str] = []

def register_name(name: str) -> None:
    registered_names.append(name)

def registered_count() -> int:
    return len(registered_names)
```

The registry persists across function calls without forcing callers to carry a registry object through every API boundary.

### Exported shared mutable state

```incan
pub static registered_names: list[str] = []

def register_name(name: str) -> None:
    registered_names.append(name)
```

Another module may import `registered_names` and observe or mutate the same live list value. The binding itself still belongs to the defining module, so rebinding that name from another module is not allowed.

### How users should think about it

- Use `const` when the value is immutable.
- Use `static` when the module owns mutable runtime state and, when exported, intentionally shares that state.
- Use `class` fields when a caller should own and pass around the state explicitly.
- Use `ctx` when the problem is typed application configuration, not mutable runtime storage.
- Name `static` bindings like ordinary mutable variables (`snake_case`), not like constants. They are shared storage cells, not immutable values.

## Reference-level explanation

### Declaration form

A `static` declaration has the form:

```incan
static name: Type = initializer
```

An exported static has the form:

```incan
pub static name: Type = initializer
```

### Placement rules

- `static` declarations must appear in module scope, as part of the module body. They do not need to be the first declaration in the file.
- `static` declarations must not appear inside functions, methods, traits, `model` bodies, `class` bodies, or control-flow blocks.
- A `static` declaration must include an explicit type annotation.
- A `static` declaration must include an initializer.

### Storage and lifetime

- Each `static` declaration must create exactly one storage cell per module instance.
- The initializer must run exactly once, eagerly, when the module is initialized.
- The value stored in the cell must persist for the lifetime of that module instance.
- Reads of the static must observe the current contents of the storage cell, not a copied constant value.
- Binding a name to a static's value must expose the live stored value rather than a cloned copy.

### Mutation rules

- Assignment to a `static` name in its defining module must mutate the storage cell.
- Compound assignment to a `static` name in its defining module must mutate the storage cell.
- Method calls that mutate the current value stored in a `static` are permitted in the defining module when the stored type supports that operation.
- Direct assignment or compound assignment to an imported `static` name from a different module must be a compile error, even if the static is public.
- If a `pub static` exposes a mutable value, mutating that live value through ordinary aliasing, method calls, or field assignment is permitted.
- Rebinding a local variable with the same name must follow ordinary shadowing rules and must not mutate the static unless the name being assigned resolves to the static itself.

### Visibility and imports

- `static` declarations follow ordinary module visibility rules.
- A non-`pub` static must remain private to its defining module.
- A `pub static` may be imported or referenced from another module.
- Importing a `pub static` exposes the same live stored value rather than a copied snapshot.
- Public visibility does not grant ownership of the binding cell itself; imported rebinding remains invalid.

### Initialization restrictions

- A `static` initializer is evaluated at module initialization time, not under RFC 008 const-evaluable rules.
- A `static` initializer may use ordinary expression forms that are valid at module scope, including constructor calls and function calls, subject to the restrictions below.
- A `static` initializer must not assign to any `static`.
- A `static` initializer may reference earlier-declared `static` values and visible `const` values.
- A `static` initializer must not reference a later-declared `static`.
- Cyclic initialization between statics must be rejected.

### Type checking

- The initializer expression must be assignable to the declared static type.
- Later assignments to the static must be assignable to the declared static type.
- Reads of a static use the declared type directly.
- A `static` declared with a mutable value type uses that type's ordinary mutation surface; this RFC does not impose implicit copy or clone semantics on static values.
- `const` and `static` remain distinct declaration kinds; a `const` must not be reassigned, while a `static` may be mutated.

### Diagnostics

The compiler should produce targeted diagnostics for at least these cases:

- `static` used outside module top level.
- Missing type annotation on a `static`.
- Missing initializer on a `static`.
- Attempted assignment to an imported `static`.
- Attempted reassignment of a `const` where the author likely meant `static`.
- Forward reference to a later `static` during initialization.
- Cyclic static initialization.

## Design details

### Syntax

This RFC introduces `static` as a distinct top-level declaration form. The concrete declaration syntax is defined above in `Reference-level explanation / Declaration form`.

This RFC does not introduce `static mut`. `static` is the mutable storage form. `const` already owns the immutable top-level role.

### Semantics

`static` introduces module-owned mutable runtime storage. It is not a local rebinding and not a compile-time constant. Reads access the current stored value directly. Writes replace or mutate that stored value according to the operation performed. When a static stores a mutable object, ordinary aliasing exposes the same live object rather than a copied value.

The distinction from `const` is intentional and sharp:

- `const` is an immutable named value.
- `static` is a mutable storage cell with module lifetime.

This RFC intentionally distinguishes binding ownership from object mutability. The defining module owns the binding cell, so imported rebinding is invalid. But if a public static stores a mutable object, that shared object may be mutated through ordinary aliasing and method calls.

Rust's `static` is the closest familiar analogue, but Incan is not copying Rust literally. The Rust Reference says a static item "represents an allocation in the program" and that all references point at the same allocation. It also says the initializer is a constant expression and that mutable statics require `unsafe`. Incan keeps the single-live-cell intuition while intentionally choosing eager module initialization and one mutable `static` form instead of Rust's `static` / `static mut` split. See [The Rust Reference: Static items](https://doc.rust-lang.org/reference/items/static-items.html).

### Interaction with existing features

- **`const`**: `const` remains the immutable top-level binding form. This RFC does not relax `const` reassignment rules.
- **`ctx`**: `ctx` remains the configuration singleton surface. `static` is for mutable runtime storage, not configuration resolution.
- **`class`**: `class` fields remain the right tool when state should be owned by an explicit value passed through the API.
- **Class-owned state**: this RFC does not introduce class-level statics or singleton type members. Those semantics should be designed separately if Incan wants them.
- **Imports / modules**: statics become ordinary module members for name resolution and visibility; `pub static` intentionally allows shared mutable exports while keeping rebinding rules module-owned.
- **Async / concurrency**: this RFC guarantees one-time eager initialization and a coherent shared-state surface, but it does not promise that concurrent compound mutations on escaped mutable aliases are serialized or atomic.
- **Rust interop**: `static` should reduce the need for Rust-backed helper leaves whose only job is to hold trivial module-local state.

### Compatibility / migration

This RFC is additive. Existing programs remain valid. The only new reserved word introduced is `static`, so any existing user-defined identifier named `static` would need to migrate once the keyword lands.

Libraries that currently use Rust-backed helpers purely to allocate ids or hold trivial module state may migrate those leaves into pure Incan once `static` exists, but this RFC does not require such migration.

## Alternatives considered

- **Keep using Rust-backed helpers for counters and registries**: rejected because it hides a genuine language/runtime gap behind interop and weakens Incan as the source-of-truth surface for its own libraries.
- **Use `ctx` for library-owned mutable state**: rejected because RFC 033's model is a set-once configuration singleton, not a general mutable runtime storage facility.
- **Introduce only a stdlib counter or registry type**: rejected because it solves specific symptoms rather than the language capability gap; library-owned persistent storage should not require a bespoke standard type for every pattern.
- **Allow bare top-level variables without a keyword**: rejected because `const` already owns immutable top-level values and a distinct keyword makes stateful storage easier to reason about.
- **Allow `static` inside `class` bodies in the same RFC**: deferred. Class-owned shared storage is conceptually adjacent, but it requires separate decisions around inheritance, shadowing, lookup through the type surface, and whether instances may read class statics through attribute access.
- **Split the feature into `static` and `static mut`**: rejected for now because `const` already covers the immutable top-level role and a single mutable `static` form keeps the mental model simpler.
- **Clone-on-read static values**: rejected because it pulls the feature away from Python-style module-state expectations and would make exported mutable statics feel unlike ordinary live shared objects.

## Drawbacks

- `static` introduces ambient mutable state, which can be misused and can make code harder to reason about if over-applied.
- Module initialization becomes more semantically important because statics now have runtime initialization behavior.
- Shared mutable exports mean aliasing becomes a deliberate part of the language surface rather than a purely internal runtime detail.
- Concurrency semantics become a language/runtime concern rather than something authors can ignore, and this RFC does not make shared mutable statics automatically race-free.
- Tooling must make the distinction between immutable declarations and mutable module state visible and understandable.

## Implementation architecture

One recommended internal model is to treat each `static` as one compiler-recognized module storage cell with explicit read and write operations in lowering, rather than lowering it as a disguised local variable. Backends should preserve the "one cell per module instance" contract, eager top-to-bottom initialization behavior, and live shared-object semantics rather than relying on constant inlining or clone-on-read shims.

This RFC does not require one specific backend storage strategy. It only requires the user-visible semantics described above.

## Layers affected

- **Parser / AST**: the parser must recognize `static` as a top-level declaration form and represent it distinctly from `const`.
- **Typechecker / Symbol resolution**: the compiler must typecheck static initializers and assignments, resolve statics as module members, reject imported rebinding, and enforce earlier-only static references during initialization.
- **IR lowering**: lowering must preserve the difference between reading a module storage cell and reading an immutable value, including live shared-object semantics for mutable stored values.
- **Emission**: generated output must preserve one-cell-per-module-instance semantics and eager top-to-bottom initialization behavior.
- **Stdlib / Runtime (`incan_stdlib`)**: runtime support may be needed for initialization ordering and shared mutable storage behavior, but this RFC does not require automatic synchronization for escaped aliases.
- **Formatter**: the formatter must support `static` declarations and preserve canonical spacing and ordering.
- **LSP / Tooling**: hover, completion, rename, diagnostics, and symbol displays should surface `static` as mutable module state rather than as a constant.

## Implementation Plan

- Add `static` as a distinct declaration kind in the lexer/parser/AST/formatter and surface it through LSP/tooling.
- Extend typechecking and symbol resolution so module statics are first-class bindings with declaration-order validation, imported-rebinding diagnostics, and local live-alias tracking.
- Introduce first-class IR support for module statics plus runtime storage helpers in `incan_stdlib` so reads, writes, and direct aliases preserve live storage semantics.
- Extend library export manifests and `pub::` import resolution so `pub static` shares the same storage cell across modules.
- Cover the feature with parser, typechecker, codegen snapshot, and end-to-end runtime tests, then update language docs and release notes.

## Implementation log

- [x] Parser/AST/formatter support for `static` / `pub static`
- [x] Typechecker support for module-scope placement, required annotation/initializer, declaration-order rules, cycle rejection, and imported-static rebinding diagnostics
- [x] IR lowering and Rust emission support for first-class module static storage
- [x] Runtime storage helpers in `incan_stdlib` for live reads, writes, and direct aliases
- [x] Library manifest / `pub::` support for exported statics
- [x] Language docs and release notes updated
- [x] Targeted parser, typechecker, codegen snapshot, and runtime tests added
- [x] Full repository verification gate (`mkdocs build --strict`, `make fmt`, `make pre-commit-full`, `make smoke-test`)

## Design Decisions

- `pub static` is part of v1. Exporting shared mutable module state is an intended use case rather than a deferred capability.
- Static initialization is eager and follows module declaration order.
- Static initializers may reference earlier-declared statics and visible consts, but not later-declared statics.
- `static` uses live shared-object semantics rather than clone-on-read semantics.
- Imported rebinding of a `static` name is rejected, but mutation through ordinary aliases and method calls on exported mutable values is allowed.
- This RFC does not promise automatic race-free or atomic behavior for concurrent mutations on escaped mutable aliases.
