# Incapunk docs theme

The Incapunk docs theme is the visual language for the public documentation site: forged rails, graphite surfaces, restrained chroma, and structure-first ornament.

[Incus](../../project/incus.md), the character who occasionally appears around the site, follows the same rule: he should reward attention without competing with the documentation.

The production theme itself is the durable reference: its tokens, shared components, and canonical Incus artwork are kept in this docs workspace. Avoid building a second visual specification that can drift away from the site.

## Design rules

- Chroma belongs in rails, seams, and edge behavior, not in long prose.
- Gold defines structure; it should not flood normal body text.
- Body copy should stay cooler and more legible than the surrounding frames.
- Ornament should reinforce hierarchy rather than fragment it.
- If an effect cannot be tied back to real documentation structure, remove it or simplify it.

## Implementation model

The production theme is applied through MkDocs Material primitives rather than bespoke page templates. That means the theme should improve the default components people already use:

- header chrome
- primary navigation
- table of contents
- admonitions and details blocks
- tables
- code blocks
- horizontal rules
- lists and task lists
- Mermaid diagrams

This keeps the documentation maintainable. New docs pages should use normal Markdown first; they should not need local HTML wrappers to look like part of the site.

## Incus asset model

Incus uses individual transparent WebP assets rather than a monolithic sprite atlas. The generated manifest groups poses by meaning: `tip`, `info`, `warning`, `hint`, `python`, `rust`, `javascript`, `composed-failure`, `system`, `success`, `neutral`, and `easter-egg`.

Normal MkDocs admonitions are eligible automatically. The runtime chooses at most one Incus slot per page and selects its asset deterministically from the page path, so the character does not jump between poses during navigation or refresh. A small proportion of ordinary tip, hint, and info appearances use the Easter-egg pool instead.

Seasonal assets stay outside the ordinary pool. The zombie variant is grouped under `seasonal-october` and is added to Easter-egg selection only during October.

The extraction inventory and curation rules live in `workspaces/docs-site/scripts/extract_incus_library.py`; the browser-facing manifest lives beside the generated assets under `docs/shared/incapunk/incus-library/`. The original contact sheets are external curation inputs and are intentionally not published with the site. To rebuild the library, pass the two contact-sheet directories explicitly:

```console
python scripts/extract_incus_library.py \
  --source-a /path/to/first-batch \
  --source-b /path/to/second-batch \
  --output docs/shared/incapunk/incus-library
```

## Reference workflow

Use the shared tokens and components in `incapunk.css` as the visual reference when changing the theme: forged gold rails over dark graphite surfaces, restrained cyan and magenta in structural edges, and premium docs chrome rather than generic dark-mode SaaS styling.

When a visual detail does not scale cleanly to MkDocs Material, prefer the simpler site-native version instead of forcing the theme into page-specific hacks.

## CSS maintenance rules

- Edit existing sections rather than appending late override piles.
- Keep reusable visual decisions in the root Incapunk tokens.
- Do not override MkDocs Material's column layout with custom page grids.
- Keep rail and flair effects scoped to structural frames.
- Run `make docs-build` after theme changes.
- Restart `make docs` before visual review because CSS edits are not reliably live-reloaded.
