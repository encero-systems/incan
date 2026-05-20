# Incan Contributor Book (Advanced, Rust-first)

This is a linear, Rust-first track for contributors working on the Incan compiler, tooling, and docsite.

This book is intentionally opinionated and may duplicate some material from the standalone contributing pages so it can be read **end-to-end** like a book.

## Before you start (required)

- [Architecture](../../explanation/architecture.md) — compilation pipeline + module layout
- [Extending the language](../../how-to/extending_language.md) — builtin vs new syntax decision + end-to-end checklists
- [Layering rules](../../explanation/layering.md) — dependency boundaries and guardrails
- [Readable, maintainable Rust](../../explanation/readable-maintainable-rust.md) — conventions and guardrails for changing the Rust codebase
- [RFC index](../../../RFCs/index.md) — required for *new* language features (syntax/semantics), not for bugs/chores

## Chapters

Read in order:

1. [01. Architecture tour](01_architecture_tour.md)
2. [02. Layering and boundaries](02_layering_and_boundaries.md)
3. [03. Proposals: issues vs RFCs](03_proposals_issues_vs_rfcs.md)
4. [04. Your first change: add a builtin](04_add_a_builtin.md)
5. [05. Your first syntax change](05_add_new_syntax.md)
6. [06. Tooling loop: formatter + tests](06_tooling_loop_formatter_and_tests.md)
7. [07. Tooling loop: LSP](07_tooling_loop_lsp.md)
8. [08. Docsite contributor loop](08_docsite_contributor_loop.md)
