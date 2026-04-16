# Reference

The Reference section is the canonical, stable specification of the Incan Programming Language. It provides precise
definitions for grammar, types, semantics, standard library APIs, and runtime behavior.

Use this section when:

- You need exact syntax or signature details
- You want behavior clarified without examples
- You’re verifying compatibility across versions

For step-by-step learning and patterns, see [Tutorials](../tutorials/book/index.md) and the
[How-to guides](../index.md). For practical examples, see the repo’s `examples/` directory.

## Table of contents

- [Language reference (generated)](language.md): compiler-generated tables (keywords, operators, builtins, etc.)
- [Glossary](glossary.md): canonical terminology used across the docs
- [Imports and modules](imports_and_modules.md): import syntax, module paths, and module resolution rules
- [Static storage](static_storage.md): `static`, `pub static`, initialization rules, and live shared module state
- [std.testing](stdlib/testing.md): assertions, markers, fixtures, and parametrization
- [Standard library reference](stdlib/index.md): signatures for `std.*` modules (`std.math`, `std.async`, `std.testing`, ...)
- [Numeric semantics](numeric_semantics.md): numeric operators, promotion rules, and edge cases
- [Strings](strings.md): string types, formatting, and string operations
- [Derives & traits](derives_and_traits.md): derives, trait authoring, method decorators, and generic instance methods
- Derives:
    - [String representation](derives/string_representation.md): `Debug`, `Display`
    - [Comparison](derives/comparison.md): `Eq`, `Ord`, `Hash`
    - [Copying/default](derives/copying_default.md): `Clone`, `Copy`, `Default`
    - [Serialization](derives/serialization.md): `Serialize`, `Deserialize`
    - [Validation](derives/validation.md): `Validate`
    - [Custom behavior](derives/custom_behavior.md): overriding derived behavior
