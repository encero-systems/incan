# RFC 023: Compilable Stdlib & Rust Module Binding

- **Status:** Implemented
- **Created:** 2026-02-08
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 005 (Rust interop), RFC 013 (Rust crate dependencies), RFC 022 (stdlib namespacing & compiler→stdlib handoff)
- **Target version:** 0.1.0
- **Implemented version:** 0.2.0

> Note: (re-baselined closure criteria: `.incn` source-of-truth, Incan-first stdlib, narrow runtime bridges)

## Summary

This RFC proposes two related changes that reduce the compiler's role as a registry of stdlib implementations, and enable an ecosystem of Rust-backed Incan libraries:

1. **Compilable stdlib**: stdlib `.incn` files transition from documentation-only stubs to **real, compilable Incan source code**. The compiler compiles them through the normal pipeline. Rust-backed leaves are kept narrow and explicit: most use `@rust.extern`, while source-declared wrappers may still use internal `rust::` imports when that keeps the public Incan surface as the source of truth.
2. **`rust.module()` binding**: a new module-level directive that declares an Incan module (or stdlib module) is backed by a specific Rust module path. This replaces hardcoded path-rewriting in the compiler with a data-driven declaration, and enables third-party Incan libraries backed by Rust crates.

Together, these changes push the Incan stdlib toward being written in **mostly plain Incan**, with a minimal set of Rust-backed primitives — and make the same pattern available to the ecosystem, allowing users to write their own rust-backed Incan libraries.

## Implemented Closeout Notes

- `.incn` source is now the source of truth for the stdlib surfaces closed out under RFC 023, including `std.async`, `std.math`, `std.reflection`, and `std.traits.{convert,ops,error,indexing,callable,prelude}`.
- `std.traits.convert` now compiles through the normal stdlib pipeline via `@classmethod` conversion hooks for `from` / `try_from`. Broader Rust trait-impl authoring on wrappers remains follow-up work under RFC 043.
- Build, test, and lock flows derive stdlib-driven feature/extra-dependency activation from shared namespace metadata (for example `std.async` enabling Tokio and `std.math` pulling `libm`).
- Explicit generic `with` bounds are enforced in the frontend against concrete argument types; backend trait-bound inference remains additive rather than the first place bound violations show up.
- `@rust.extern` declaration-shape errors are caught in the frontend, and downstream Cargo/`rustc` failures are wrapped back onto the `.incn` declaration site in the CLI build surface.

## Motivation

### The stdlib is all Rust, and the compiler is the bottleneck

Today, Incan's stdlib is implemented entirely in Rust (`crates/incan_stdlib/src/`), and the `.incn` files in `stdlib/`
are documentation-only stubs that the compiler ignores. Adding a single function to a stdlib module requires touching
up to five files across four compiler stages:

1. **Rust implementation** (`crates/incan_stdlib/src/testing.rs`) — write the function
2. **Incan stub** (`stdlib/testing.incn`) — add the signature for docs/IDE
3. **Typechecker registry** (`src/frontend/typechecker/collect.rs`) — hardcode the function's type signature
4. **Emission mapping** (`src/backend/ir/emit/decls.rs`) — hardcode the `std.testing` → `incan_stdlib::testing` path
   rewrite
5. **Module registry** (`crates/incan_core/src/lang/stdlib.rs`) — register the module metadata

The function signature is duplicated (once in the `.incn` stub, once as handwritten Rust data structures in the
typechecker), and they can drift. The path-rewriting in emission is a growing `if/else` chain that must be extended for
every new stdlib module. After this RFC, the `.incn` source file becomes the **single source of truth** — the compiler
parses it for type signatures and compiles it into the generated output — eliminating both the duplication and the
manual wiring.

### Most stdlib code doesn't need to be Rust

Many stdlib functions are algorithmically simple and can be written in Incan, provided a small set of Rust-backed
primitives exists. The stdlib already demonstrates this pattern:

```incan title="stdlib/derives/comparison.incn"
# already today
trait Ord:
    @rust.extern
    def __lt__(self, other: Self) -> bool: ...    # Rust-backed primitive

    def __le__(self, other: Self) -> bool:        # Pure Incan
        return self.__lt__(other) or self.__eq__(other)

    def __gt__(self, other: Self) -> bool:        # Pure Incan
        return other.__lt__(self)

    def __ge__(self, other: Self) -> bool:        # Pure Incan
        return other.__lt__(self) or self.__eq__(other)
```

Only `__lt__` is `@rust.extern`; the other three are pure Incan built on that primitive. The same pattern applies
broadly. For example, `std.testing` has 12+ functions but only one (`fail()`) is irreducibly Rust — every `assert_*`
variant is expressible as pure Incan on top of `fail()`.

### No ecosystem path for Rust-backed Incan libraries

Users cannot create their own Rust-backed Incan libraries without modifying the compiler. Today, `@std.builtin` is
reserved for stdlib sources, and there is no mechanism for a third-party package to say "my Incan module is backed by
this Rust crate." This limits the ecosystem to either pure Incan packages or raw `rust::` imports.

## Goals

- Stdlib `.incn` files become the **single source of truth** for both documentation and compilation — no more duplicated
  signatures in the typechecker.
- Adding a new stdlib function requires touching only **two files**: the Rust implementation (if `@rust.extern`) and the
  `.incn` source. The compiler requires no per-function or per-module special-casing.
- Third-party Incan libraries can wrap Rust crates using the same mechanism the stdlib uses, without compiler changes.
- The set of Rust-backed primitives is **explicitly minimized** and clearly identified.

## Non-Goals

- Replacing the `rust::` import mechanism (RFC 005). `rust::` remains the way to import Rust crate items directly into
  Incan code. `rust.module()` + `@rust.extern` provides a higher-level alternative: wrapping Rust crates with
  Incan-shaped APIs. Both mechanisms coexist.
- Automatic generation of `.incn` stubs from Rust source. This may be valuable tooling but is out of scope.
- Advanced trait features (associated types, supertraits, dispatch strategy). This RFC establishes the core trait
  mechanics — bound syntax, inference, and the Rust mapping — which cover the vast majority of use cases. Remaining
  capabilities can be addressed in targeted follow-ups as real-world usage demands them.

## Guide-level explanation (how users think about it)

### Stdlib: mostly Incan, with Rust-backed leaves

The Incan standard library is written in Incan. Most functions have real Incan implementations that the compiler compiles
through the normal pipeline. Only the functions that need runtime/OS/framework access are marked `@rust.extern` and
backed by Rust.

For example, `std.testing` looks like this:

```incan title="stdlib/testing.incn"
"""
Incan Testing Framework
"""
rust.module("incan_stdlib::testing")

# ---- Rust-backed primitive ----

@rust.extern
def fail(msg: str) -> None:
    """Explicitly fail a test. Runtime-provided."""
    ...


# ---- Pure Incan (compiled normally) ----

def assert(condition: bool) -> None:
    """Assert condition is true."""
    if not condition:
        fail("assertion failed")

def assert_eq[T](left: T, right: T) -> None:
    """Assert two values are equal."""
    if left != right:
        fail(f"assertion failed: left != right\n  left:  {left}\n  right: {right}")

def assert_ne[T](left: T, right: T) -> None:
    """Assert two values are not equal."""
    if left == right:
        fail(f"assertion failed: left == right\n  both:  {left}")

def assert_true(condition: bool) -> None:
    """Assert condition is true."""
    assert(condition)

def assert_false(condition: bool) -> None:
    """Assert condition is false."""
    assert(not condition)

def assert_is_some[T](option: Option[T], msg: str = "") -> T:
    """Assert Option is Some and return the value."""
    match option:
        Some(value) => return value
        None => fail(if msg != "": msg else "expected Some, got None")

def assert_is_none[T](option: Option[T], msg: str = "") -> None:
    """Assert Option is None."""
    match option:
        Some(_) => fail(if msg != "": msg else "expected None, got Some")
        None => pass

def assert_is_ok[T, E](result: Result[T, E], msg: str = "") -> T:
    """Assert Result is Ok and return the value."""
    match result:
        Ok(value) => return value
        Err(e) => fail(if msg != "": msg else f"expected Ok, got Err({e})")

def assert_is_err[T, E](result: Result[T, E], msg: str = "") -> E:
    """Assert Result is Err and return the error."""
    match result:
        Ok(_) => fail(if msg != "": msg else "expected Err, got Ok")
        Err(e) => return e
```

One `@rust.extern` leaf, many pure Incan functions. Users import and use them exactly as before — the change is internal
to how the compiler processes the stdlib.

### `rust.module()`: declaring a Rust-backed module

The `rust.module()` directive appears at the top of a `.incn` file and tells the compiler where `@rust.extern` items
are backed:

```incan
# stdlib/testing.incn
rust.module("incan_stdlib::testing")

@rust.extern
def fail(msg: str) -> None: ...

# ... pure Incan functions ...
```

When the compiler encounters `@rust.extern def fail(...)` in a module with `rust.module("incan_stdlib::testing")`, it
emits a reference to `incan_stdlib::testing::fail` in the generated Rust code.

### Third-party Rust-backed libraries

The same mechanism works outside the stdlib. A library author can ship an Incan package backed by a Rust crate:

```bash
my_cache_lib/
├── Cargo.toml              # Rust crate with the implementation
├── src/
│   └── lib.rs              # pub fn get(key: &str) -> Option<String> { ... }
└── stubs/
    └── cache.incn          # Incan-shaped contract
```

```incan title="stubs/cache.incn"
rust.module("my_cache_lib")

@rust.extern
def get(key: str) -> Option[str]: ...

@rust.extern
def set(key: str, value: str, ttl: int = 0) -> None: ...

# Pure Incan convenience functions built on the primitives

def get_or_default(key: str, default: str) -> str:
    """Get a value from the cache, returning a default if not found."""
    match get(key):
        Some(value) => return value
        None => return default

def get_or_set(key: str, default: str, ttl: int = 0) -> str:
    """Get a value from the cache, setting it to default if not found."""
    match get(key):
        Some(value) => return value
        None:
            set(key, default, ttl)
            return default
```

Users consume it like any Incan module:

```incan title="my_app.incn"
from my_cache.cache import get_or_default

def load_config(key: str) -> str:
    return get_or_default(key, "default_value")
```

### Trait bounds on generics

When generic Incan functions are compiled to Rust, the compiler infers trait bounds from usage. For example,
`assert_eq[T]` uses `!=` and f-string interpolation on `T`, so the compiler emits
`fn assert_eq<T: PartialEq + std::fmt::Display>(...)`. Authors can also annotate bounds explicitly using `with`:

```incan
def assert_eq[T with (Eq, Display)](left: T, right: T) -> None:
    ...
```

See [Section 5](#5-trait-bound-inference-and-annotation-normative) for the full inference rules and annotation syntax.

## Reference-level explanation (precise rules)

### 1) Compilation rules (normative)

Any `.incn` file — whether it belongs to the standard library, a third-party library, or an application — can mix pure
Incan code with Rust-backed functions. These rules apply uniformly:

- The compiler parses and compiles `.incn` files through the normal pipeline (parser → typechecker → lowering →
  emission) regardless of where they live.
- Function signatures and type definitions in `.incn` files are the **authoritative type information** for the
  typechecker. The compiler must not maintain separate, hardcoded signature registries.
- Functions with `@rust.extern` have `...` bodies; the compiler emits a call to the corresponding Rust implementation
  (resolved via `rust.module()`) rather than compiling the body.
- A function **with** `@rust.extern` that also has a non-trivial body (anything other than `...` or `pass`) is a
  **compile error**: *"`@rust.extern` function must have a `...` body — the implementation is provided by Rust."*
- The Incan signature of a `@rust.extern` function is a **contract**: its parameter types and return type must correspond
  to the Rust function's signature after Incan-to-Rust type mapping (e.g. `str` → `String`, `int` → `i64`,
  `List[T]` → `Vec<T>`). The compiler does not validate this — mismatches are caught downstream by `rustc`, and Incan
  wraps the resulting error with a diagnostic pointing to the `@rust.extern` item and its `rust.module()` directive
  (see [Diagnostics](#diagnostics)).
- Models, classes, enums, and traits are compiled through the normal pipeline. Only individual functions/methods marked
  `@rust.extern` receive special treatment.

> **RFC 022 supersession**: RFC 022 introduced `@std.builtin` as a stdlib-only decorator. This RFC replaces it with
> `@rust.extern`, which serves the same purpose (marking a function whose body is provided by Rust) but is not
> restricted to stdlib modules. The `in_stdlib_module()` check in the typechecker that currently gates `@std.builtin`
> should be removed. The `DecoratorId::StdBuiltin` variant should be renamed to `DecoratorId::RustExtern` with
> canonical spelling `"rust.extern"`.

### 2) `rust.module()` directive (normative)

#### Syntax

```ebnf
rust_module_directive = "rust" "." "module" "(" STRING_LITERAL ")" ;
```

Where `<rust_path>` is a string literal containing a valid Rust module path using `::` separators
(e.g., `"incan_stdlib::testing"`, `"my_crate::sub::module"`). The compiler stores it as an opaque string and emits it
verbatim in generated `use` statements.

`rust.module()` is a **module-level directive** — a bare statement that appears at the top of a `.incn` file (before
any declarations). It is not a decorator: decorators in Incan modify the declaration that immediately follows them,
while `rust.module()` is a standalone statement about the module itself. (The decorator form `@rust.module("...")` is
reserved for future use when `module` is introduced as a keyword — e.g., `@rust.module("...") module foo:`.)

#### Semantics

- `rust.module("path::to::module")` declares that `@rust.extern` items in this Incan module are backed by Rust
  functions at `path::to::module::<item_name>`.
- The Rust module path is treated as opaque — the compiler does not validate it against Rust source. Mismatches are
  caught downstream by `rustc`, and Incan wraps the error with a diagnostic (see [Diagnostics](#diagnostics)).

#### Placement rules

- **Exactly one per file**: a file with `@rust.extern` items must have exactly one `rust.module()` directive. A file
  without `@rust.extern` items may omit it. Multiple directives in the same file are a hard error.
- **No inheritance**: `rust.module("incan_stdlib::web")` in `std.web` does not apply to `std.web.response`. Each
  module with `@rust.extern` items needs its own directive.
- **One Rust module per Incan module**: if you need `@rust.extern` bindings to *different* Rust modules, split them
  into separate `.incn` files.

**Example** — *non-propagation across submodules*:

```incan title="stdlib/web/app.incn"
rust.module("incan_stdlib::web")

@rust.extern
def run_server(app: App) -> None: ...     # OK — directive present
```

```incan title="stdlib/web/response.incn"
# ERROR: contains @rust.extern but has no rust.module() directive
@rust.extern
def json_response(data: str) -> Response: ...

# Fix: add rust.module("incan_stdlib::web::response") to the top of this file
```

#### Resolution & validation

The `rust.module()` path must resolve to one of:

- The **standard library runtime crate** (`incan_stdlib::*`), added to the generated `Cargo.toml` when any `std.*`
  module is imported.
- A **crate declared in the package's manifest** (`incan.toml` `[dependencies]`, per RFC 013).

If the path references an undeclared crate, the compiler emits an error explaining the crate must be declared as a
dependency. This ensures paths are auditable and Cargo dependency generation is deterministic.

**Path sanitization (security)**: the `rust.module()` path is emitted verbatim into generated Rust `use` statements. To
prevent code injection, the compiler must validate that the path is a well-formed Rust module path — only identifier
segments (`[a-zA-Z_][a-zA-Z0-9_]*`) separated by `::`. Paths containing semicolons, quotes, whitespace, parentheses,
or any other characters are rejected with a diagnostic. This validation is low-cost and eliminates the possibility of
crafted path strings breaking out of a `use` statement in the generated Rust output.

#### `@rust.extern` item-kind restriction

`@rust.extern` is allowed on **free functions** and **trait default methods**. It is **not allowed** on instance methods
(`def method(self, ...)`).

The reason: `rust.module("path")` + `@rust.extern` maps to `path::<item_name>` — unambiguous for free functions, but
for instance methods it would require a naming convention like `path::TypeName__method_name` that couples Incan type
names to Rust function names. Instead, types that need Rust-backed behavior should delegate to free-function primitives:

```incan
@rust.extern
def run_server(app: App, host: str, port: int) -> None: ...

class App:
    def run(self, host: str = "127.0.0.1", port: int = 8080) -> None:
        run_server(self, host, port)
```

### 3) Irreducible primitives (normative direction)

`@rust.extern` should be applied to the **smallest possible set of primitives** — functions whose implementations
fundamentally require Rust runtime, OS, or framework access.

|            Primitive             |          Module          |               Why it needs Rust                |
| -------------------------------- | ------------------------ | ---------------------------------------------- |
| `fail(msg)`                      | `std.testing`            | `panic!()` — process termination               |
| `print(msg)` / `println(msg)`    | (builtin)                | `println!()` — stdout I/O                      |
| `__eq__` (derived)               | `std.derives.comparison` | Compiler-generated field-by-field comparison   |
| `__lt__` (derived)               | `std.derives.comparison` | Compiler-generated field-by-field ordering     |
| `__hash__` (derived)             | `std.derives.comparison` | Compiler-generated hashing via `std::hash`     |
| `clone` (derived)                | `std.derives.copying`    | Compiler-generated deep copy                   |
| `to_json` / `from_json`          | `std.serde.json`         | Serde derives — proc macros                    |
| `math.*` functions               | `std.math`               | Rust `f64` methods (`.sqrt()`, `.sin()`, etc.) |
| `run_server(app)`                | `std.web`                | Tokio/Axum server bootstrap                    |
| `request_header(req, name)` etc. | `std.web`                | Framework extractor access                     |

Everything else — `assert_eq`, `assert_ne`, `assert_true`, `Ord.__le__`, `Eq.__ne__`, conversion traits, response
builders — should be pure Incan.

### 4) Stdlib compilation model (normative direction)

When the compiler encounters `from std.testing import assert_eq`:

1. **Resolution**: resolves `std.testing` to `stdlib/testing.incn`.
2. **Parsing**: parses the file as normal Incan source (cached after first parse within a compilation unit).
3. **Type extraction**: finds `def assert_eq[T](left: T, right: T) -> None` with a real body — registers it as a
   normal function in the typechecker's symbol table (no hardcoded `FunctionInfo` needed).
4. **Compilation**: compiles the function body through the normal pipeline. The call to `fail()` within `assert_eq`
   resolves to `@rust.extern` → `incan_stdlib::testing::fail`.
5. **Emission**: emits a normal Rust function for `assert_eq`. Only `fail()` results in a reference to `incan_stdlib`.

### 5) Trait bound inference and annotation (normative)

When an Incan generic function is compiled to Rust, the generated Rust function needs appropriate trait bounds on its
type parameters. Today this is not an issue because generic stdlib functions are routed to pre-written Rust that already
carries bounds. In the compilable stdlib model, the compiler must generate these bounds itself.

#### Inference from usage (normative; required for v0.x)

The compiler infers Rust trait bounds from operations used on generic type parameters within a function body.

Inference rules (minimum required set):

|             Incan operation             |  Inferred Rust trait bound  |
| --------------------------------------- | --------------------------- |
| `==`, `!=`                              | `PartialEq`                 |
| `<`, `<=`, `>`, `>=`                    | `PartialOrd`                |
| f-string interpolation (`f"...{x}..."`) | `std::fmt::Display`         |
| `+`                                     | `std::ops::Add<Output = T>` |
| `-`                                     | `std::ops::Sub<Output = T>` |
| `*`                                     | `std::ops::Mul<Output = T>` |
| `/`                                     | `std::ops::Div<Output = T>` |
| `%`                                     | `std::ops::Rem<Output = T>` |
| `clone()`                               | `Clone`                     |
| used as `Dict` key                      | `Eq + Hash`                 |
| used as `Set` element                   | `Eq + Hash`                 |

**Deferred operations** (require associated-type inference; out of scope for initial implementation):

|  Incan operation   |      Inferred Rust trait bound      |                     Why deferred                      |
| ------------------ | ----------------------------------- | ----------------------------------------------------- |
| `x[i]` (read)      | `std::ops::Index<I, Output = U>`    | Requires inferring index type `I` and output type `U` |
| `x[i] = v` (write) | `std::ops::IndexMut<I, Output = U>` | Same; plus mutability inference                       |
| `for elem in iter` | `IntoIterator<Item = Elem>`         | Requires propagating the associated `Item` type       |

When multiple operations are used on the same type parameter, all inferred bounds are combined (unioned) into a single
`where` clause on the generated Rust function.

Bounds are placed on the underlying generic parameter `T` even when the generated Rust passes `&T`. For comparison,
formatting, and hashing traits this works via Rust's blanket implementations (e.g., `&T: PartialEq` when
`T: PartialEq`, `&T: Display` when `T: Display`). For arithmetic traits (`Add`, `Sub`, etc.) blanket impls do **not**
exist on references — the compiler's ownership inference (section 6) must ensure that arithmetic operations receive
owned or copied values rather than references, so the bound on `T` remains sufficient.

**Example** — *compiling `assert_eq[T]`*:

```incan
def assert_eq[T](left: T, right: T) -> None:
    if left != right:
        fail(f"assertion failed: left != right\n  left:  {left}\n  right: {right}")
```

The compiler observes `!=` → `PartialEq` and `f"...{left}..."` → `Display`, emitting:

```rust
fn assert_eq<T: PartialEq + std::fmt::Display>(left: T, right: T) {
    if left != right {
        incan_stdlib::testing::fail(format!(
            "assertion failed: left != right\n  left:  {}\n  right: {}", left, right
        ));
    }
}
```

Inference must be **transitive**: if a generic function calls another generic function that requires bounds, the caller
must infer those bounds as well.

#### Explicit annotation syntax (normative)

Incan supports explicit trait bound annotations using the `with` keyword — consistent with existing trait conformance
syntax on type declarations (`model Money with Add[Money, Money]:`).

```incan
# Single bound — bare word
def identity[T with Clone](value: T) -> T: ...

# Multiple bounds — parenthesised
def assert_eq[T with (Eq, Debug)](left: T, right: T) -> None: ...

# Multiple type parameters — with on each
def convert[T with (From[U], Clone), U with Debug](value: U) -> T: ...
```

Grammar extension:

```ebnf
type_param     = IDENT [ "with" bounds ] ;
bounds         = bound | "(" bound { "," bound } ")" ;
bound          = IDENT [ "[" type_args "]" ] ;
```

Commas always separate type parameters; parentheses group multiple bounds within a single parameter's `with` clause.
The `+` operator (Rust-style) is intentionally avoided because `+` already means addition in Incan.

Semantics:

- Explicit bounds are **additive** with inferred bounds. Writing `[T with Eq]` when the body also uses f-string
  interpolation on `T` results in `T: PartialEq + std::fmt::Display`.
- Explicit bounds enable the **Incan typechecker** to validate callers at the Incan level, rather than deferring all
  trait-bound errors to `rustc`.
- Incan trait names map to Rust trait bounds deterministically:

  | Incan trait   | Rust trait bound              |
  | ------------- | ----------------------------- |
  | `Eq`          | `PartialEq`                   |
  | `Ord`         | `PartialOrd`                  |
  | `Hash`        | `Hash`                        |
  | `Clone`       | `Clone`                       |
  | `Debug`       | `std::fmt::Debug`             |
  | `Display`     | `std::fmt::Display`           |
  | `Serialize`   | `serde::Serialize`            |
  | `Deserialize` | `serde::de::DeserializeOwned` |

### 6) Implicit ownership and borrowing (normative principle)

Incan handles ownership and borrowing **implicitly**. The compiler analyzes code to determine the most efficient
ownership strategy for the generated Rust. Users do not annotate ownership, borrowing, or lifetimes.

This principle is already partially implemented (the compiler infers `&self` vs `&mut self` for method receivers and
auto-borrows strings to `&str`). The compilable stdlib extends it to all compiled Incan code.

Compiler strategy:

- **Read-only parameters**: borrowed references (`&T`).
- **Mutated parameters**: mutable borrows (`&mut T`).
- **Consumed parameters** (stored, returned, or moved): owned values (`T`).
- **Primitives** (`int`, `float`, `bool`): always copied (`Copy` in Rust).
- **Return values**: always owned.
- **Ambiguous cases**: fall back to **cloning**. Cloning is always safe; it trades potential performance for simplicity.

Two invariants constrain the inference:

- **No observable semantic change**: emitted decisions must not change user-visible behavior.
- **Clone implies `Clone` bound**: if the inference clones a value of generic type `T`, the generated Rust needs
  `T: Clone`. Ownership inference feeds into trait-bound inference — they are not independent systems.

**Borrow safety across awaits**: the inference must never emit borrows held across `await` points (borrows across
suspension points prevent `Send`, breaking Tokio). When in doubt, clone.

Performance-critical code that requires hand-tuned ownership can use `rust::` imports (RFC 005) as an escape hatch.

### 7) Build output for compiled stdlib (normative direction)

- **Option A (recommended for v0.x): compile from source each time.** The compiler compiles stdlib `.incn` on every
  build, emitting the generated Rust alongside user code. Simple, always fresh, cacheable via incremental compilation.
- **Option B (future optimization): pre-compiled stdlib.** Stdlib is compiled during the Incan toolchain's own build
  process and baked into `incan_stdlib`. Reduces per-project compile times but requires version-locking.

## Design details

### Interaction with existing features

#### RFC 005 (Rust interop)

`rust::` imports (RFC 005) let **end users** import Rust crate items directly — Rust-shaped API. `rust.module()` lets
a module declare that its `@rust.extern` items are backed by Rust — Incan-shaped API wrapping Rust. These compose
cleanly: a library's `.incn` source might use `rust::` imports internally while also declaring `rust.module()`.

#### RFC 022 (stdlib namespacing & compiler→stdlib handoff)

This RFC is a natural sequel to RFC 022. RFC 022's `StdlibModuleInfo` registry becomes a transitional fallback: the
`stub_path` field is still used for source discovery, but the `feature` field (for Cargo feature gating) can eventually
be derived from `rust.module()` directives + `incan.toml` configuration.

#### Traits and derives

Derived trait implementations (`@derive(Eq)`, `@derive(Hash)`, etc.) are compiler-generated and remain `@rust.extern`
at the individual method level. The trait definitions themselves (including non-`@rust.extern` methods like `__ne__`,
`__le__`, `__gt__`, `__ge__`) are compiled as pure Incan.

#### Async

Async functions follow the same rules: `@rust.extern` async functions have their bodies provided by Rust;
non-`@rust.extern` async functions are compiled normally.

### Layering implications

The Incan codebase follows a strict dependency direction:

```text
                  ┌────────────────┐
                  │   incan_core   │  (shared types & registries)
                  └────▲───────▲───┘
                       │       │
            depends on │       │ depends on
                       │       │
       ┌───────────────┴──┐  ┌─┴──────────────┐
       │ incan (compiler) │  │  incan_stdlib  │  (Rust runtime backing)
       └──────────┬───────┘  └───────▲────────┘
                  │                  │
           emits  │                  │ depends on (via Cargo.toml)
                  │                  │
             ┌────▼──────────────────┴────┐
             │     generated program      │
             └────────────────────────────┘
```

This RFC preserves and strengthens that layering:

- The **compiler** depends on `incan_core` but must **not** depend on `incan_stdlib`.
- **Generated user programs** depend on `incan_stdlib` (added to `Cargo.toml` by the compiler when `std.*` modules are
  used).
- Compiled stdlib Rust code is emitted **as part of the generated user project** — not injected back into
  `incan_stdlib`. After this RFC, `incan_stdlib` narrows to **irreducible runtime primitives** only.

### Generated Rust module naming for Incan `std.*`

Compiling `std.*` modules would naively produce `mod std { ... }`, which **shadows Rust's own `std` crate**. To avoid
this, compiled Incan `std.*` modules are emitted under a renamed root module:

- Incan `std.testing` → Rust `crate::__incan_std::testing`
- Incan `std.web.app` → Rust `crate::__incan_std::web::app`

The `__incan_std` prefix is an implementation detail — never visible to Incan users. This mapping applies only to the
generated module tree, not to `rust.module()` paths (which point to Rust crates and are emitted as-is).

### Rust-keyword module names (implementation prerequisite)

Incan module names that are Rust keywords (e.g., `std.async`) produce invalid Rust (`mod async;`). The emitter's
existing `escape_keyword()` helper must be applied consistently across **all** generated Rust output:

- **Module declarations**: `mod async;` → `mod r#async;`
- **Use-path segments**: `use crate::async::...` → `use crate::r#async::...`
- **Filenames**: use `#[path = "async.rs"]` attributes so the filesystem name stays clean
- **Generated identifiers**: type paths, `impl` blocks, etc. originating from keyword-named modules

#### Reserved keyword list (Rust edition 2021)

**Strict keywords**: `as`, `async`, `await`, `break`, `const`, `continue`, `crate`, `dyn`, `else`, `enum`, `extern`,
`false`, `fn`, `for`, `if`, `impl`, `in`, `let`, `loop`, `match`, `mod`, `move`, `mut`, `pub`, `ref`, `return`,
`self`, `Self`, `static`, `struct`, `super`, `trait`, `true`, `type`, `unsafe`, `use`, `where`, `while`

**Reserved for future use**: `abstract`, `become`, `box`, `do`, `final`, `macro`, `override`, `priv`, `try`, `typeof`,
`unsized`, `virtual`, `yield`

This list is tied to Rust edition 2021. If the target edition changes, this list must be updated.

> Note: **2024 edition** adds `gen` as a reserved keyword.

### Diagnostics

#### Mismatch between `.incn` declaration and Rust implementation

When `rustc` reports a type mismatch for a `@rust.extern` function call, the Incan compiler must wrap it with a
diagnostic that names the `rust.module()` path and the `@rust.extern` function, points to the `.incn` declaration, and
suggests verifying the Rust signature under the standard type mapping (RFC 005).

#### Missing `rust.module()`

```
error: `@rust.extern` function `fail` in module `std.testing` has no Rust backing path.
  --> stdlib/testing.incn:5:1
   |
 5 | @rust.extern
   | ^^^^^^^^^^^^ this function's body is marked as runtime-provided
   |
   = help: add `rust.module("path::to::rust::module")` to the top of this file
```

#### Unused `rust.module()`

If a module has `rust.module()` but no `@rust.extern` items, emit a **warning**:

```
warning: `rust.module()` directive has no effect — no `@rust.extern` items found.
  --> stdlib/utils.incn:1:1
   |
 1 | rust.module("incan_stdlib::utils")
   | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ unused directive
   |
   = help: remove it if this module is pure Incan, or add `@rust.extern` to Rust-backed functions
```

#### `@rust.extern` on instance methods

```
error: `@rust.extern` is not allowed on instance methods.
  --> stdlib/web/app.incn:12:5
   |
12 | @rust.extern
   | ^^^^^^^^^^^^ instance methods cannot be runtime-provided
   |
   = help: extract a free function (e.g. `run_server(app, ...)`) and delegate to it from the method
```

#### `@rust.extern` with non-trivial body

```
error: `@rust.extern` function must have a `...` body — the implementation is provided by Rust.
  --> my_lib/stubs/cache.incn:5:1
   |
 5 | @rust.extern
   | ^^^^^^^^^^^^ this function is marked as Rust-provided
 6 | def get(key: str) -> Option[str]:
 7 |     return None
   |     ^^^^^^^^^^ but has an Incan body
   |
   = help: remove the body and use `...` instead, or remove `@rust.extern` if this is a pure Incan function
```

#### Invalid `rust.module()` path

```
error: `rust.module()` path contains invalid characters.
  --> my_lib/stubs/cache.incn:1:1
   |
 1 | rust.module("my_crate; malicious_code()")
   |             ^^^^^^^^^^^^^^^^^^^^^^^^^^^^ must be a valid Rust module path
   |
   = help: use only identifier segments separated by `::` (e.g. `"my_crate::my_module"`)
```

#### Unresolved Rust path (feature-gated crates)

If the `rust.module()` path references a crate behind a disabled Cargo feature, the build error should be wrapped with
a diagnostic pointing to the directive and suggesting the feature may not be enabled.

### Compatibility / migration

This RFC is a **non-breaking internal change** for end users — the import syntax and stdlib behavior are unchanged.

For compiler/stdlib developers, migration is:

1. Add `rust.module()` directives to stdlib `.incn` files.
2. Convert stub-only functions to real Incan implementations where possible, keeping `@rust.extern` only on irreducible
   primitives.
3. Remove hardcoded `FunctionInfo` registries from the typechecker.
4. Remove hardcoded path-rewriting logic from emission.
5. Verify behavior via existing tests (codegen snapshots, integration tests).

This can be done incrementally, one stdlib module at a time.

## Alternatives considered

### Keep everything in Rust, improve the wiring

Continue writing all stdlib implementations in Rust and invest in making the wiring less manual (e.g., auto-generating
typechecker registries from `.incn` stubs). This reduces the symptom (manual wiring) but not the cause (stdlib can't be
written in Incan). It also doesn't enable third-party Rust-backed libraries.

### py4j / PyO3-style runtime bridge

Use a runtime bridge to call between Incan and Rust at runtime. Architecturally wrong for Incan: since Incan compiles
*into* Rust, there's no separate runtime to bridge. A bridge would add serialization overhead, runtime complexity, and
GC coordination problems for no benefit.

### Auto-generate `.incn` stubs from Rust metadata

Use `rustdoc --output-format json` or similar. Potentially valuable as a *tooling* layer (`incan stubgen
rust::serde_json`) but doesn't address the core issue: the stdlib should be Incan, not generated wrappers. Could potentially
complement this RFC as a follow-up tool.

### `@rust.module` (decorator) instead of `rust.module()` (directive)

Decorators modify the declaration that immediately follows them — a file-level `@rust.module("...")` with no following
declaration breaks that contract. The bare directive form is reserved for module-level statements; the decorator form is
reserved for future use when `module` is introduced as a keyword.

## Drawbacks

- **Stdlib compile time**: compiling stdlib `.incn` on every build adds cost vs. pre-compiled `incan_stdlib`. Should be
  negligible for the stdlib's size; caching and pre-compilation (Option B) can mitigate.
- **Downstream error quality**: `@rust.extern` signature mismatches surface as `rustc` errors. Incan must wrap these
  with good diagnostics, but underlying messages may leak Rust types. Same challenge and mitigation as `rust::` imports.
- **Validation gap**: the compiler trusts `rust.module()` paths and `@rust.extern` signatures — some errors are only
  caught at the `rustc` stage. Acceptable and consistent with `rust::` imports.
- **Library authoring complexity**: creating a Rust-backed Incan library requires both Incan and Rust knowledge.
  Inherent to the use case; comparable to writing C extensions for Python.

## Acceptance checklist

### Closure definition (re-baselined)

RFC 023 is considered complete only when all three closure gates are true at the same time:

- [x] **Source-of-truth gate:** Stdlib/public Rust-backed module contracts are authored in `.incn`, and the compiler no longer depends on duplicated handwritten signature registries or ad-hoc fallback sources for those contracts.
- [x] **Dogfooding gate:** The stdlib is aggressively Incan-first; behavior that can reasonably be expressed in current Incan semantics is implemented in `.incn` and compiled through the normal pipeline.
- [x] **Runtime-bridge gate:** Remaining Rust runtime code is explicit and narrow (`rust.module()` + `@rust.extern` leaves only) and retained only where the boundary is genuinely irreducible at the current language/runtime stage.

Re-baselining note: this closure definition intentionally avoids smuggling unrelated later-RFC scope into RFC 023. Follow-up RFCs can extend semantics, but RFC 023 closes on clean boundaries and single-source ownership.

### Spec / semantics

- [x] Stdlib `.incn` files are compilable Incan source, not documentation-only stubs.
- [x] `rust.module("path::to::module")` directive is specified with clear syntax and semantics.
- [x] `@rust.extern` supersedes RFC 022's `@std.builtin` — same semantics, no source-category restriction.
- [x] `DecoratorId::StdBuiltin` renamed to `DecoratorId::RustExtern` with canonical spelling `"rust.extern"`.
- [x] `@rust.extern` items require a `rust.module()` directive on their containing module.
- [x] `@rust.extern` restricted to free functions and trait default methods; rejected on instance methods.
- [x] The irreducible primitives principle is documented: `@rust.extern` should be minimized.
- [x] Compiled Incan `std.*` modules emitted under `__incan_std` root to avoid Rust `std` shadowing.

### Compilation pipeline

- [x] Compiler parses and compiles stdlib `.incn` files through the normal pipeline.
- [x] Typechecker derives function signatures from parsed `.incn` AST, not hardcoded registries.
- [x] Emitter uses `rust.module()` path for `@rust.extern` items, not hardcoded path-rewriting.
- [x] Hardcoded `testing_import_function_info()` (and equivalents) are removed.
- [x] Hardcoded `if is_stdlib_testing` / `if is_stdlib_web` branches are removed from emission.
- [x] `escape_keyword` applied to module declarations, `use`-path segments, and filenames.

### Trait bound inference and annotation

- [x] Emitter infers Rust trait bounds from operations on generic type parameters (minimum set per table above).
- [x] Inferred bounds are combined and emitted as `where` clause / inline bounds on generated Rust functions.
- [x] Inference is transitive: calling a generic function that requires bounds propagates those bounds to the caller.
- [x] Parser/AST supports explicit trait bound syntax (`[T with (Eq, Debug)]`).
- [x] Explicit bounds are additive with inferred bounds.
- [x] Trait names in bounds are resolved through normal import/scoping and mapped to Rust trait bounds.

### `rust.module()` directive

- [x] Parser/AST supports `rust.module("...")` as a module-level directive.
- [x] Path validated: must reference `incan_stdlib` or a declared dependency in `incan.toml`.
- [x] `@rust.extern` items without a resolvable Rust backing path produce a clear error.
- [x] No propagation to nested submodules.
- [x] Duplicate `rust.module()` in the same module is a hard error.
- [x] `rust.module()` path sanitized: only valid Rust identifier segments separated by `::` are accepted.

### Stdlib migration

- [x] `std.testing` converted to compilable Incan with `fail()` as sole `@rust.extern` primitive.
- [x] `std.derives.*`: non-`@rust.extern` methods compiled from Incan source.
- [x] `std.web` response builders: pure Incan where possible, `@rust.extern` for framework I/O.
- [x] All stdlib `.incn` files with `@rust.extern` carry `rust.module()` directives.
- [x] `std.async.*` behavior is runtime-backed via narrow `@rust.extern` leaves (no broad `fail_t` placeholder surface).
    > Note: remaining closeout is concentrated in the wrapper-style async modules (`std.async.task`, `std.async.sync`, `std.async.channel`, and `std.async.prelude`); `std.async.time` and `std.async.select` are already direct-interoperability modules.
- [x] `StdlibModuleInfo` fallback mapping removed (or marked deprecated).

### Diagnostics checklist

- [x] Missing `rust.module()` → error with suggestion.
- [x] `@rust.extern` with non-trivial body → error suggesting `...` body or removing the decorator.
- [x] `@rust.extern` on instance method → error suggesting free-function extraction.
- [x] `@rust.extern` signature mismatches → wrapped `rustc` diagnostic pointing to `.incn` declaration.
- [x] Unused `rust.module()` (no `@rust.extern` items) → warning.
- [x] Invalid `rust.module()` path (failed sanitization) → error with valid-path hint.

### Tests

- [x] Typechecker tests: stdlib signatures resolved from `.incn` source.
- [x] Codegen snapshot tests: compiled Incan stdlib functions in generated output.
- [x] Codegen snapshot tests: generic functions emit correct Rust trait bounds (inferred and explicit).
- [x] Integration tests: behavioral equivalence with pre-migration stdlib.
- [x] Negative tests: calling a bounded generic function with a non-conforming type → Incan-level error.
- [x] Negative tests: `@rust.extern` with non-trivial body, invalid `rust.module()` path, unused `rust.module()` warning.
- [x] Transitive inference test: `foo[T]` calling `assert_eq[T]` acquires `PartialEq + Display` bounds from callee.
