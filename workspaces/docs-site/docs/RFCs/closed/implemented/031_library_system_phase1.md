# RFC 031: Incan Library System — Phase 1 (Local Path Dependencies)

- **Status:** Implemented
- **Created:** 2026-03-06
- **Author(s):** Danny Meijer (@dannymeijer)
- **Issue:** [#165](https://github.com/dannys-code-corner/incan/issues/165)
- **RFC PR:** [#177](https://github.com/dannys-code-corner/incan/pull/177)
- **Related**:
    - RFC 027 (incan-vocab — keyword registration API)
    - RFC 034 (`incan.pub` registry)
- **Written against:** v0.1
- **Shipped in:** v0.2

## Summary

Introduce Incan library dependencies so that one Incan project can depend on another, import its types, and compile against its generated Rust crate. Phase 1 (this RFC)covers the minimal local-only flow to serve as a foundation for later phases. This includes: `path` dependencies, a type manifest (`.incnlib`), a library build mode, `pub::` import syntax, optional vocab metadata for library-provided soft keywords, and Cargo wiring through ordinary path dependencies. The core abstractions here are intended to carry forward into later phases such as git dependencies and the `incan.pub` registry, even if some concrete CLI details or artifact layout choices evolve.

RFC 034 captures the full library system, including git dependencies and the `incan.pub` registry.

## Motivation

The Incan compiler currently has no concept of external Incan packages:

- **Module resolution** only finds local `.incn` files on the filesystem.
- **Dependencies** (`[rust-dependencies]` in `incan.toml`) only handle raw Rust crates via RFC 013.
- **There is no way** for one Incan project to import types, models, or functions from another Incan project.

This blocks the entire library ecosystem. Without library support:

- Every DSL keyword a library introduces would need to be hard-wired into the compiler.
- Users cannot share reusable Incan code between projects except by copy-pasting `.incn` files.

The core insight is a **two-artifact model**: a library ships both a **type manifest** (everything the typechecker needs) and a **generated Rust crate** (the library's `.incn` source already lowered to Rust source plus crate metadata). The consumer project never reparses, re-typechecks, or re-lowers library `.incn` source as part of its own build.

## Goals

- Allow one Incan project to declare another as a dependency (via `path` reference) and import its exported types, functions, and soft keywords.
- Introduce `incan build --lib` as the canonical command for building a library artifact: a type manifest (`.incnlib`) plus a generated Rust crate.
- Allow a library project to declare optional vocab metadata for library-provided soft keywords so the consumer compiler can activate them from the built artifact rather than from source.
- Establish `pub::` as the import namespace prefix for Incan library dependencies, parallel to `rust::` for Rust crates.
- Rename `[dependencies]` in `incan.toml` to `[rust-dependencies]` for Rust crate pass-through, freeing the unqualified `[dependencies]` key for Incan library dependencies.
- Define the manifest schema and the consumer build flow (manifest loading → typechecking → Rust code emission → Cargo wiring) in sufficient normative detail for implementation.

## Non-Goals

- Git-based dependency resolution, `~/.incan/libs/` caching, lockfile entries, or `incan fetch` — those are Phase 2 concerns.
- The `incan.pub` registry, `incan publish`, or SemVer resolution — those are Phase 3 concerns addressed by RFC 034.
- Transitive Incan library dependencies. Phase 1 only covers direct local path dependencies, so recursive dependency manifests and graph resolution are out of scope.
- Namespace collision resolution beyond a clear compile error.
- Deep LSP warm-cache strategies for remote dependencies — not needed for local path deps.
- External desugarer transport or execution for library-defined DSL blocks — Phase 1 only carries declarative metadata plus soft keyword registrations.

## Guide-level explanation (how users think about it)

### Library author workflow

Like Rust, where the presence of `src/lib.rs` makes a crate a library, Incan uses `src/lib.incn` as the convention. If `src/lib.incn` exists, the project is a library. No required library-specific TOML section is needed.

The `src/lib.incn` file declares public exports using `pub` re-export syntax (new — the Incan equivalent of Rust's `pub use`):

```incan
# mylib/src/lib.incn
"""My reusable Incan library."""

pub from widgets import Widget, Layout
pub from helpers import format_output
```

The `pub` modifier on `from ... import` is only valid in `lib.incn`. It declares which symbols are part of the library's public API. Imports without `pub` are internal to the library.

If the library also exposes soft keywords (or custom DSL), it declares an optional companion vocab crate in `incan.toml`:

```toml
[vocab]
crate = "crates/mylib-vocab"
```

That crate depends on RFC 027's `incan-vocab` surface and contributes keyword registrations that `incan build --lib` serializes into the library artifact alongside the checked export manifest.

The library author builds with:

```bash
incan build --lib
```

This compiles all `.incn` source through the full pipeline, generates a Rust crate in `target/lib/`, and produces a type manifest (e.g., `mylib.incnlib`) derived from the library's checked public API.

### Consumer project workflow

A consumer declares the dependency in `incan.toml`:

```toml
# my-app/incan.toml
[project]
name = "my-app"
version = "0.1.0"

[dependencies]
mylib = { path = "../mylib" }
```

Then imports and uses library types normally:

```incan
# src/main.incn
from pub::mylib import Widget
from local_models import AppState

def build_ui(state: AppState) -> Widget:
    return Widget(title=state.name)
```

The `pub::` prefix makes the import source unambiguous:

```incan
from pub::mylib import Widget          # Incan library (from [dependencies])
from rust::tokio import spawn          # Rust crate (from [rust-dependencies])
from local_models import AppState      # Local project module
```

`incan build` resolves the dependency, loads the manifest, typechecks against library types, and wires the library's Rust crate into the generated Cargo.toml. The consumer never touches library source.

### What the user sees

```bash
$ incan build
  Resolving dependencies...
    mylib = ../mylib (path)
  Loading manifests...
    mylib 0.1.0 — 3 exports (Widget, Layout, format_output)
  Compiling my-app...
  Done.
```

## Reference-level explanation (precise rules)

### The two-artifact model

A library produces two artifacts during `incan build --lib`:

1. **Type manifest** (`.incnlib`) — a JSON file containing everything the typechecker needs: exported models, classes, functions, traits, enums, type aliases, and soft keyword declarations. Generated from the library's checked public API — never hand-written.

2. **Generated Rust crate** — the library's `.incn` source lowered to Rust source plus `Cargo.toml`, ready for `cargo` to compile and link against. The Phase 1 artifact ships generated `.rs` source, not a compiled `.rlib`, so the artifact remains target-independent and compatible with the consumer's toolchain.

```text
mylib/target/lib/
├── mylib.incnlib             # Type manifest (JSON)
├── Cargo.toml                # Rust crate metadata
└── src/                      # Generated Rust source
    ├── lib.rs
    ├── widgets.rs
    └── helpers.rs
```

### Manifest schema

```text
Manifest:
  name: str                          # Package name
  version: str                       # SemVer
  incan_version: str                 # Minimum compiler version required
  manifest_format: int               # Schema version (for forward compatibility)

  exports:
    models: list[ModelExport]        # Model definitions with fields, traits, methods
    classes: list[ClassExport]       # Class definitions with inheritance, methods
    functions: list[FunctionExport]  # Free functions with typed signatures
    traits: list[TraitExport]        # Trait definitions with required methods
    enums: list[EnumExport]          # Enum definitions with variants
    type_aliases: list[TypeAlias]    # Type aliases

  soft_keywords:
    activations: list[SoftKeywordActivation]
      # Extracted from the library's vocab crate (RFC 027) during build --lib.
      # Never hand-authored.
```

The manifest is serialized as JSON for Phase 1 because it is human-readable, debuggable, and requires no extra dependencies. Implementations should isolate the encoding behind a stable manifest read/write boundary so the on-disk format may evolve later without changing the language contract. The semantic contract is the manifest schema, not JSON as a forever format.

`TypeRef` is recursive: it must support plain named types, applied generics, optionals, results/unions, and nested combinations. Bounds belong at the declarations that introduce type parameters, not on every individual use site, so exported generic types and exported generic functions must carry their type parameters plus bound metadata alongside the recursive `TypeRef` tree.

The `.incnlib` manifest is intentionally a semantic surface artifact, not a transport or dependency-resolution artifact. It describes the checked public API plus optional vocab metadata; it does not describe where the generated Rust crate lives on disk, nor does it attempt to encode future git/registry/lockfile resolution data. For Phase 1, we mandate that the generated Rust crate name must match the library package name, and the crate's on-disk location is determined by the dependency transport (`target/lib/` for local builds, extracted package root for registry packages); the `.incnlib` file does not hold information about this (on purpose).

### `pub::` import syntax

The language recognises `pub::` as a library namespace prefix, parallel to the existing `rust::` prefix:

```text
import_stmt ::= "from" import_path "import" import_items
import_path ::= "pub::" IDENT          # Incan library
              | "rust::" rust_path      # Rust crate (existing)           |
              | module_path             # Local project module (existing) |
```

Resolution: the compiler must look up the identifier after `pub::` in the loaded library manifests (populated from `[dependencies]` in `incan.toml`). If found, the imported names are resolved against the manifest's exports. If not found, a diagnostic is emitted.

### `incan.toml` changes

**Consumer project** — new `[dependencies]` section for Incan libraries:

```toml
[dependencies]
mylib = { path = "../mylib" }
```

**Library project** — no special section needed. The presence of `src/lib.incn` makes the project a library. `incan build --lib` checks for this file and errors if it doesn't exist.

**Optional vocab companion** — libraries that expose soft keywords may declare:

```toml
[vocab]
crate = "crates/mylib-vocab"
```

That crate is built during `incan build --lib`, and its keyword registrations are serialized into the library artifact.

**Rename**: existing `[dependencies]` for Rust crates becomes `[rust-dependencies]`. Incan library dependencies get the unqualified `[dependencies]` name — they will be the more common case long-term. The compiler emits a clear migration diagnostic if it detects Rust crate names in `[dependencies]`.

### Compilation flow: `incan build --lib`

```text
src/*.incn
  → Read optional [vocab].crate and collect library soft keyword registrations
  → Lexer → Parser → Typechecker (validate all exports)
  → Lowering → Emission → Rust crate in target/lib/src/
  → Generate manifest from the checked public API → target/lib/<name>.incnlib
  → cargo build (compile the generated Rust crate and validate that it links)
```

The manifest is derived from the library's checked public API and includes every declaration exported through `lib.incn`. No separate export declaration mechanism is required.

### Compilation flow: `incan build` (consumer)

```text
incan.toml
  → Parse [dependencies]
  → For each dependency: resolve path, read <name>.incnlib
  → Make manifest exports available to semantic name resolution
  → Make soft keyword activations available to import-driven parsing
  → Parse + typecheck user's .incn files (library types already available)
  → Lower + emit user's Rust code (generates `use <lib>::...` references)
  → Generate Cargo.toml with library crate paths as path dependencies
  → cargo build (compiles and links against the generated library Rust crates without re-lowering library Incan source)
```

### Rust code emission

For a consumer file importing `from pub::mylib import Widget`:

```rust
// Generated Rust
use mylib::Widget;

fn build_ui(state: AppState) -> Widget {
    // ...
}
```

The generated `Cargo.toml`:

```toml
[dependencies]
mylib = { path = "../mylib/target/lib" }
```

The consumer only needs the generated library crate as a normal Cargo dependency. The library crate's own `Cargo.toml` declares any Rust dependencies it needs, and Cargo resolves those transitively in the usual way.

### Soft keyword activation

Libraries that introduce soft keywords define them via RFC 027's vocab surface. The compiler extracts these declarations during `incan build --lib` and serializes them into the manifest. During consumer build, keyword activations are loaded from the manifest so imports can activate the relevant soft keywords without requiring access to library source.

For Phase 1, activation remains **import-driven**, not project-wide. Depending on a library makes its vocabulary available for resolution, but the relevant `pub::...` imports are what activate the library's soft keywords, just as stdlib soft keywords are activated by the imports that bring their namespaces into scope.

### Interaction with existing features

- **`rust::` imports (RFC 005)**: `pub::` and `rust::` are parallel namespace prefixes. They share the same import syntax, differing only in resolution mechanism (manifest lookup vs. Rust crate path).
- **Stdlib namespaces (RFC 022/023)**: `std.*` imports are compiler-provided and always available. `pub::*` imports are user-declared and require `[dependencies]`. They coexist without overlap.
- **Soft keywords (RFC 022)**: Library soft keywords use the same import-activated model as stdlib soft keywords. The language must not introduce a separate library-only activation rule.
- **Vocab crate (RFC 027)**: `incan-vocab` defines the shared types (`VocabRegistration`, `DslSurface`, `DeclarationSurface`, `ClauseSurface`, lower-level keyword DTOs, and manifest metadata types) used by both library authors and the compiler. This RFC uses those types to populate the built library artifact.

### Compatibility / migration

- **Breaking**: `[dependencies]` in `incan.toml` is renamed to `[rust-dependencies]` for Rust crate deps. A migration diagnostic guides users.
- **Additive**: `pub::` import syntax, `[dependencies]` for Incan libraries, `src/lib.incn` convention, `incan build --lib` — all new.

## Alternatives considered

### Source-only (re-compile library `.incn` on every consumer build)

The consumer would re-lex, re-parse, re-typecheck, and re-lower the entire library on every build. Rejected because it's slow for large libraries and eliminates the possibility of pre-compiled distribution.

### Rust-crate-only (no manifest)

Ship only the generated Rust crate, skip the manifest. The consumer gets Rust compilation but no Incan-level type checking — the compiler wouldn't know about library types' fields, methods, or type parameters. Rejected because it defeats the purpose of Incan's type system.

### No `pub::` prefix (bare library imports)

`from mylib import Widget` without any prefix. Rejected because it's ambiguous — is `mylib` a local module or a library? The `pub::` prefix makes the source unambiguous, paralleling `rust::` for Rust crates.

### Explicit `[exports]` table in `incan.toml`

Instead of deriving exports from `pub` visibility in `lib.incn`, require an explicit list of exported symbols in `incan.toml`. Rejected because it duplicates information the typechecker already has and adds ceremony without benefit.

### Ship compiled `.rlib` artifacts as the Phase 1 contract

Package a compiled Rust library instead of generated Rust source. Rejected for Phase 1 because `.rlib` artifacts are tied to the consumer's Rust toolchain and target configuration, while generated Rust source keeps the library artifact target-independent and lets Cargo compile everything in one normal dependency graph.

## Drawbacks

- **Complexity**: The two-artifact model, manifest format, and `build --lib` flow add significant compiler complexity.
- **Breaking change**: Renaming `[dependencies]` → `[rust-dependencies]` for Rust crates will require migration for existing projects.
- **Library author friction**: Authors must run `incan build --lib` and keep artifacts up-to-date. Phase 2 (git deps + CI) will automate this.
- **No versioning**: Phase 1 path dependencies have no version resolution — if the library changes, the consumer gets whatever's on disk. Versioning comes in Phase 2 (git tags) and Phase 3 (`incan.pub` + SemVer).

## Layers affected

- **Project configuration and dependency declarations**: `incan.toml` gains a clear split between Incan library dependencies (`[dependencies]`) and Rust crate dependencies (`[rust-dependencies]`), plus an optional `[vocab]` section for library-provided keyword metadata.
- **Library surface definition**: `src/lib.incn` becomes the public entrypoint for library exports, and `pub` re-exports define the checked API surface that is serialized into the manifest.
- **Parsing and import resolution**: the language adds `pub::` as a distinct import namespace, and library-provided soft keywords follow the same import-driven activation model as existing soft keywords.
- **Semantic analysis**: consumer projects resolve imported library items from manifests rather than from library source, so exported types, functions, and keyword metadata must be represented in a typechecker-readable artifact.
- **Code generation and build orchestration**: `incan build --lib` must emit both the manifest and a generated Rust crate, while consumer builds must wire that crate into the generated Cargo dependency graph as a path dependency.
- **Tooling and editor support**: the compiler and LSP should share the same manifest schema and validation rules so imported library symbols behave consistently in builds and editor workflows.

## Implementation Plan

### Phase 1: Project model and dependency surfaces

- Finalize the Phase 1 project model where library dependencies use `[dependencies]` and Rust crates use `[rust-dependencies]`.
- Add migration diagnostics so existing projects using Rust crates under `[dependencies]` get clear, actionable guidance.
- Define the library-mode preconditions for `incan build --lib`, including `src/lib.incn` discovery and user-facing errors.

### Phase 2: Library export surface and manifest production

- Implement `pub` re-export handling in `src/lib.incn` as the checked public surface for library artifacts.
- Generate `.incnlib` manifests from checked exports, including nominal types, function signatures, generics, and bounds metadata.
- Keep manifest encoding behind a stable read/write boundary so the semantic contract remains stable as transport evolves.

### Phase 3: Consumer import resolution and semantic integration

- Add `pub::` namespace import resolution against loaded dependency manifests.
- Integrate manifest-provided symbols into semantic name resolution and typechecking without reparsing library source.
- Emit precise diagnostics for missing libraries, missing exports, and import collisions with local symbols.

### Phase 4: Build orchestration, lowering, and Cargo wiring

- Ensure `incan build --lib` emits both required artifacts: `.incnlib` and a generated Rust crate.
- Ensure consumer builds emit Rust imports that map `pub::` symbols to the generated dependency crate.
- Wire generated Cargo dependencies to library crate paths and rely on Cargo for transitive Rust dependency resolution.

### Phase 5: Vocab and soft-keyword pipeline alignment

- Integrate optional library vocab metadata extraction during `build --lib` and persist keyword activations into manifests.
- Load library keyword activations in consumer builds and apply the same import-driven activation model used by stdlib soft keywords.
- Keep this layer compatible with RFC 027 so later phases can extend transport and registry behavior without changing keyword semantics.

### Phase 6: Tooling, validation, and rollout

- Reuse the compiler's manifest parsing and validation in LSP so editor behavior matches build behavior.
- Add comprehensive tests across parser/import semantics, manifest schema IO, typechecker resolution, and codegen/Cargo integration.
- Update docs and release notes with migration guidance and Phase 1 usage patterns for library authors and consumers.

## Implementation log

### Spec / design lock

- [x] Confirm all Phase 1 scope boundaries remain aligned with RFC 034 handoff (git/registry deferred).
- [x] Confirm `pub` re-export expectations in `src/lib.incn` and document any edge-case constraints.
- [x] Confirm manifest schema v1 fields for exports, type parameters, and bounds metadata.

### Configuration and CLI behavior

- [x] Parse and validate `[dependencies]` as Incan library dependencies (path-only in Phase 1).
- [x] Parse and validate `[rust-dependencies]` as Rust crate pass-through dependencies.
- [x] Add migration diagnostics for projects still using old Rust dependency placement.
- [x] Implement `incan build --lib` precondition checks and clear failure messages.

### Parser and import resolution

- [x] Support `pub::` import path handling as a first-class namespace prefix.
- [x] Ensure import diagnostics distinguish unresolved library names vs unresolved exported symbols.
- [x] Ensure namespace collision diagnostics suggest import aliasing patterns.

### Manifest producer and consumer reader

- [x] Build manifest export entries from the checked public API rather than source reparse.
- [x] Serialize and deserialize recursive `TypeRef` trees with generic applications and wrappers.
- [x] Preserve declaration-site type-parameter bounds metadata in manifest exports.
- [x] Enforce manifest version and compiler compatibility checks on load.

### Typechecker integration

- [x] Register manifest exports in semantic lookup so imported library symbols typecheck like local declarations.
- [x] Resolve library nominal types and function signatures from manifest-backed symbols in expression contexts.
- [x] Keep local-module and library symbol resolution behavior deterministic and well-diagnosed.

### Lowering, emission, and Cargo integration

- [x] Emit generated Rust that imports library symbols from dependency crates.
- [x] Generate consumer Cargo dependency entries targeting each library's generated crate path.
- [x] Validate that consumer builds compile and link against generated library crates without re-lowering library Incan source.

### Vocab and soft keywords (RFC 027 alignment)

- [x] Build optional vocab companion metadata during `incan build --lib`.
- [x] Serialize library-provided soft-keyword activations into `.incnlib`.
- [x] Activate library soft keywords from imports in consumer builds using existing soft-keyword semantics.

### LSP and tooling parity

- [x] Reuse shared manifest parser/validator between compiler and LSP.
- [x] Ensure editor diagnostics for `pub::` imports match compiler diagnostics.
- [x] Validate completion and hover behavior for manifest-backed library symbols.

### Tests and snapshots

- [x] Add parser tests for `pub::` import forms and edge-case diagnostics.
- [x] Add typechecker tests for valid/invalid manifest-backed imports.
- [x] Add manifest IO tests for schema compatibility and bounds fidelity.
- [x] Add codegen snapshot tests covering `pub::` usage in expression positions.
- [x] Add integration tests for library build + consumer build end-to-end flow.

### Docs and release notes

- [x] Update docs-site pages for dependency configuration and library workflows.
- [x] Document migration from old Rust dependency table naming to `[rust-dependencies]`.
- [x] Add release notes entries for the library system Phase 1 user-facing changes.

## Design Decisions

1. **Manifest type representation and generic bounds.** The manifest uses a recursive `TypeRef` tree for named types, applied generics, optionals, result-like wrappers, and nested compositions. Bounds are serialized at the declarations that introduce type parameters, so exported generic types and exported generic functions carry their type parameters plus bound metadata rather than repeating bounds on every use site.

2. **Public surface completeness.** Phase 1 library exports include public nominal types and public free functions re-exported through `src/lib.incn`. Enum variants are represented as part of their exported enum definitions rather than as standalone exports, and trait implementations are not independently exportable manifest items.

3. **Namespace collision handling.** A collision between an imported library symbol and a local symbol is a compile error. Phase 1 does not add new qualified-type syntax such as `mylib.Widget`; instead, diagnostics should suggest import-site aliasing such as `from pub::mylib import Widget as LibWidget`.

4. **Library Rust dependency handling.** The consumer depends on the generated library crate, not on each Rust crate that the library happens to use internally. The library crate's own `Cargo.toml` declares its Rust dependencies, and Cargo resolves them transitively in the normal way.

5. **LSP manifest loading.** The LSP should share the same manifest parsing and validation layer as the compiler so there is a single schema implementation. LSP-specific caching may sit on top of that shared reader, but the parsing rules themselves should not fork.

6. **JSON transport for Phase 1.** `.incnlib` stays JSON in Phase 1 because it is debuggable, easy to generate, and expected to be a small fraction of total local path dependency build cost. The compiler should read each dependency artifact at most once per invocation, and the encoding may change in a later phase behind the existing reader/writer abstraction if profiling proves it worthwhile.

7. **Semantic manifest vs. transport metadata.** `.incnlib` carries checked public API and optional vocab metadata only. Dependency source/version resolution belongs to the path/git/registry layers, and filesystem layout belongs to the build/package transport layer. That keeps Phase 1's semantic contract stable even as later phases add lockfiles, caches, and registry packaging.
