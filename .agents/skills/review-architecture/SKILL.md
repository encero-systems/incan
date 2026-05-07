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

## Output artifact

Write a slice report at:

- `.agents/state/review-report.architecture.md`

Do not write to the canonical `.agents/state/review-report.md`.

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
