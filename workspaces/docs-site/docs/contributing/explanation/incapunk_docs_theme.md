# Incapunk docs theme

The Incapunk docs theme is the visual language for the public documentation site: forged rails, graphite surfaces, restrained chroma, and structure-first ornament.

The source handoff lives in `workspaces/incapunk/`. It is useful as a reference, but it is not itself the production site. The handoff README is explicit: "This is **not** a production docs page" and "It should not be copied into production as raw HTML."

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

## Reference workflow

Use `workspaces/incapunk/` as the visual reference when changing the theme. It captures the intended direction: "forged gold rails over dark graphite surfaces", "restrained cyan/magenta chroma embedded into structural edges", and "premium docs chrome rather than generic dark-mode SaaS styling."

When a reference detail does not scale cleanly to MkDocs Material, prefer the simpler site-native version. The handoff notes the same constraint: "If something in this guide feels too custom to scale across a real docs theme, the guide is wrong and should be simplified rather than forcing MkDocs Material into page-specific hacks."

## CSS maintenance rules

- Edit existing sections rather than appending late override piles.
- Keep reusable visual decisions in the root Incapunk tokens.
- Do not override MkDocs Material's column layout with custom page grids.
- Keep rail and flair effects scoped to structural frames.
- Run `make docs-build` after theme changes.
- Restart `make docs` before visual review because CSS edits are not reliably live-reloaded.
