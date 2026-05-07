---
name: review-scope
description: Review a change against issue, RFC, branch, and user-request scope. Use when a broad review needs a dedicated scope/completeness pass or when the user asks whether the implementation actually matches the intended surface.
---

# Review Scope — Incan Compiler

## Purpose

`/review-scope` is a cross-file report-only reviewer for intended scope and delivered behavior.

Own:

- missing in-scope work
- out-of-scope additions
- issue/RFC/branch intent satisfaction
- docs/examples/scaffolds promising behavior the code does not deliver

Do not own:

- local prose quality unless it creates scope drift
- detailed architecture placement
- test style details
- final branch-clean judgment

## Output artifact

Write a slice report at:

- `.agents/state/review-report.scope.md`

Do not write to the canonical `.agents/state/review-report.md`.

## Workflow

1. Derive intended scope from:
   - current user request
   - issue/branch metadata
   - governing RFC/design docs
   - user-facing docs/examples on the branch
2. Review the whole presented worktree as a single review surface, not only the orchestrator's guess at the "main task files".
3. Flag:
   - missing promised behavior
   - scope creep
   - examples/docs/scaffolds that imply unsupported flows
   - adjacent branch churn or future-work noise when it materially affects reviewability or branch quality
4. Attach findings to concrete files when possible, even if the reasoning was cross-file.
5. Do not suppress a finding just because it may be non-blocking for the immediate fix loop. Relevance is decided later.

## Slice report shape

```md
# Review Slice Report

- role: scope
- worker: <agent-id or stable label>
- status: in_progress | clean | blocked

## Scope
- issue / RFC:
- reviewed surfaces:
  - ...

## Findings
- [ ] blocker | behavior | docs-generated fiction | workspaces/docs-site/docs/language/reference/project_lifecycle.md:181
  Says plain `incan build` remains a valid direct command, but the CLI still requires `<FILE>` outside `--lib`.

## Reviewed Clean Surfaces
- <optional, only for risky or disputed clean calls>

## Notes
- <optional broader scope observation>

Use `Notes` for context, but keep actual concerns in `## Findings` rather than hiding them in prose.
```

If there are no findings, say so explicitly.
