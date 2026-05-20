# 7. Tooling loop: LSP

The language server is the “always-on” feedback loop for contributors and users. If the compiler changes, the LSP must not drift.

## What the LSP should do

At minimum, the LSP should:

- parse current files
- report syntax and type errors
- provide basic navigation (hover/definition/completion)

## How it works (high level)

The LSP reuses Incan’s compiler frontend stages to produce diagnostics from edits.

See:

- [LSP](../../../tooling/how-to/lsp.md)
- [LSP architecture](../../../tooling/explanation/lsp_architecture.md)
- [LSP protocol support](../../../tooling/reference/lsp_protocol_support.md)

## A contributor checklist

After you change syntax or typing rules:

- ensure the LSP can still parse and typecheck files using the new feature
- add a small regression test if a prior bug involved “CLI works, LSP breaks”

## Next

Next chapter: [08. Docsite contributor loop](08_docsite_contributor_loop.md).
