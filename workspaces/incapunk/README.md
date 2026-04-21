# Incapunk Style Guide

This workspace is a self-contained handoff artifact for the **Incapunk** visual style.

It exists so a frontend developer can inspect the intended look-and-feel, break the
system into reusable pieces, and port the relevant parts into the real MkDocs Material
docs site.

## What this folder contains

- `index.html`
  A single-page style guide that demonstrates the intended Incapunk language.
- `styles.css`
  The full visual system used by the handoff page.
- `assets/background_002.png`
  Background art used by the guide.
- `assets/wordmark_small_001.png`
  Small wordmark used in the header.
- `assets/wordmark_inspiration_006.png`
  Larger wordmark used in the hero treatment.

## What the page is for

This is **not** a production docs page.

It is a visual reference that captures the target style language:

- forged gold rails over dark graphite surfaces
- restrained cyan/magenta chroma embedded into structural edges
- premium docs chrome rather than generic dark-mode SaaS styling
- reusable docs primitives such as:
  - headers
  - dividers
  - admonitions
  - code blocks
  - tables
  - lists
  - hero / landing-page CTA language

## What it is not for

- It is not a finished MkDocs Material theme.
- It is not a page-template system.
- It does not imply custom RFC frontmatter or page-specific hero wrappers across the docs.
- It should not be copied into production as raw HTML.

## Expected implementation approach

The intended porting strategy is:

1. Extract the shared tokens:
   - palette
   - typography
   - border/rail logic
   - chroma behavior

2. Apply them to MkDocs Material primitives:
   - top header chrome
   - sidebar / table of contents
   - admonitions
   - tables
   - code blocks
   - horizontal rules
   - ordered / unordered lists

3. Use template overrides only where MkDocs Material structure truly requires it.

## Design rules worth preserving

- Chroma should live in rails, seams, and edge behavior.
- Gold defines structure; it should not flood the page text.
- Body copy should stay cooler and more legible than the frames around it.
- Ornament should support hierarchy, not fragment it.
- If an effect cannot be tied back to real structure, it is probably wrong.

## Practical note

If something in this guide feels too custom to scale across a real docs theme,
the guide is wrong and should be simplified rather than forcing MkDocs Material
into page-specific hacks.
