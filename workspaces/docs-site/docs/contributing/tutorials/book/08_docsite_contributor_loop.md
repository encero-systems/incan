# 8. Docsite contributor loop

Incan’s docs are a product surface. Contributors should be able to run the docsite locally and validate changes before committing.

## Run the site locally

From the repo root:

```bash
make docs
```

This should build and serve the site so you can click through it locally.

## How the docs are structured

We organize docs using the Divio model:

- tutorials (learn)
- how-to guides (do)
- reference (look up)
- explanation (understand)

When adding or moving content, prefer improving clarity and findability over creating new one-off pages.

## Reuse patterns (snippets)

We reuse common content via snippets in `workspaces/docs-site/docs/_snippets/`:

- commands
- callouts
- diagrams
- tables

### Mermaid diagrams

Mermaid diagrams are stored as DSL-only `.mmd` files and included via snippets. Keep diagrams as DSL-only content, and wrap them in a Mermaid fence where included.

## Contributor checklist for doc changes

Before opening a PR:

- ensure `make docs` works locally
- run a strict build: `make docs-build`
- keep snippets reusable and scoped (don’t copy/paste long command sequences everywhere)
