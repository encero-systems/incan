# Language: Start here

This section is about writing `.incn` programs: syntax, semantics, patterns, and mental models.

If you’re not sure where you fit, start at [Start here](../start_here/index.md).

## Deciding if Incan fits

- [Why Incan?](explanation/why_incan.md)
- [How Incan works](explanation/how_incan_works.md)

## Tutorials (learn)

- The Incan Book (Basics): [Book index](tutorials/book/index.md)
- Fallible and infallible paths: [Fallible and infallible paths](tutorials/fallible_and_infallible_paths.md)
- Web framework tutorial: [Web Framework](tutorials/web_framework.md) (advanced; reads like tutorial + how-to)

## How-to guides (do)

- [Async Programming](how-to/async_programming.md)
- [Error Messages](how-to/error_messages.md)
- [File I/O](how-to/file_io.md)
- [Generators](how-to/generators.md)
- [Module state](how-to/module_state.md)
- [Performance](how-to/performance.md)
- [Rust Interop](how-to/rust_interop.md)

## Reference (look up)

- Generated language reference: [Language reference (generated)](reference/language.md)
- Code style (canonical source layout guide): [Incan Code Style Guide](reference/code_style.md)
- Static storage: [Static storage](reference/static_storage.md)
- Numeric semantics: [Numeric Semantics](reference/numeric_semantics.md)
- Strings: [Strings](reference/strings.md)
- Generators: [Generators](reference/generators.md)
- Derives reference cluster:

| Guide                                                               | Derives                     |
| ------------------------------------------------------------------- | --------------------------- |
| [String Representation](reference/derives/string_representation.md) | `Debug`, `Display`          |
| [Comparison](reference/derives/comparison.md)                       | `Eq`, `Ord`, `Hash`         |
| [Copying & Default](reference/derives/copying_default.md)           | `Clone`, `Copy`, `Default`  |
| [Serialization](reference/derives/serialization.md)                 | `Serialize`, `Deserialize`  |
| [Validation](reference/derives/validation.md)                       | `Validate`                  |
| [Custom Behavior](reference/derives/custom_behavior.md)             | Overriding derived behavior |

## Explanation (understand)

- [Control flow](explanation/control_flow.md)
- [Closures](explanation/closures.md)
- [Consts](explanation/consts.md)
- [Module static storage](explanation/static_storage.md)
- [Derives & Traits](reference/derives_and_traits.md)
- [Enums](explanation/enums.md)
- [Generators](explanation/generators.md)
- [Error Handling](explanation/error_handling.md)
- [Imports & Modules](explanation/imports_and_modules.md)
- [Models & Classes](explanation/models_and_classes/index.md)
- [Scopes & Name Resolution](explanation/scopes_and_name_resolution.md)

## See also

- Tooling: [Tooling start here](../tooling/index.md)
- RFCs (design records): [RFC index](../RFCs/index.md)
