# Imports and modules

This page explains the mental model behind Incan modules and imports.

For details and tasks, use the split pages:

- How-to: [Imports and modules (how-to)](../how-to/imports_and_modules.md)
- Reference: [Imports and modules (reference)](../reference/imports_and_modules.md)

## Key ideas

- Incan supports two import styles (Python-style and Rust-style) that can be mixed freely.
- Modules are discovered from the filesystem — there is no explicit `mod` declaration step.
- Directories can act as modules; use `mod.incn` as the “main file” for a directory module when needed.
- Common types and functions are available without imports via the prelude (see the reference page for the full list).
- The **standard library** lives under the `std` namespace (e.g. `from std.web import route`). The compiler activates features automatically based on which `std.*` modules you import.
- Some language keywords are **import-activated** (soft keywords), for example `async` / `await` after importing
  `std.async` (details in the reference page).
- Modules may also export **shared runtime state** via `pub static`, which importing modules access as the same live storage cell rather than as a copied value.

<!-- TODO: Add a link to the standard library sections once we create them -->

!!! info "Coming from Python?"
    In Python, packages are driven by directory structure and `__init__.py`.  
    In Incan, directories are recognized automatically; you can use `mod.incn` when you need a directory/module
    entrypoint.

## How module discovery works (conceptual)

When you import a local module, the compiler:

1. Resolves the path (handling `.`, `..`, `super`, `crate`)
2. Finds the `.incn` file (or `mod.incn` for directories)
3. Parses and type-checks that file
4. Makes its types and functions available in the importing file

### Coming from Rust

In Rust, you typically need explicit module declarations before importing. In Incan, you generally “just import” and the
compiler discovers modules automatically.

### Coming from Python

In Python, packages are driven by directory structure and `__init__.py`. In Incan, directories are recognized as modules
without `__init__.py`; use `mod.incn` when you need a directory entrypoint.

## Where to go next

- Practical multi-file project layouts: [Imports and modules (how-to)](../how-to/imports_and_modules.md)
- Practical shared module state: [Module state (how-to)](../how-to/module_state.md)
- Full import syntax, path rules, prelude contents: [Imports and modules (reference)](../reference/imports_and_modules.md)
- Exact `static` / `pub static` rules: [Static storage (reference)](../reference/static_storage.md)
