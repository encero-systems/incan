# RFC 031: Incan Library System — Phase 1 (Local Path Dependencies)

- **Status:** Draft
- **Created:** 2026-03-06
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 027 (incan-vocab — keyword registration API), RFC 034 (`incan.pub` registry)
- **Target version:** v0.2

## Summary

Introduce Incan library dependencies so that one Incan project can depend on another, import its types, and compile against its generated Rust crate. Phase 1 covers the minimal local-only flow: `path` dependencies, a type manifest (`.incnlib`), a library build mode, `pub::` import syntax, and Cargo wiring through ordinary path dependencies.

The core abstractions here are intended to carry forward into later phases such as git dependencies and the `incan.pub` registry, even if some concrete CLI details or artifact layout choices evolve.

## Motivation

The Incan compiler currently has no concept of external Incan packages:

- **Module resolution** (`collect_modules()`) only finds local `.incn` files on the filesystem.
- **Dependencies** only handle raw Rust crates today (under the manifest section this RFC proposes to rename to `[rust-dependencies]`) via RFC 013.
- **There is no way** for one Incan project to import types, models, or functions from another Incan project.

This blocks the entire library ecosystem. Without library support:

- Every DSL keyword a library introduces would need to be hard-wired into the compiler.
- Users cannot share reusable Incan code between projects except by copy-pasting `.incn` files.

The core insight is a **two-artifact model**: a library ships both a **type manifest** (everything the typechecker needs) and a **generated Rust crate** (the library's `.incn` source already lowered to Rust). The consumer project does not typecheck the dependency's source as part of its own module graph.

## Guide-level explanation (how users think about it)

### Library author workflow

Like Rust, where the presence of `src/lib.rs` makes a crate a library, Incan uses `src/lib.incn` as the package root for a library-capable project. No extra TOML section is required just to declare "this project can build a library".

The `src/lib.incn` file declares the package's public API using package-root re-export syntax (new — the Incan equivalent of Rust's `pub use`):

```incan
# mylib/src/lib.incn
"""My reusable Incan library."""

pub from widgets import Widget, Layout
pub from helpers import format_output
```

The `pub` modifier on `from ... import` is only valid in `lib.incn`. It declares which symbols are part of the package's public API. Imports without `pub` are internal to the library root and do not appear in the exported manifest.

The library author builds with:

```bash
incan build --lib
```

This compiles the library through the full pipeline, emits a generated Rust crate, and produces a type manifest (for example `mylib.incnlib`) from the library's checked public surface. The exact on-disk layout under `target/` is an implementation detail; what matters normatively is that the build produces both the Rust crate material and the manifest.

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

`incan build` resolves the dependency, loads the manifest, typechecks against library types, and wires the generated library crate into the consumer's generated `Cargo.toml` as a normal path dependency. For local `path` dependencies, the build tool may rebuild stale library artifacts as an implementation detail, but the consumer typechecks against the manifest rather than folding dependency source into its own module graph.

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

A library build produces two logical artifacts:

1. **Type manifest** (`.incnlib`) — a JSON file containing everything the typechecker needs: exported models, classes, functions, traits, enums, type aliases, and soft keyword declarations. Generated from the typechecker's symbol table — never hand-written.

2. **Generated Rust crate** — the library's `.incn` source lowered to Rust, ready for Cargo to compile and link against. Shipped as generated Rust source rather than a platform-specific compiled binary artifact.

The exact directory names under `target/` are intentionally not normative in this RFC. Phase 2 / RFC 034 packaging can bundle these logical artifacts into a single distributable package without changing their meaning.

### Manifest schema

```text
Manifest:
  name: str                          # Package name
  version: str                       # SemVer
  incan_version: str                 # Minimum compiler version required
  manifest_format: int               # Schema version (for forward compatibility)
  dependencies: list[Dependency]     # Other Incan libraries this depends on

  exports:
    models: list[ModelExport]        # Model definitions with fields, traits, methods
    classes: list[ClassExport]       # Class definitions with inheritance, methods
    functions: list[FunctionExport]  # Free functions with typed signatures
    traits: list[TraitExport]        # Trait definitions with required methods
    enums: list[EnumExport]          # Enum definitions with variants
    type_aliases: list[TypeAlias]    # Type aliases

  soft_keywords:
    activations: list[SoftKeywordActivation]
      # Extracted from the library's vocab crate (RFC 027), if any.
      # Never hand-authored.

  rust_crate:
    name: str                        # Rust crate name (for generated Cargo.toml)
    path: str                        # Relative path to Rust crate root
```

The manifest is serialized as JSON for Phase 1 (human-readable, debuggable, zero extra deps). `manifest_format` versioning is what protects future evolution; exact JSON field layout is not the semantic contract by itself.

### `pub::` import syntax

The parser recognises `pub::` as a library namespace prefix, parallel to the existing `rust::` prefix:

```text
import_stmt ::= "from" import_path "import" import_items
import_path ::= "pub::" IDENT          # Incan library
              | "rust::" rust_path      # Rust crate (existing)
              | module_path             # Local project module (existing)
```

Resolution: the compiler looks up the identifier after `pub::` in the loaded library manifests (populated from `[dependencies]` in `incan.toml`). If found, the imported names are resolved against the manifest's exports. If not found, a diagnostic is emitted.

### `incan.toml` changes

**Consumer project** — new `[dependencies]` section for Incan libraries:

```toml
[dependencies]
mylib = { path = "../mylib" }
```

**Library project** — no special section needed. The presence of `src/lib.incn` makes the project library-capable. `incan build --lib` checks for this file and errors if it doesn't exist.

**Rename**: existing `[dependencies]` for Rust crates becomes `[rust-dependencies]`. Incan library dependencies get the unqualified `[dependencies]` name — they will be the more common case long-term. The compiler emits a clear migration diagnostic if it detects Rust crate names in `[dependencies]`.

### Compilation flow: `incan build --lib`

```text
src/*.incn
  → Lexer → Parser → Typechecker (validate all exports)
  → Lowering → Emission → generated library Rust crate
  → Generate manifest from checked public API → <library-artifact-dir>/<name>.incnlib
  → cargo build (validate the Rust crate compiles)
```

The manifest is generated from the checked public API declared by `lib.incn`. There is no separate TOML export list.

### Compilation flow: `incan build` (consumer)

```text
incan.toml
  → Parse [dependencies]
  → For each dependency: resolve path, ensure/load the built library artifact, read <name>.incnlib
  → Load manifest exports into typechecker symbol table
  → Parse + typecheck user's .incn files
  → On `pub::` imports, activate any matching library vocab metadata from the manifest
  → Lower + emit user's Rust code (generates `use <lib>::...` references)
  → Generate Cargo.toml with library crate paths as path dependencies
  → cargo build (Cargo resolves the library crate's own Rust deps transitively)
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
mylib = { path = "../mylib/<generated-library-crate-dir>" }
```

The consumer only needs the library crate as a normal Cargo dependency. The generated library crate's own `Cargo.toml` declares any Rust dependencies it needs, and Cargo resolves those transitively in the usual way.

### Soft keyword activation

Libraries that introduce soft keywords define them via the `VocabProvider` trait (RFC 027). The library build extracts those declarations and serializes them into the manifest alongside the ordinary export surface. During consumer build, the compiler reads that metadata from the manifest and uses it with the same import-driven activation model used for other soft keywords.

In Phase 1, activation should remain **import-driven**, not project-wide. Depending on a library makes its vocabulary available for resolution, but keywords are activated by the relevant `pub::...` imports, just as stdlib soft keywords are activated by the imports that bring their modules into scope.

### Interaction with existing features

- **`rust::` imports (RFC 005)**: `pub::` and `rust::` are parallel namespace prefixes. They share the same import syntax, differing only in resolution mechanism (manifest lookup vs. Rust crate path).
- **Stdlib namespaces (RFC 022/023)**: `std.*` imports are compiler-provided and always available. `pub::*` imports are user-declared and require `[dependencies]`. They coexist without overlap.
- **Soft keywords (RFC 022)**: Library soft keywords use the same import-activated model as stdlib soft keywords.
- **Vocab crate (RFC 027)**: `incan-vocab` defines keyword-registration and desugaring metadata. The library export manifest itself remains a product of the library build, not the vocab crate's authoritative replacement.

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

## Drawbacks

- **Complexity**: The two-artifact model, manifest format, and `build --lib` flow add significant compiler complexity.
- **Breaking change**: Renaming `[dependencies]` → `[rust-dependencies]` for Rust crates will require migration for existing projects.
- **Build orchestration**: Library builds now have artifact freshness concerns. Tooling must decide when a local path dependency needs to be rebuilt.
- **No versioning**: Phase 1 path dependencies have no version resolution — if the library changes, the consumer gets whatever's on disk. Versioning comes in Phase 2 (git tags) and Phase 3 (`incan.pub` + SemVer).

## Layers affected

- **Parser** (`crates/incan_syntax/src/parser/`) — recognise `pub::` as an import path prefix (parallel to `rust::`); support `pub from ... import ...` re-export syntax valid only in `lib.incn`, requiring a visibility field on `ImportDecl`.
- **Typechecker** (`src/frontend/typechecker/`) — load manifest exports into the symbol table before checking user code so library types are indistinguishable from local types during checking; load soft keyword activations from manifests into the parser's keyword registry.
- **IR Emission** (`src/backend/ir/emit/`) — generate `use <lib>::...` statements for `pub::` imports.
- **Project generator** (`src/backend/project/`) — add library crate paths to the generated `Cargo.toml` `[dependencies]` section.
- **CLI** (`src/cli/`) — introduce `incan build --lib`; wire dependency resolution (path lookup, manifest loading, stale-artifact detection) into `build`, `run`, and `test` flows; emit the migration diagnostic when Rust crate names are found in `[dependencies]`.
- **Manifest layer** (new) — `LibraryManifest` data model, JSON serialization/deserialization, and the generation step that extracts the checked public API from the typechecker's symbol table.
- **LSP** — load library manifests on workspace open to provide completions and hover info for library types.

## Unresolved questions

1. **Generic type representation in manifests.** How does the manifest represent generic types like `Container[T]` with bounds like `T: Serializable`? The TypeRef format needs to handle generics, Optional types, Result types, and nested generics (`list[Container[T]]`).

2. **Artifact freshness policy for path deps.** Should `incan build` always rebuild local library dependencies, use timestamps/hashes, or require an explicit user command to refresh them?

3. **Export-surface ergonomics beyond `lib.incn`.** Phase 1 uses package-root re-exports in `src/lib.incn`. If Incan later wants broader visibility modifiers or more granular package APIs, should that remain sugar over the same root export model or become a broader language visibility feature?

Implementation details such as the LSP's manifest cache layering are intentionally out of scope for this RFC.

## Future phases (out of scope for this RFC)

- **Phase 2: Git dependencies + caching.** `mylib = { git = "https://...", tag = "v0.1.0" }`. Adds lockfile (`incan.lock`), `~/.incan/libs/` cache, checksum verification, transitive dependency resolution, and inline `@` version syntax.
- **Phase 3: `incan.pub` registry.** Published Incan libraries with SemVer resolution. `mylib = "0.1.0"` resolves from the registry. `incan publish` for library distribution. Defined in [RFC 034](034_incan_pub_registry.md).
