# Contributing to Incan

Incan compiles checked source through Body IR and Incan lowering into the pinned rustc. Oven is the Incan command line and builds units from `loaf.toml`. This repository has no Cargo project.

## Start here

- [Contributor index](workspaces/docs-site/docs/contributing/index.md)
- [Architecture](workspaces/docs-site/docs/contributing/explanation/architecture.md)
- [Extending the language](workspaces/docs-site/docs/contributing/how-to/extending_language.md)
- [Readable Incan](workspaces/docs-site/docs/contributing/explanation/readable-incan.md)
- [Readable Rust](workspaces/docs-site/docs/contributing/explanation/readable-maintainable-rust.md)
- [Contract families](workspaces/contracts/README.md)

## Prepare the checkout

Use an installed dev.8 toolchain with its matching compiled standard packages. See the [installation guide](workspaces/docs-site/docs/tooling/how-to/install_and_run.md) for toolchain setup and [compiler driver](loaves/toolchain/incan-rustc-driver/README.md) for its unit interface.

```bash
git clone https://github.com/encero-systems/incan
cd incan
make install-hooks
incan run examples/simple/hello.incn
```

The hook setup selects `.githooks/`. Commit messages are authored by their committer and carry no tool attribution, session trailer or generation footer.

## Find the owning boundary

| Area | Source |
| --- | --- |
| Syntax and parsing | `loaves/kernel/incan_syntax/src/` |
| Type checking | `loaves/compiler/incan_frontend/src/typechecker/` |
| Checked Body IR construction | `loaves/compiler/incan_frontend/src/body_ir/` |
| Body IR types | `loaves/kernel/incan_semantics_core/src/body_ir.rs` |
| Incan lowering | `loaves/compiler/incan_mir_lowering/src/` |
| Native plan | `loaves/compiler/incan_mir_plan/src/` |
| rustc adapter | `loaves/toolchain/incan-rustc-driver/src/` |
| Compilation sessions | `loaves/compiler/incan_driver/src/session.rs` |
| Oven | `workspaces/oven/src/` |
| Standard packages | `loaves/stdlib/` |

Read nearby same-stage implementations before editing. The checker owns resolved types, identities and ownership facts; later stages consume them. A lowering fix must not guess a missing checked fact from a source name.

Application behavior and tooling whose home is Incan stay authored in Incan. Rust changes belong in existing Rust compiler and adapter components. Use an existing crate through `rust::` interop before introducing a bridge.

## Prove a change

Extend an existing contract case in the same area before adding a case. Preserve the original expectations and establish a failing result before repairing behavior. The [contract runner guide](workspaces/contracts/README.md) documents selectors, scratch directories and worker limits. Run the smallest selector during development and the affected whole families at commit points.

```bash
make fmt
make pre-commit-fast
make us-english-check
```

`make fmt` runs file-scoped rustfmt. Format Incan like the surrounding source. Every changed Rust function or method needs accurate rustdoc; every new Incan declaration needs a docstring. Use Result propagation rather than `.unwrap()` or `.expect()`, including in tests.

Run `make docs-build` from `workspaces/docs-site` when changing the docs site. Review each page's intent separately: a successful build proves links and rendering, while reference pages still need a complete public contract.

## Check documentation paths

Run `make doc-paths` to check concrete repository references in contributor documentation, non-state Markdown under `.agents/`, contributor pages and Loaf READMEs. Point each reference at the current owning source, or remove it when that source is gone. Do not create a file merely to satisfy a reference.

The checker also reads fenced examples and diagrams. Use real repository inputs for runnable examples. `scripts/check_doc_paths.allow` records intentional illustrative references as a document, exact token and reason separated by tabs; a trailing `/**` names a subtree. An exception must explain an illustrative reference, never hide a stale implementation path.

## Pull requests

Explain the concrete behavior that changed and the evidence that proves it. Follow the repository's PR template and commit-message convention. Keep source, tests and documentation together when they establish the same contract. Report unrun checks and remaining failures precisely.

By contributing, you agree that your contributions are licensed under Apache 2.0.
