# Incan Documentation

[Start here]:start_here/index.md
[Roadmap]:roadmap.md
[the Book]:language/tutorials/book/index.md
New here? Start with: [Start here].

<!-- top level sections -->
[Tooling]:tooling/index.md
[Language]:language/index.md
[Contributing]:contributing/index.md
[RFCs]:RFCs/index.md
[Comparisons]:comparisons/index.md

## Comparisons

If you are evaluating whether Incan belongs in a project, start with the direct comparisons. They explain where Incan is
trying to win, where it loses today, and when Python, Rust, or Python compatibility tools are the better choice.

[Incan vs Python]:comparisons/python.md
[Incan vs Rust]:comparisons/rust.md
[Incan vs Python compatibility tools]:comparisons/python_compatibility_tools.md

| Document | Description |
| --- | --- |
| [Incan vs Python] | Python-like ergonomics without Python's runtime and deployment model |
| [Incan vs Rust] | Rust-native output without writing every application layer in Rust |
| [Incan vs Python compatibility tools] | Why Incan is not RustPython, Codon, Nuitka, or Cython |

## Tooling & Setup

<!-- Tooling sub-sections -->
[Getting Started]:tooling/tutorials/getting_started.md
[Editor Setup]:tooling/how-to/editor_setup.md
[Language Server]:tooling/how-to/lsp.md
[Formatting]:language/reference/code_style.md
[Testing]:tooling/how-to/testing.md

How to install, configure, and use Incan [Tooling].

| Document          | Description                                      |
| ----------------- | ------------------------------------------------ |
| [Getting Started] | Installation and first steps                     |
| [Editor Setup]    | IDE configuration and syntax highlighting        |
| [Language Server] | LSP for diagnostics, hover, and go-to-definition |
| [Formatting]      | Canonical Incan code style and formatting rules  |
| [Testing]         | Test runner (`incan test`)                       |

## Language Guide

<!-- Language sub-sections -->
[Error Messages]:language/how-to/error_messages.md
[Error Handling]:language/explanation/error_handling.md
[File I/O]:language/how-to/file_io.md
[Async Programming]:language/how-to/async_programming.md
[Derives & Traits]:language/reference/derives_and_traits.md
[Testing in Incan]:language/how-to/testing_stdlib.md
[Scopes & Name Resolution]:language/explanation/scopes_and_name_resolution.md
[Imports & Modules]:language/explanation/imports_and_modules.md
[Rust Interop]:language/how-to/rust_interop.md
[Web Framework]:language/tutorials/web_framework.md

How to write Incan code: [Language].

| Document                   | Description                                            |
| -------------------------- | ------------------------------------------------------ |
| [the Book]                 | The Incan Book (Walks you through the Basics from a-z) |
| [Error Messages]           | Understanding and fixing compiler errors               |
| [Error Handling]           | Result, Option, and the `?` operator                   |
| [File I/O]                 | Reading, writing files and path handling               |
| [Async Programming]        | Async/await with Tokio                                 |
| [Derives & Traits]         | Derive macros and trait system                         |
| [Testing in Incan]         | Inline `module tests:`, assertions, fixtures, markers  |
| [Scopes & Name Resolution] | Block scoping, shadowing, and how names are resolved   |
| [Imports & Modules]        | Module system, imports, and built-in functions         |
| [Rust Interop]             | Using Rust crates directly from Incan                  |
| [Web Framework]            | Building web apps with Axum                            |

### Derives Reference

<!-- language/reference/derives sub-sections -->
[String Representation]:language/reference/derives/string_representation.md
[Comparison]:language/reference/derives/comparison.md
[Copying & Default]:language/reference/derives/copying_default.md
[Serialization]:language/reference/derives/serialization.md
[Custom Behavior]:language/reference/derives/custom_behavior.md

| Document                | Description                 |
| ----------------------- | --------------------------- |
| [String Representation] | Debug and Display           |
| [Comparison]            | Eq, Ord, Hash               |
| [Copying & Default]     | Clone, Copy, Default        |
| [Serialization]         | Serialize, Deserialize      |
| [Custom Behavior]       | Overriding derived behavior |

## RFCs (Request for Comments)

Design proposals for upcoming features are recorded in the form of [RFCs].

--8<-- "_snippets/tables/rfcs_index.md"

## Compiler & Contributing

!!! info "For contributors"
    Architecture and compiler-internals docs are primarily for contributors. If you’re learning the language, start with
    the Book and the Language Guide instead of RFCs.

Docs for contributors working on the compiler and language evolution:

[Compiler Architecture]:contributing/explanation/architecture.md
[Extending the Language]:contributing/how-to/extending_language.md
[Contributing Index]:contributing/index.md

| Document                 | Description                                                             |
| ------------------------ | ----------------------------------------------------------------------- |
| [Roadmap]                | Tracks implementation status and near-term planning                     |
| [RFCs]                   | Design proposals for upcoming features are recorded in the form of RFCs |
| [Contributing]           | Contributor documentation landing page                                  |
| [Compiler Architecture]  | Compilation pipeline, module layout, and internal stages                |
| [Extending the Language] | When to add builtins vs new syntax; end-to-end checklists               |
| [Contributing Index]     | Contributor documentation landing page                                  |
