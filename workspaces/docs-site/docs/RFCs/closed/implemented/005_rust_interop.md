# RFC 005: Rust Interop

- **Status**: Implemented
- **Author(s)**: Danny Meijer (@dannymeijer)
- **Issue**: #68
- **RFC PR**: —
- **Created**: 2025-12-10
- **Target version**: 0.2
- **Related**:
    - [RFC 013] (Rust crate dependencies)
    - [RFC 020] (Cargo offline/locked policy)

## Summary

Define an ergonomic, **explicit** Rust interop surface for Incan.

This RFC tightens the contract so we avoid “it looks like Rust” leakage and set correct expectations:

- `rust::...` imports map predictably to Rust `use` paths
- core type mapping is deterministic (e.g. `int` is always `i64`)
- common ownership/borrow friction (especially `str` vs `&str`) is handled without Rust syntax in user code
- limitations are stated up front (interop is powerful but not “everything in crates.io just works”)

Dependency pinning and lockfiles are specified by [RFC 013].  
Cargo policy enforcement (`--offline/--locked/--frozen`) and generated-project persistence are specified by [RFC 020].

Prime directive: interop must not force users to learn Rust borrowing/lifetimes/traits at the surface level!

## Goals

- Allow importing Rust crates/modules from Incan via an explicit `rust::...` prefix.
- Generate correct Rust `use` statements with stable, auditable namespacing.
- Provide deterministic type mapping rules for core types and standard collections.
- Define a safe, Incan-shaped calling model (no lifetimes, no explicit `&` in Incan code).
- State explicit limitations and diagnostics expectations so the feature is predictable and debuggable.

## Non-Goals

- A promise that “any Rust crate works”. Interop is scoped; outside that scope, failures are expected but must be
  diagnosable.
- Exposing Rust surface syntax in Incan (`&`, `&mut`, lifetimes, turbofish `::<T>`).
- Arbitrary proc-macros. (Incan may support a curated derive surface via `@derive`; see below.)
- Calling `unsafe` Rust items without an explicit Incan opt-in (out of scope for this RFC).

## Guide-level explanation (how users think about it)

### Imports: crate segment vs module path

`rust::` imports always start with a **crate segment**:

- `rust::<crate_name>` identifies the Rust crate
- any following `::...` segments are the Rust module path **inside** that crate

Examples:

```incan
# External crate (serde_json), item at crate root
from rust::serde_json import from_str, to_string

# Rust standard library (no Cargo dependency)
from rust::std::time import Instant

# External crate, deeper module path
from rust::chrono::naive::date import NaiveDate
```

### Rust standard library root (`rust::std`)

Incan uses a single, canonical spelling for Rust’s standard library:

- `rust::std::...` refers to Rust’s standard library and never produces a Cargo dependency.

Note:

- `std::...` (without `rust::`) refers to Incan’s standard library modules.

Reserved (out of scope for this RFC):

- `rust::core::...` and `rust::alloc::...` are not supported yet (future `no_std` / target work).

### The “no Rust syntax” rule

Interop should feel like Incan, not Rust:

- user code does **not** write `&value` or lifetimes
- common `str`/`&str` friction is handled by the compiler when calling external Rust functions

## Reference-level explanation (precise rules)

### Import syntax (normative)

Rust imports use the existing `import` / `from ... import ...` forms, with a `rust::` prefix:

```incan
import rust::CRATE[::PATH...] [as ALIAS]
from rust::CRATE[::PATH...] import ITEM[, ITEM2 ...]
```

> Note: `::` notation is the canonical style for Rust interop imports.
> Dot-notation (e.g. `from rust.serde_json import ...`) is accepted but emits a non-fatal **compiler warning**
> suggesting the correct `::` form. The import still resolves correctly. The LSP surfaces this as a yellow squiggle.

AST mapping (informative; matches current parser structure):

- `CRATE` maps to `ImportKind::{RustCrate|RustFrom}.crate_name`
- `PATH...` maps to `ImportKind::{RustCrate|RustFrom}.path`

### Crate name and path decomposition (normative)

- The **first** identifier after `rust::` is the crate segment (`crate_name`).
- All following `::`-separated identifiers are the module path within that crate.

Example decomposition: `from rust::chrono::naive::date import NaiveDate`

- `crate_name = "chrono"`
- `path = ["naive", "date"]`
- `items = ["NaiveDate"]`

### Rust standard library root (`rust::std`) (normative)

If `crate_name` is `std`, then:

- the import maps to a Rust path rooted at that crate (e.g. `std::time::Instant`)
- **no Cargo dependency is added** for that crate

Reserved (out of scope for this RFC):

- If `crate_name` is `core` or `alloc`, the compiler must emit a compile-time error instructing the user to use
  `rust::std::...` instead (or wait for future `no_std` / target support).

### Crate naming limitations (normative)

Incan spells the `crate_name` segment as a Rust identifier (letters/digits/underscore).

Rules:

- the crate segment must be a valid identifier (`[A-Za-z_][A-Za-z0-9_]*`)
- hyphenated crates are spelled with underscores in `rust::` imports (Rust identifier form)

```incan
# crates.io package: wasm-bindgen
# Rust crate identifier: wasm_bindgen
from rust::wasm_bindgen import prelude
```

Note:

- Cargo/crates.io normalize `-` and `_` in crate names, so `wasm-bindgen` is correctly resolved when referenced as
  `wasm_bindgen` in generated Rust code and dependency keys.
- The generated Cargo dependency key uses the exact `crate_name` spelling from the `rust::` import (the underscore/Rust-
  identifier form).
- Explicit package↔crate mapping is only needed for non-trivial mismatches (e.g. `package = "..."` with a different crate
  name), and should live in RFC 013’s `incan.toml` dependency specification rather than in the `rust::` import syntax.

### Type mapping (normative)

Interop uses deterministic core type mapping:

- `bool` → `bool`
- `int` → `i64`
- `float` → `f64`
- `str` → `String`
- `List[T]` → `Vec[T]`
- `Dict[K, V]` → `std::collections::HashMap<K, V>`
- `Option[T]` → `Option<T>`
- `Result[T, E]` → `Result<T, E>`

Numeric note:

- Rust integer widths other than `i64` (e.g. `usize`, `u128`) are not implicitly mapped to `int`.
  Conversions must be explicit (e.g. via a builtin like `int(...)`) or handled by a dedicated adapter.

### Borrowing and string conversion rules (normative)

Incan does not expose Rust borrowing syntax.

To make common Rust APIs usable (especially those taking `&str`), the compiler applies:

- string literals used where an owned string is required are lowered with `.to_string()`
- when calling an **external Rust function** (imported via `rust::...`), an argument expression of Incan type `str` is
  lowered as a **borrowed string view** (`&str`) by default (implemented by borrowing the underlying `String`, e.g.
  `value.as_str()` / `&value` on the Rust side). This makes the common Rust API pattern “takes `&str`” ergonomic without
  exposing Rust syntax in user code.
- **Forcing an owned string**: if user code syntactically constructs an owned string expression via `.to_string()` (e.g.
  `value.to_string()`), the compiler must treat that argument as **owned** and pass it by value (a clone), rather than
  applying the default borrow lowering.
- This RFC does not require Rust signature inspection to choose between `&str` vs `String`. The rule is purely based on
  the Incan argument expression shape (default: borrowed view; explicit `.to_string()`: owned clone).
- if a Rust interop call fails due to a `String`/`&str` mismatch for an argument originating from an Incan `str`,
  the compiler must emit a targeted diagnostic pointing at the argument expression and suggesting either:
    - add `.to_string()` to force passing an owned `String` (clone), or
    - remove `.to_string()` / pass the value directly so the compiler can pass a borrowed view (`&str`) (default).

Scope:

- this RFC requires borrow/ownership adaptation for **strings** (the most common interop mismatch)
- general borrow inference for arbitrary Rust types is out of scope
- rust signature inspection (e.g. via rustdoc/rust-analyzer metadata) and compile‑retry ‘guessing’ strategies to auto-fix
  borrow/ownership mismatches are out of scope for this RFC.

### Calling model: methods vs associated functions (normative)

Incan uses a single dot-call syntax at the surface for both methods and associated functions.

Lowering rules:

- If the receiver is a **value**, `value.method(args...)` lowers to a Rust method call: `value.method(args...)`.
- If the receiver resolves to a **type-like identifier** (an Incan type name or an imported Rust type), then
  `Type.method(args...)` lowers to a Rust associated function call: `Type::method(args...)`.

This is why the examples below use `Instant.now()` and `Uuid.new_v4()` even though the corresponding Rust spelling is
`Instant::now()` / `Uuid::new_v4()`.

### Derives, traits, and serde (normative direction)

Many Rust APIs require trait bounds (e.g. `HashMap` keys require `Eq + Hash`; `serde_json` requires `Serialize` /
`Deserialize`).

Incan’s user-facing mechanism for this is the `@derive(...)` decorator (not Rust proc-macro syntax).

Derive identifiers are **language vocabulary**: they do not need importing and are validated against a curated registry.

Requirement:

- Incan supports a curated derive set sufficient for common interop:
    - `Debug`, `Clone`, `Eq`, `Hash`
    - `Serialize`, `Deserialize` (to make `serde_json` usable on Incan models)

This is intentionally **not** “arbitrary proc-macros”: the derive set is curated and wired into the compiler/runtime
contract.

Implementation model note (important for determinism):

- Even with a curated `@derive(...)` list, the implementation may emit Rust `#[derive(...)]` for those traits and thus
  execute Rust proc-macros at build time (e.g. serde derives).
- This is acceptable only in combination with locked/pinned dependency resolution ([RFC 013]) and reproducible/offline
  build policy controls ([RFC 020]).
- The curated derive list is part of Incan’s compatibility contract (versioned, documented, and stable-by-default).

### Panic/unwind and error policy (normative)

Rust interop compiles into a single Rust program (generated code + dependencies). This is **not** an `extern "C"` [^extern-c]
FFI (Foreign Function Interface) boundary.

[^extern-c]: `extern "C"` selects the C ABI/calling convention for interop with C. It matters because unwinding (panics)
    across a real `extern "C"` boundary is not allowed; in Incan interop we generate one Rust program, so this is a normal
    Rust-to-Rust call path, not an FFI boundary.

**Policy**:

- Rust `Result`/`Option` values map to Incan `Result`/`Option` and work with `?`/pattern matching as usual.
- Rust panics behave like panics in generated Rust code:
    - by default they terminate the program/test (panic semantics are Rust-defined)
    - implementations should ensure the error output clearly indicates “this was a Rust panic” and includes enough context
      (crate/function if available) to debug
    - catching panics and converting them into Incan runtime errors is a possible future extension, but out of scope here

### Unsafe policy (normative)

Calling `unsafe` Rust items is out of scope for this RFC.

- The compiler must not generate Rust `unsafe { ... }` on behalf of user code.
- Therefore, Rust APIs that require `unsafe` are unsupported and should produce a clear, targeted diagnostic explaining
  that “unsafe interop is out of scope” (even if the underlying trigger originates from Rust compilation).
- A future RFC may introduce an explicit `unsafe` block/marker in Incan (and an associated safety policy).

### Diagnostics expectations (normative)

When Rust interop fails at the Incan layer (before invoking Cargo), errors must be actionable and include:

- inferred `crate_name` and `path`
- the missing item name (crate/module/type/function)
- a suggestion to:
    - verify the Rust crate API path, and/or
    - add version/features in `incan.toml` (RFC 013)

## Examples

```incan
from rust::std::time import Instant

def measure() -> float:
    start = Instant.now()
    # ... work ...
    return start.elapsed().as_secs_f64()
```

```incan
from rust::uuid import Uuid

def new_id() -> str:
    return Uuid.new_v4().to_string()
```

```incan
from rust::serde_json import from_str as json_parse, Error as JsonError

@derive(Deserialize)
model UserData:
    name: str
    email: str


def parse_user_data(json_str: str) -> Result[UserData, JsonError]:
    return json_parse(json_str)?
```

## Limitations

- No arbitrary proc-macros or custom derives (only curated derives via `@derive`).
- Trait-heavy or GAT/lifetime-heavy APIs may not map cleanly.
- No explicit borrow/lifetime syntax in Incan.

Rationale (why these limits exist):

- Arbitrary proc-macros are effectively “run arbitrary Rust at build time”; they undermine determinism, portability,
  and the “Incan stays Incan” surface. A curated `@derive(...)` set keeps the interop contract explicit and reviewable.
- Trait-heavy and lifetime-heavy Rust APIs often require expressing trait bounds and borrowing/lifetimes at call
  sites; trait bound inference and explicit annotation syntax are addressed by RFC 023 (Compilable Stdlib & Rust Module
  Binding). Lifetime/borrow surface beyond strings is deferred to a future RFC.
- Incan’s goal is to remove Rust’s borrow-checker ergonomics from user code. The compiler may adapt borrows internally
  (currently scoped mainly to strings), but users should not be forced to write Rust-like lifetime/borrow annotations.

## Design decisions

1. **Non-trivial package↔crate mapping**

  Decision: keep `rust::` imports crate-identifier-only; represent non-trivial package renames in `incan.toml`
  (`package = "..."` / dependency aliasing), not in import syntax.

  Reason: this preserves a simple and deterministic language surface while aligning with RFC 013 ownership of Cargo
  dependency modeling.

2. **Borrow adaptation scope beyond strings**

  Decision: RFC 005 stays string-focused (`str` ergonomics only). No general non-string borrow inference and no Rust
  signature inspection in this RFC.

  Reason: broader adaptation requires high-complexity type/signature analysis and risks unpredictable behavior;
  string-only adaptation captures the dominant interop friction at low complexity.

  Follow-up tracking: incremental non-string ownership/borrow improvements are tracked by issue #121.

3. **Panic handling policy for Rust interop**

  Decision: preserve Rust panic semantics in RFC 005 (no catch-and-convert runtime boundary in this RFC).

  Reason: generated programs are Rust-to-Rust call paths, and preserving native panic behavior avoids hidden control-flow
  changes. Future opt-in conversion can be introduced in a dedicated RFC.

4. **Non-native target behavior (`wasm32`, etc.)**

  Decision: target-specific interop constraints are out of RFC 005 scope and must be specified in target/toolchain RFCs
  (e.g., [RFC 003] or a dedicated target-constraints RFC).

  Reason: interop validity is target-dependent (runtime availability, crate support, panic model), so policy belongs in
  the target model rather than the base Rust interop contract.

## Appendix: `crate::...` absolute module paths for Incan modules (normative)

`crate::...` is a Rust-style spelling for **Incan module paths** (project-root absolute imports). It is **not** related
to `rust::...` (Rust crate imports).

```incan
import crate::config as cfg
from crate::utils import format_date
```

Notes:

- `crate::...` is for Incan modules (project root), not for selecting a Rust crate.
- Parent navigation uses `super::...` / `..` (see [RFC 000]).

## Checklist (acceptance)

- [x] `rust::` import syntax and crate/path decomposition is fully specified and implemented
- [x] `rust::std` works without a Cargo dependency, and `rust::core` / `rust::alloc` are rejected with a clear diagnostic
- [x] Core type mapping is deterministic (`int=i64`, `float=f64`, `str=String`, collections, `Option`/`Result`)
- [x] String borrow/ownership adaptation for external calls works and is documented
- [x] Curated derive set is defined and sufficient for common crates (`HashMap`, `serde_json`)
- [x] Diagnostics for common failure modes are actionable (crate/path/item + hint toward [RFC 013] config)

--8<-- "_snippets/rfcs_refs.md"
