---
name: review-docs-claims
description: Review user-facing docs, CLI reference text, examples, and scaffolds for truthfulness, prose quality, and RFC leakage. Use when a broad review needs a dedicated docs/claims pass or when the user asks for docs-truth review specifically.
---

# Review Docs Claims — Incan Compiler

## Purpose

`/review-docs-claims` is a narrow report-only reviewer for touched user-facing documentation and examples.

Own:

- docs/CLI/examples/scaffolds claiming unimplemented behavior
- user-facing RFC leakage outside explicit inventories
- touched markdown prose quality
- release-note inventory consistency when applicable

Do not own:

- Rust comment prose
- architectural placement
- test style
- final branch-clean judgment

## Output artifact

Write a slice report at:

- `.agents/state/review-report.docs-claims.md`

Do not write to the canonical `.agents/state/review-report.md`.

## Workflow

1. Review the touched user-facing `.md` files, CLI help surfaces, examples, and scaffolds assigned by the orchestrator.
2. Check actual implementation against the docs. Prefer the current code and current tests over optimistic prose, stale assumptions, or superseded branch history.
3. RFC text is still canonical, but if the current branch deliberately diverges and the divergence is explicitly documented with a coherent reason, report that as a documented deviation rather than blindly calling it fiction.
4. Flag:
   - docs-generated fiction
   - user-facing RFC references outside explicit inventory contexts
   - short-prosed or mechanically chopped markdown
   - stale release-note inventory for implemented user-facing work
5. If behavior is out of scope, prefer correcting the docs rather than inventing implementation in the report.

## Slice report shape

Keep slice reports findings-first. Do not enumerate every clean file with a full checklist. Only record:

- actual findings,
- and, optionally, a short `## Reviewed Clean Surfaces` section for risky or non-obvious clean calls.

```md
# Review Slice Report

- role: docs-claims
- worker: <agent-id or stable label>
- status: in_progress | clean | blocked

## Scope
- assigned files:
  - workspaces/docs-site/docs/foo.md

## Findings

- [ ] blocker | docs | docs-generated fiction | workspaces/docs-site/docs/foo.md:76
  Claims `requires-incan` is enforced today, but the branch does not implement that.

## Reviewed Clean Surfaces

- workspaces/docs-site/docs/bar.md — docs/claims reviewed; no findings
```

If there are no findings, say so explicitly.
