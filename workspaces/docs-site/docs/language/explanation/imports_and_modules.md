# Imports and modules

This page explains the mental model behind Incan modules and imports.

For details and tasks, use the split pages:

- How-to: [Imports and modules (how-to)](../how-to/imports_and_modules.md)
- Reference: [Imports and modules (reference)](../reference/imports_and_modules.md)

## Key ideas

- Incan supports two import styles (Python-style and Rust-style) that can be mixed freely.
- Modules are discovered from the filesystem — there is no explicit `mod` declaration step.
- Directories can act as modules; use `mod.incn` as the “main file” for a directory module when needed.
- Builtin types and functions are available without imports (listed in the [language reference](../reference/language.md)).
- The **standard library** lives under the `std` namespace (e.g. `from std.web import route`). The compiler activates features automatically based on which `std.*` modules you import.
- Some language keywords are **import-activated** (soft keywords), for example `async` / `await` after importing `std.async` (details in the reference page).
- Modules may also export **shared runtime state** via `pub static`, which importing modules access as the same live storage cell rather than as a copied value.
- Published libraries preserve this same filesystem hierarchy below `pub::package`. Source directories become package namespaces automatically, while declaration visibility still comes from `pub`.

<!-- TODO: Add a link to the standard library sections once we create them -->

!!! info "Coming from Python?"
    In Python, packages are driven by directory structure and `__init__.py`. In Incan, directories are recognized automatically; you can use `mod.incn` when you need a directory/module
    entrypoint.

## How module discovery works (conceptual)

When you import a local module, the compiler:

1. Resolves the path (handling `.`, `..`, `super`, `crate`)
2. Finds the `.incn` file (or `mod.incn` for directories)
3. Parses and type-checks that file
4. Makes its types and functions available in the importing file

### Coming from Rust

In Rust, you typically need explicit module declarations before importing. In Incan, you generally “just import” and the compiler discovers modules automatically.

### Coming from Python

In Python, packages are driven by directory structure and `__init__.py`. In Incan, directories are recognized as modules without `__init__.py`; use `mod.incn` when you need a directory entrypoint.

A `rust::` import and a project module import are different kinds of dependency. `rust::serde_json` declares a dependency on a Rust crate, which `incan lock` resolves and records; `from db.schema import Database` names a module of the same package, so it adds no dependency.

In generated Rust, an import of a project module names the module that source resolution loaded by its path from the crate root, such as `crate::db::schema::Database` for `from db.schema import Database`, whichever module of the project writes the import.

## Source hierarchy and package facades

A library has two complementary public shapes. Its checked source hierarchy gives consumers stable, scalable namespaces such as `pub::hees_ai.hyperquant`. Its `src/lib.incn` can additionally re-export selected declarations as a small flat facade. Preserving both avoids forcing large packages into one enormous root namespace without breaking existing facade imports.

Directory namespaces expose public declarations from their immediate source units. Deeper directories remain nested. When siblings use the same natural declaration name, the parent spelling is intentionally ambiguous and consumers select the child module. The source tree therefore reduces naming pressure without inventing a separate module registry.

The checked `.incnlib` API graph is the authority for declarations and visibility at the package boundary. Compiler resolution and editor discovery project that graph instead of independently rediscovering the producer's source, while codegraph import records retain the canonical nested package path written by the consumer. Generated Rust uses the checked namespace projection and compiled module set to reproduce the physical module and facade layout.

## Where to go next

- Practical multi-file project layouts: [Imports and modules (how-to)](../how-to/imports_and_modules.md)
- Practical shared module state: [Module state (how-to)](../how-to/module_state.md)
- Full import syntax and path rules: [Imports and modules (reference)](../reference/imports_and_modules.md)
- Exact `static` / `pub static` rules: [Static storage (reference)](../reference/static_storage.md)
