---
name: review-orchestrate
description: Orchestrate a broad Incan review using specialized reviewer roles, parallel slice reports, and a canonical merged review report. Use when the user asks for delegated review, multi-agent review, or when a broad dirty worktree should be reviewed by specialized passes rather than one generic reviewer.
---

# Review Orchestrate — Incan Compiler

## Purpose

`/review-orchestrate` is the review control plane for broad worktrees.

It does not replace the reviewer roles. It:

- derives scope,
- chooses the reviewer roster,
- spawns specialized reviewers,
- gathers slice reports,
- merges them into the canonical `.agents/state/review-report.md`,
- preserves findings with provenance,
- records disagreements when they matter,
- applies a Boy Scout closing gate,
- and decides whether the branch is clean, blocked, or should move to `/fix`.

Use this when:

- the user explicitly wants multi-agent review,
- the dirty worktree spans multiple subsystems or both code and docs,
- or serial reruns are becoming more expensive than parallel review.

For small local reviews, use `/review` instead.

This skill is only valid when it uses real subagents for reviewer slices. If the orchestrator stays single-agent and merely writes multiple slice reports itself, that run is non-compliant with `review-orchestrate`.

## Reviewer roster

Core reviewer roles:

- `review-rust-prose`
- `review-docs-claims`
- `review-test-style`
- `review-scope`
- `review-architecture`
- `review-code-smells`

Cross-cutting orchestrator gates:

- `boyscout_gate`
- disagreement resolution
- canonical report merge

## Reports

The slice reports are the primary review artifacts.

The canonical report is a thin merged index:

- who reviewed what,
- what findings remain,
- where disagreements exist,
- what was verified,
- and whether the branch is clean or blocked.

It is not a second full review document. The slice reports remain the detailed evidence.

Do not copy every per-file checklist from worker reports into the canonical report. Keep the detail in the slice
reports and let the canonical report point at it.

Worker slice reports:

- `.agents/state/review-report.rust-prose.md`
- `.agents/state/review-report.docs-claims.md`
- `.agents/state/review-report.test-style.md`
- `.agents/state/review-report.scope.md`
- `.agents/state/review-report.architecture.md`
- `.agents/state/review-report.code-smells.md`

Canonical merged report:

- `.agents/state/review-report.md`

Workers do **not** write to the canonical report directly.
Every worker slice report must identify the worker that produced it, and the canonical report must record the worker set used for the run.

## Workflow

1. Derive review scope from the current dirty worktree.
2. Decide which reviewer roles are needed. Use the lightest honest roster:
   - touched `.rs` with prose comments -> `review-rust-prose`
   - touched user-facing `.md`, CLI help, examples, scaffolds, release notes -> `review-docs-claims`
   - touched tests or code implying coverage drift -> `review-test-style`
   - broad branch-intent / RFC / issue validation -> `review-scope`
   - subsystem-level code changes -> `review-architecture`
   - touched code files with local maintainability risk -> `review-code-smells`
3. Spawn real subagents for the reviewer slices. Do not perform the reviewer roles yourself except for final orchestration and merge.
4. Use `orchestrate-parallel-work` when the user explicitly asked for subagents/delegation and the slices are worth parallelizing. For broad dirty-worktree review, that should be the default execution model of this skill.
5. Give each reviewer explicit owned scope and forbid edits unless the user explicitly asked for review-and-fix behavior.
6. Collect the slice reports from those worker agents.
7. Merge them into `.agents/state/review-report.md`.
8. Preserve reviewer findings by default. Do **not** prune aggressively at merge time just because the orchestrator is skeptical. High recall matters more than prematurely cleaning the queue.
9. Merge duplicates aggressively. If multiple reviewers identify the same real issue, record it once with multiple sources.
10. Do **not** require consensus for a finding. One well-grounded finding is enough to keep it open.
11. Every merged finding must keep:
   - its reviewer role(s),
   - its source slice report(s),
   - a `kind` used for handling, not suppression.
12. Useful `kind` values include:
   - `behavior`
   - `docs`
   - `rustdoc`
   - `test-gap`
   - `maintainability`
   - `design-tension`
13. A finding may also carry a merge disposition:
   - `open`
   - `fixed`
   - `blocked`
   - `unverified`
   - `contested`
   Do not drop a finding merely because it is `unverified` or `contested`; keep it visible with provenance.
14. Record disagreement only when it is real and useful:
   - a reviewer explicitly disputes a clean call on a risky surface,
   - reviewers materially disagree on severity, ownership, or whether something is in scope,
   - or a clean/blocked decision depends on the disagreement.
15. Silence is not disagreement. “Reviewer A found an issue and reviewer B did not mention it” is not consensus and is not
    disagreement by itself.
16. Review the whole presented worktree. Do not arbitrarily narrow the review surface to the orchestrator's guess at the user's "real task". Findings about adjacent churn, stale docs, or quality regressions are fair game. Relevance and fixability are decided later by `/fix` or the human, not by suppressing the finding.
17. Apply the `boyscout_gate`: ask whether touched code/docs ended at least as clean as they were.
18. Mark the canonical report `clean` only when:
   - applicable reviewer slices are complete,
   - no open findings remain,
   - and no open disagreements remain on risky clean claims.

## Canonical report merge shape

```md
# Review Report

- branch: <branch>
- status: in_progress | clean | blocked

## Workers
- rust-prose: <agent-id or stable label>
- docs-claims: <agent-id or stable label>
- ...

## Scope
- reviewed worktree:
- reviewer roster:
- primary slice reports:
  - .agents/state/review-report.rust-prose.md
  - ...

## Activity
...

## Findings

- [ ] F1 | warning | rustdoc | rustdoc prose | src/project_lifecycle/version.rs:122
  roles: rust-prose
  sources:
    - .agents/state/review-report.rust-prose.md
  status: open
  summary: Manually chopped single-paragraph rustdoc.

- [ ] F2 | error | behavior | env dependency overlays are inspectable but not executable | src/cli/commands/lifecycle.rs:193
  roles: scope, architecture
  sources:
    - .agents/state/review-report.scope.md
    - .agents/state/review-report.architecture.md
  status: open
  summary: Env overlays resolve for display but do not enter command execution/dependency resolution.

## Disagreements

- none

## Clean Corroboration

- src/project_lifecycle/env.rs — rust-prose clean
- workspaces/docs-site/docs/tooling/tutorials/getting_started.md — docs-claims clean

Only record clean corroboration where the lack of findings materially matters:

- touched `.rs` files with prose comments,
- touched user-facing `.md`,
- docs/CLI/example claim surfaces,
- or files involved in a resolved disagreement.

Do not bloat the canonical report with routine clean restatements for every slice.

## Verification
- <command> — <result>
```

## Disagreement policy

- Do not treat “no finding from one reviewer” as proof the file is clean.
- Do not require consensus to keep a finding open.
- If two reviewers truly disagree, record that explicitly in `## Disagreements`.
- A risky clean claim is final only when:
  - two relevant reviewers support the clean call, or
  - the orchestrator resolves the disagreement explicitly and says why.

Risky file classes include:

- touched `.rs` files with prose comments
- touched user-facing `.md`
- CLI/help/examples/scaffolds with behavioral claims
- release notes for implemented user-facing work

## Compliance rules

A `review-orchestrate` run is invalid if any of the following is true:

- no real reviewer subagents were spawned
- the orchestrator wrote the slice reports itself instead of collecting them from workers
- the canonical report omits worker identities for the slices
- the canonical report is missing
- the canonical report copies worker checklists wholesale instead of acting as a thin merged index
- the orchestrator claims corroborated clean/disagreement state without worker-produced slice reports
- the orchestrator silently drops findings instead of preserving them as `open`, `unverified`, or `contested`

If subagents cannot be used, do not pretend this skill ran successfully. Fall back explicitly to `/review` and say that `review-orchestrate` could not be honored.

## Relationship to other skills

- Use `orchestrate-parallel-work` as the worker-spawning substrate when delegation is justified.
- Use `/fix` after `review-orchestrate` if the user wants findings repaired.
- Use `/review-and-fix` only for smaller scopes or after narrowing the problem; for broad scopes, prefer `review-orchestrate` followed by `/fix`.
