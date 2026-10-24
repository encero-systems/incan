# Contributing: Start here

This section is for contributors working on the Incan compiler/tooling and language evolution.

If you’re new, start with:

- [Architecture](explanation/architecture.md)
- [Layering rules](explanation/layering.md)

## How-to guides (do)

- [Extending the Language](how-to/extending_language.md) — when to add builtins vs new syntax; end-to-end checklists
- [Author Library DSLs with `incan_vocab`](how-to/authoring_vocab_crates.md) — how to publish import-activated DSL blocks, scoped surfaces, and desugarers

## Tutorials (learn)

- Contributor Book (Advanced, Rust-first): [Book index](tutorials/book/index.md)

## Explanation (understand)

- [Architecture](explanation/architecture.md) — compilation pipeline, module layout, internal stages
- [Readable, maintainable Rust](explanation/readable-maintainable-rust.md) — team conventions and engineering practices
- [Hees.ai v0.5 dependency inventory](explanation/hees_ai_v05_dependency_inventory.md) — historical proof-lane dependency map and scope guardrails

## Reference (look up)

- [Contributors](contributors.md) — community acknowledgments for external contributions
- [Layering rules](explanation/layering.md) — dependency boundaries and guardrails

## Design (RFCs and roadmap)

RFCs are design records (not canonical user docs):

- [RFC index](../RFCs/index.md)
- [Roadmap](../roadmap.md)

Before proposing *new* language features (syntax/semantics), read:

- [Proposals: issues vs RFCs](tutorials/book/03_proposals_issues_vs_rfcs.md)
