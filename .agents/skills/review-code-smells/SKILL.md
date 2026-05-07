---
name: review-code-smells
description: Review touched code for maintainability smells such as duplication, awkward indirection, suspicious clones, dead code, poor naming, and Boy Scout cleanup opportunities. Use when a broad review needs a dedicated maintainability pass or when the user asks for code-smell review specifically.
---

# Review Code Smells — Incan Compiler

## Purpose

`/review-code-smells` is a file-focused report-only reviewer for local maintainability issues.

Own:

- duplication
- dead code
- awkward or unnecessary indirection
- suspicious clones
- poor naming in touched code
- touched-code cleanup opportunities that satisfy the Boy Scout rule

Do not own:

- architecture-level placement decisions
- docs truthfulness
- test style details
- final branch-clean judgment

## Output artifact

Write a slice report at:

- `.agents/state/review-report.code-smells.md`

Do not write to the canonical `.agents/state/review-report.md`.

## Workflow

1. Review the touched code files assigned by the orchestrator.
2. Flag:
   - duplicated branches or helpers
   - no-op/unused/dead surfaces
   - awkward APIs or naming in touched code
   - needless complexity relative to the actual requirement
3. Keep this pass practical. Do not invent refactors unrelated to the touched surface.
4. Classify each finding as either:
   - `maintainability`
   - `design-tension`
   - or `bug`
   This classification is for downstream handling only. It must not be used to hide the finding.

## Slice report shape

Keep slice reports findings-first. Only list clean surfaces when that clean call is non-obvious or useful for the
orchestrator.

```md
# Review Slice Report

- role: code-smells
- worker: <agent-id or stable label>
- status: in_progress | clean | blocked

## Scope
- assigned files:
  - src/cli/mod.rs

## Findings

- [ ] warning | maintainability | dead cli surface | src/cli/mod.rs:313
  `--bin` is exposed publicly but has no effect.

## Reviewed Clean Surfaces
- src/cli/commands/init.rs — maintainability reviewed; no findings
```

If there are no findings, say so explicitly.
