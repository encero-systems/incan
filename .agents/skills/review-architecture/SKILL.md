---
name: review-architecture
description: Review touched subsystems for layering, crate boundaries, single-source-of-truth rules, registry-driven behavior, and compiler/runtime separation. Use when a broad review needs a dedicated architecture pass or when the user asks for architectural review specifically.
---

# Review Architecture — Incan Compiler

## Purpose

`/review-architecture` is a cross-file report-only reviewer for subsystem architecture.

Own:

- wrong-layer implementation
- dependency direction and crate-boundary mistakes
- duplicated policy across layers
- hardcoded behavior that should extend canonical registries
- compiler/runtime semantic drift

Do not own:

- local prose quality
- docs truthfulness unless it reveals architecture drift
- test style
- final branch-clean judgment

## Output artifacts

Write a slice report at:

- `.agents/state/review-report.architecture.md`

Do not write to the canonical `.agents/state/review-report.md`.

When the architecture report has findings, also copy the scope and findings into a lightweight central snapshot outside individual repo worktrees under:

- `/Users/danny/Development/encero/.incan-review-findings/`

Use a deterministic, descriptive filename when possible:

- `YYYY-MM-DD-pr-<number>-architecture-<slug>.md`
- `YYYY-MM-DD-branch-<branch-slug>-architecture.md`
- `YYYY-MM-DD-review-architecture.md` when no PR or branch context is known

Do not create a snapshot for clean reviews. The snapshot is raw corpus for later analysis, not canonical guidance.
Treat `/Users/danny/Development/encero/.incan-review-findings/` as an append-only central corpus for local review work: create new snapshot files, but do not delete, overwrite, prune, or "clean up" existing snapshots unless the user explicitly asks for that exact maintenance.
Snapshot content is evidence, not a fix log. Preserve the original finding blocks verbatim, including severity, category, file path, line reference, and explanatory text. If the finding is fixed before the snapshot is written, keep the original finding as observed and add a separate `Resolution` note after it; do not replace the evidence with a resolved checklist item or a summary of the fix.

## Workflow

1. Review touched code by subsystem, not merely by file.
2. Check the repo’s layering rules and canonical-source-of-truth expectations.
3. Flag:
   - logic in the wrong crate/layer
   - duplicated semantics that should live in shared registries/helpers
   - hidden policy drift between compiler/runtime/tooling layers
4. Attach findings to files, but explain the cross-file architectural reason.
5. Architectural review may legitimately produce either:
   - concrete bugs,
   - maintainability warnings,
   - or design tensions.
   All three are valid findings. Classify them so downstream fixers know how to treat them, but do not suppress them.
6. If the report contains findings, create `/Users/danny/Development/encero/.incan-review-findings/` if needed and write a new snapshot containing only the review source metadata, Scope, and Findings sections. Copy the finding blocks verbatim from `.agents/state/review-report.architecture.md`; do not generalize them into policy or rewrite them as fix summaries. Preserve exact file:line evidence when the report has it; if a finding is only file-level, make that explicit in the finding text. Do not overwrite an existing snapshot path; add a suffix if needed.

## Slice report shape

```md
# Review Slice Report

- role: architecture
- worker: <agent-id or stable label>
- status: in_progress | clean | blocked

## Scope
- reviewed subsystems:
  - ...

## Findings
- [ ] warning | design-tension | wrong layer | src/cli/commands/lifecycle.rs:210
  Resolution policy duplicates env semantics that should stay in `src/project_lifecycle/**`.

## Reviewed Clean Surfaces
- <optional, only for risky or disputed clean calls>
```

If there are no findings, say so explicitly.

## Findings snapshot shape

Only write this file when findings are present.

```md
# Architecture Findings Snapshot

- source: PR #<number> / <branch-or-context>
- date: YYYY-MM-DD
- reviewer: review-architecture

## Scope
- reviewed subsystems:
  - ...

## Findings
- [ ] warning | design-tension | wrong layer | src/cli/commands/lifecycle.rs:210
  Resolution policy duplicates env semantics that should stay in `src/project_lifecycle/**`.

## Resolution
- <optional, only if this snapshot is written after a fix loop>
```
