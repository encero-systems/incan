# Reference

The Reference section specifies the Incan Programming Language: its grammar, types, semantics, standard library APIs, and runtime behavior. Each page states the contract of one surface: its forms, rules, types, signatures, defaults, and refusals with their codes.

For step-by-step learning and patterns, see [Tutorials](../tutorials/book/index.md) and the [How-to guides](../index.md). For practical examples, see the repo’s `examples/` directory.

## Table of contents

- [Language reference (generated)](language.md): compiler-generated tables (keywords, operators, builtins, etc.)
- [Feature inventory (generated)](feature_inventory.md): generated capability atlas for syntax, stdlib, interop, testing, async, tooling, and library surfaces
- [Code style guide](code_style.md): layout, spacing, and readability rules for `.incn` source
- [Assignments](assignments.md): bindings, reassignment, tuple unpacking, tuple assignment, and chained assignment
- [Functions and calls](functions.md): function signatures, ordinary call binding, rest parameters, and call-site unpacking
- [Computed properties](computed_properties.md): field-like derived members, trait requirements, and `property` vs `def`
- [Newtypes](newtypes.md): nominal wrappers, validated construction, implicit coercion sites, and constraints
- [Generators](generators.md): `Generator[T]`, `yield`, generator expressions, and generator methods
- [Symbol aliases](symbol_aliases.md): top-level aliases, same-type method aliases, exports, and diagnostics
- [Callable presets](callable_presets.md): RHS partial declarations, projected defaults, method presets, and diagnostics
- [Conditional compilation](conditional_compilation.md): package-owned feature predicates and compile-time source projection
- [Glossary](glossary.md): the terms the docs use
- [File I/O](file_io.md): the `std.fs` file and path surface
- [Imports and modules](imports_and_modules.md): import syntax, module paths, and module resolution rules
- [Static storage](static_storage.md): `static`, `pub static`, initialization rules, and live shared module state
- [Frozen collections](frozen_collections.md): the `FrozenList`, `FrozenSet` and `FrozenDict` types a `const` holds, and their reads
- [Match patterns](match_patterns.md): the patterns of `match`, `if let`, and `while let`, and when the arms of a `match` cover its subject
- [Project lifecycle](project_lifecycle.md): project roots, `loaf.toml` metadata, version bumps, and named environments
- [Reflection](reflection.md): reflection helpers of models and classes
- [std.testing](stdlib/testing.md): assertions, markers, fixtures, and parametrization
- [Standard library reference](stdlib/index.md): signatures for `std.*` modules (`std.math`, `std.async`, `std.collections`, ...)
- [Numeric semantics](numeric_semantics.md): numeric operators, promotion rules, and edge cases
- [Strings](strings.md): string types, formatting, and string operations
- [Union types](union_types.md): anonymous closed unions, `A | B`, narrowing, and `match` type patterns
- [Derives & traits](derives_and_traits.md): derives, trait authoring, method decorators, and generic instance methods
- [Stdlib traits](stdlib_traits/index.md): the standard library's protocol traits (iteration, indexing, operators, conversions, and others)
- Derives:
    - [String representation](derives/string_representation.md): `Debug`, `Display`
    - [Comparison](derives/comparison.md): `Eq`, `Ord`, `Hash`
    - [Copying/default](derives/copying_default.md): `Clone`, `Copy`, `Default`
    - [Serialization](derives/serialization.md): `json`, `Serialize`, `Deserialize`
    - [Validation](derives/validation.md): `Validate`
    - [Custom behavior](derives/custom_behavior.md): overriding derived behavior
