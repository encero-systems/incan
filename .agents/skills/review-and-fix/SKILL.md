---
name: review-and-fix
description: Run the Incan review/fix loop end to end: review, fix actionable findings, and review again until clean or blocked. Use when the user wants an autonomous review-and-repair pass or says /review-and-fix.
---

# Review And Fix — Incan Compiler

## Purpose

`/review-and-fix` is the user-facing entry point for autonomous review plus repair.

The user should not need to choose between the simple reviewer and the orchestrated reviewer. This skill decides that
itself from the current worktree.

Under the hood it chooses one of two paths:

- **small/local scope**: `/loop /review /fix`
- **broad/multi-surface scope**: `review-orchestrate` -> `/fix` -> final review pass

`/loop` still owns repetition and stopping conditions for the small/local path. For broad scopes, this skill owns the
high-level routing and uses the persistent report at `.agents/state/review-report.md`.

## Workflow

1. Inspect the current dirty worktree before choosing a path.
2. Choose the **small/local** path when the change is narrow enough for one honest reviewer, for example:
   - a small number of touched files,
   - one main subsystem,
   - code-only or docs-only,
   - no evidence of cross-cutting code/docs/tests/RFC churn.
3. Choose the **broad/multi-surface** path when any of the following are true:
   - multiple subsystems are touched,
   - both code and user-facing docs moved,
   - tests, docs, and implementation all changed,
   - the worktree is large enough that one reviewer is likely to skip surfaces,
   - the user explicitly wants subagents or delegated review.
4. For the **small/local** path:
   - invoke `/loop` with `/review` as detector and `/fix` as repair skill,
   - let `/loop` repeat until clean or legitimately blocked.
5. For the **broad/multi-surface** path:
   - run `review-orchestrate`,
   - consume the merged findings with `/fix`,
   - then run a final review pass appropriate to the remaining scope.
6. Treat `.agents/state/review-report.md` as a live structured artifact, not a final prose summary.
7. For orchestrated runs, treat slice reports as the primary evidence and the canonical report as a thin merged
   findings index. Preserve findings by default; do not over-prune them during merge.
8. Each new `/review-and-fix` invocation starts with a fresh report scaffold. Do not reuse an older run's findings or
   activity log.

## Routing rule

The default user expectation should be simple:

- users ask for `/review-and-fix`
- this skill decides whether the work is small or broad
- the user does not need to manually choose `/review` vs `review-orchestrate`

If the scope is borderline, prefer the broader review path. The cost of one wider first pass is lower than repeated
serial misses and reruns.

## Verification expectations

The repair pass should aim to leave the worktree in a strong verified state:

- `make fmt`
- narrow relevant checks
- `cargo run -p incan_core --bin generate_lang_reference` when language registries, the reference generator, or `workspaces/docs-site/docs/language/reference/language.md` changed; inspect and commit any resulting generated diff
- `make pre-commit`

If the broad gate fails for unrelated environmental reasons, report that explicitly as residual risk instead of
pretending the loop is clean.

## Output format

Produce a short summary:

```md
## Review And Fix — <subject / branch>

### Chosen path
<small/local review loop or broad orchestrated review>

### Review findings
<high-signal findings from the chosen review path>

### Fixes applied
<what changed>

### Final review state
<clean / blocked / residual risk>

### Verification
<commands and outcomes>
```
