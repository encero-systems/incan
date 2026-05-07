---
name: review-rust-prose
description: Review touched Rust files for prose-comment quality, rustdoc accuracy, and manually chopped comment wrapping. Use when a broad review needs a dedicated Rust prose/rustdoc pass or when the user asks for comment-quality review specifically.
---

# Review Rust Prose — Incan Compiler

## Purpose

`/review-rust-prose` is a narrow report-only reviewer for touched `.rs` files that contain `///`, `//!`, or prose `//`.

Own:

- rustdoc paragraph shape
- manually chopped comment wrapping
- touched doc/comment accuracy against current behavior
- public/non-trivial documentation coverage in touched Rust code

Do not own:

- test style
- docs-site markdown
- architectural placement
- final branch-clean judgment

## Output artifact

Write a slice report at:

- `.agents/state/review-report.rust-prose.md`

Do not write to the canonical `.agents/state/review-report.md`.

## Workflow

1. Review only the touched `.rs` files assigned by the orchestrator. If no slice was assigned, limit yourself to touched `.rs` files with prose comments.
2. For each file, inspect touched prose blocks directly. Do not infer prose quality from broad code reading.
3. Flag:
   - clause-by-clause or narrow-column manual wrapping
   - fake paragraph breaks
   - stale or misleading rustdoc
   - missing touched public/non-trivial docs that should exist
4. Treat the accepted shape as:
   - natural paragraph prose, then `make fmt` wrapping
   - or multiple paragraphs separated for a real semantic break
5. Record only Rust prose/documentation findings for this slice.

## Slice report shape

Keep slice reports findings-first. Do not enumerate every clean file with a full checklist. Only record:

- actual findings,
- and, optionally, a short `## Reviewed Clean Surfaces` section for risky or non-obvious clean calls.

```md
# Review Slice Report

- role: rust-prose
- worker: <agent-id or stable label>
- status: in_progress | clean | blocked

## Scope
- assigned files:
  - src/foo.rs

## Findings

- [ ] warning | rustdoc prose | src/foo.rs:42
  Manually chopped single-paragraph rustdoc.

## Reviewed Clean Surfaces

- src/bar.rs — rust prose reviewed; no findings
```

Only include `## Reviewed Clean Surfaces` when the clean call matters to the orchestrator, such as:

- touched `.rs` files with prose comments,
- files that are likely to be checked by another reviewer,
- or surfaces involved in disagreement resolution.

If there are no findings, say so explicitly.
