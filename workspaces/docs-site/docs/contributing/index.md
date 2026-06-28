# Contributing: Start here

This section is for contributors working on the Incan compiler/tooling and language evolution.

If you’re new, start with:

- [Architecture](explanation/architecture.md)
- [Layering rules](explanation/layering.md)

## How-to guides (do)

- [Extending the Language](how-to/extending_language.md) — when to add builtins vs new syntax; end-to-end checklists
- [Author Library DSLs with `incan_vocab`](how-to/authoring_vocab_crates.md) — how to publish import-activated DSL blocks, scoped surfaces, and desugarers
- [Auditing generated Rust](how-to/auditing_generated_rust.md) — how to produce strict-surface generated Rust review reports

## Tutorials (learn)

- Contributor Book (Advanced, Rust-first): [Book index](tutorials/book/index.md)

## Explanation (understand)

- [Architecture](explanation/architecture.md) — compilation pipeline, module layout, internal stages
- [Duckborrowing](explanation/duckborrowing.md) — backend ownership planning for generated Rust
- [Generated Rust quality](explanation/generated_rust_quality.md) — quality, performance, and hostability contract for emitted Rust
- [Readable, maintainable Rust](explanation/readable-maintainable-rust.md) — team conventions and engineering practices

## Reference (look up)

- [Contributors](contributors.md) — community acknowledgements for external contributions
- [Layering rules](explanation/layering.md) — dependency boundaries and guardrails
- [Generated Rust stdlib coverage](reference/generated_rust_stdlib_coverage.md) — generated-Rust coverage inventory for stdlib modules

## Design (RFCs and roadmap)

RFCs are design records (not canonical user docs):

- [RFC index](../RFCs/index.md)
- [Roadmap](../roadmap.md)

Before proposing *new* language features (syntax/semantics), read:

- [Proposals: issues vs RFCs](tutorials/book/03_proposals_issues_vs_rfcs.md)
