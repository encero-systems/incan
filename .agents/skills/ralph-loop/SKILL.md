---
name: ralph-loop
description: Run a high-thoroughness end-to-end implementation loop using clean worker worktrees, per-slice planning, implementation, explicit review/fix iteration, orchestrator integration, and final commit/PR drafting. Use when the user explicitly wants a Ralph/Wiggum-style loop, autonomous multi-agent execution, a delegated feature/bugfix carried from confirmed scope through review-ready completion, or RFC-driven implementation that should be reviewed, bumped to In Progress, decomposed into phases, and executed through nested sub-loops. Supports Codex subagents by default and external CLI workers such as OpenCode when explicitly requested and available.
---

# Ralph Loop

Use this skill when the user wants more than basic delegation. The goal is not "parallelize some work"; the goal is to carry a scoped task through planning, implementation, review, integration, and ship-ready artifacts with explicit backpressure.

Keep `orchestrate-parallel-work` generic. Use this skill as the opinionated wrapper around it.

## Core stance

- Confirm the requested scope before spawning anything.
- If the input is an RFC, treat RFC hygiene and lifecycle state as part of the work, not pre-existing setup.
- Treat user-facing docs and versioning as part of implementation, not optional closeout polish.
- Optimize for end-to-end correctness, not maximal concurrency.
- Use clean worktrees for real implementation slices.
- Every implementation must land in a fresh worktree rooted under `/Users/danny/Development/encero/tmp` so VS Code picks it up; do not implement in `/tmp` or in the main repo checkout.
- Treat each slice as a managed work packet with durable state on disk, not as a chat thread that has to remember its own scope.
- Require every worker to plan, implement, verify, review, and fix within its owned slice.
- Require the orchestrator to perform its own explicit plan -> do -> check -> act integration loop after collecting worker output.
- Require workers and the orchestrator to maintain a persistent review report at `.agents/state/review-report.md` inside each worktree so findings survive across loops.
- Do not commit, push, or open a PR unless the user explicitly asked for that. When not explicitly asked, still draft the commit message and PR description as ready-to-use artifacts.

## PDCA model

`ralph-loop` is a plan -> do -> check -> act loop, not a linear pipeline.

Use these meanings consistently:

- **Plan**: confirm scope, define slices, create task state, decide the next concrete increment
- **Do**: implement the planned increment in the owned slice
- **Check**: verify behavior, run review, compare delivered work against the requested scope
- **Act**: either accept and move forward, repair findings, or go back to planning when scope is not actually satisfied

Important:

- Review findings that are simple defects belong in **Act -> fix**.
- Evidence that the slice or integration output does not satisfy the intended scope belongs in **Act -> re-plan**.
- Do not treat “missing promised behavior” as just another lint item. That is a planning/state problem first.

## When not to use this skill

Do not use this for:

- a small task where one agent can finish faster than the orchestration overhead
- a tightly coupled design problem with one unresolved architectural decision
- multiple workers that would need to edit the same files or shared API surface
- tasks where the user wants a plan only

## Workflow

### 0. RFC intake mode

If the user feeds the loop an RFC, enter RFC intake mode before ordinary execution.

In RFC intake mode:

1. Run `review-rfc` on the RFC and apply direct fixes.
2. Inspect `Status:` and the design tail honestly.
3. If unresolved questions remain, summarize them and ask the user before bumping anything.
4. Check for process blockers that require user input, such as:
   - external dependencies not yet available
   - another RFC or issue that partially blocks the scope
   - scope that should be split before implementation
5. If the RFC is still `Draft` and design is settled, use `bump-rfc` to move it to `Planned`.
6. If implementation is actually being picked up now, use `bump-rfc` to move it to `In Progress`. For this skill, the parent Ralph loop itself counts as "a contributor has picked up the work"; a child loop does not need its own PR or branch-level ceremony.
7. Use the RFC's `Implementation Plan` and `Progress Checklist` as the source of truth for implementation phases. If they are missing or weak, strengthen them before spawning workers.
8. Establish the docs/version baseline up front: verify the repo's actual dev version from the source-of-truth metadata, identify which user-facing docs must change if the feature lands, and do not assume an older release line from stale release notes or worker worktrees.

Do not quietly force an RFC past open design questions. Stop and ask the user.

### 1. Confirm the scope

Restate the requested end-state before starting execution. Confirm:

- target repository or repositories
- issue / RFC / branch context
- explicit goals
- explicit non-goals
- whether the task is RFC-wide or only a subset of RFC phases
- whether commit / push / PR creation were requested
- whether the user wants Codex subagents or an external backend such as OpenCode

If scope is ambiguous, stop and ask a short numbered list of missing decisions.

### 2. Pick the execution backend

Default to Codex subagents plus git worktrees under `/Users/danny/Development/encero/tmp`.

If the user explicitly wants OpenCode:

- verify `opencode` is installed and callable
- verify auth / provider setup is present enough to run unattended
- prefer non-interactive execution such as `opencode run ...` or a preconfigured OpenCode agent profile
- do not rely on the TUI for unattended worker execution

If OpenCode is requested but unavailable, say so plainly and either stop or fall back to Codex only with the user's approval.

### 3. Prepare the orchestration boundary

Use `start-work` once at the orchestration boundary to resolve issue/RFC context, branch naming, and relevant learnings.

Do not mechanically run `start-work` inside every worker unless each worker owns a distinct issue or RFC. The important requirement is a clean worktree plus resolved context, not duplicated branch ceremony.

Create the worktree root first if it does not exist: `/Users/danny/Development/encero/tmp`.

Create:

- one orchestrator worktree for final integration
- one clean worker worktree per implementation slice

Create durable slice state under each worktree:

- `.agents/state/ralph-loop/overview.md` for orchestrator-wide state
- `.agents/state/ralph-loop/STEERING.md` for operator overrides, urgency changes, and mid-run guidance
- `.agents/state/ralph-loop/slices.json` for the orchestrator-owned slice registry
- `.agents/state/ralph-loop/<slice-id>/scope.md`
- `.agents/state/ralph-loop/<slice-id>/plan.md`
- `.agents/state/ralph-loop/<slice-id>/tasks.json`
- `.agents/state/ralph-loop/<slice-id>/tasks.md`
- `.agents/state/ralph-loop/<slice-id>/status.md`
- `.agents/state/ralph-loop/<slice-id>/handoff.md`

Treat the slice folder as the source of truth for that slice. Do not rely on memory or only on conversational context.

Use these files deliberately:

- `STEERING.md`: human or orchestrator overrides that should preempt normal task ordering
- `slices.json`: orchestrator registry of all active slices, their owners, and their current state
- `tasks.json`: machine-readable task list for the slice
- `tasks.md`: human-readable task narrative and notes
- `status.md`: current PDCA state, blockers, and next action

Base all worker worktrees from the same resolved starting point unless there is a deliberate dependency chain.

For non-decomposed work, the single-agent implementation still belongs in a fresh worktree under `/Users/danny/Development/encero/tmp`; "keep the work local" does not mean "edit the primary checkout directly."

Before spawning workers, identify:

- the source-of-truth version file(s) for the repo
- whether the task is on a `-dev.N` line and therefore needs a version bump
- the authored user-facing docs that must be updated if the change is user-visible

Do not treat RFC edits or release notes alone as sufficient user documentation.

### 4. Decide whether parallelism is justified

Before spawning workers, decide whether the task actually decomposes cleanly. If it does not, keep the work local and continue as a single-agent Ralph loop.

When it does decompose, hand off to `orchestrate-parallel-work` for slice definition, ownership, and worktree isolation under `/Users/danny/Development/encero/tmp`.

If the task came from an RFC:

- derive slices from RFC implementation phases or coherent checklist groupings, not arbitrary percentages
- keep parent ownership of RFC lifecycle edits, progress-checklist updates, commit text, and PR drafting
- treat child loops as implementation subloops only
- allow nested decomposition only one level down: child loops may use `orchestrate-parallel-work` for leaf workers inside their owned scope, but they must not spawn further `ralph-loop` children

For each slice, create `tasks.md` with explicit task items. Keep them concrete and finishable, not vague “phase done” markers.

Also create `tasks.json` for the same slice. `tasks.json` is the authoritative status surface; `tasks.md` is the human-readable companion.

Suggested shape:

```json
{
  "slice_id": "lifecycle-env",
  "status": "planned",
  "tasks": [
    {
      "id": "T1",
      "title": "Establish manifest/env schema ownership boundary",
      "status": "todo",
      "notes": ""
    },
    {
      "id": "T2",
      "title": "Remove CLI-local env schema parsing",
      "status": "todo",
      "notes": ""
    }
  ]
}
```

Example shape:

```md
# Slice Tasks — lifecycle-env

- [ ] Establish manifest/env schema ownership boundary
- [ ] Remove CLI-local env schema parsing
- [ ] Add internal manifest override discovery
- [ ] Make env overlays affect nested `incan lock`
- [ ] Add focused tests
- [ ] Run slice verification
- [ ] Run slice review/fix loop
```

Subagents should work tasks to completion against these files and update them as state changes.

The orchestrator should maintain `slices.json` with entries like:

```json
[
  {
    "slice_id": "lifecycle-env",
    "owner": "<worker name or id>",
    "worktree": "/Users/danny/Development/encero/tmp/...",
    "status": "doing",
    "next_action": "Implement T2"
  }
]
```

### 5. Give each worker a strict end-to-end contract

Each worker must own a non-overlapping slice with:

- exact goal
- owned files or directories
- explicit non-goals
- dedicated worktree path under `/Users/danny/Development/encero/tmp`
- dedicated slice folder under `.agents/state/ralph-loop/<slice-id>/`
- verification command
- expected result format

For RFC-driven work, prefer one child loop per implementation phase or tightly related checklist group.

Each worker must perform this loop inside its slice:

1. **Plan**
   - Build a short slice plan using `create-plan`.
   - Write `scope.md`, `plan.md`, `tasks.json`, and `tasks.md`.
   - Break the slice into concrete tasks inside `tasks.json` and keep `tasks.md` as the readable companion.
   - Set initial slice state in `status.md` as `planned`.
2. **Do**
   - Implement the next planned task set, not the whole universe.
   - Update `tasks.json`, `tasks.md`, and `status.md` as task ownership and progress change.
   - Use `doing` as the active execution state.
3. **Check**
   - Run targeted verification for the slice.
   - Run `review` on the slice in report-only mode, or `review-orchestrate` if the slice itself is broad enough to justify specialization.
   - Compare the delivered behavior against `scope.md`, not only against test output.
   - Use `checking` as the active verification/review state.
4. **Act**
   - Run `fix` on every actionable in-scope blocker or warning.
   - If the slice hits a likely compiler bug that is out of scope, invoke `flag-compiler-bug` before reporting the blocker or choosing a workaround.
   - If check/review shows that promised scope is still not satisfied, change slice state to `replan_required` and go back to **Plan**. Update `scope.md`, `plan.md`, `tasks.json`, and `tasks.md` before doing more code.
   - If outside help is needed, set slice state to `blocked` with a concrete blocker.
   - If the slice is actually complete, set slice state to `done` and write `handoff.md`.
5. Repeat until:
   - no actionable in-scope items remain,
   - the slice tasks are complete,
   - and `scope.md` is honestly satisfied,
   or report a concrete blocker with its classification.

The slice's `.agents/state/review-report.md` must be kept current throughout this loop.
The slice folder under `.agents/state/ralph-loop/<slice-id>/` must also stay current throughout this loop.

Allowed slice states:

- `planned`
- `doing`
- `checking`
- `replan_required`
- `blocked`
- `done`

Do not invent vague alternatives like “mostly done” or “almost ready”.

Workers must be told:

- they are not alone in the repo
- they must not revert or overwrite others' work
- they must adapt to concurrent changes
- they must not produce PRs, PR descriptions, or final commit artifacts of their own when they are child loops under a parent RFC loop
- child loops must not spawn further `ralph-loop` children; if they need extra decomposition, they may use `orchestrate-parallel-work` only for leaf-level workers within their owned scope
- they must not commit or push unless the user explicitly asked for that

Require every worker to return the shape in [reference.md](reference.md).

### 6. Integrate centrally

The orchestrator owns integration. Workers do not integrate each other.

The orchestrator must:

- inspect each worker result and changed-file list
- inspect each worker slice folder, not just the final prose summary
- inspect `slices.json` and ensure every slice has an honest terminal or active state
- reconcile naming, docs, tests, and architectural seams across slices
- ensure user-facing docs were updated for user-visible behavior, not only RFC text or release notes
- verify the repo version baseline again before finish and bump `-dev.N` by one at minimum for implementation work on the active dev line
- update RFC progress state and checklist items as phases land
- move the accepted work into the orchestrator worktree cleanly
- run the repo-level gate
- run **Plan -> Do -> Check -> Act** on the integrated result:
  - **Plan**: confirm the combined slice outputs still satisfy the original end-state and create/update orchestrator task state in `.agents/state/ralph-loop/overview.md`
  - **Do**: integrate the accepted worker results
  - **Check**: run verification plus `review` or `review-orchestrate` on the integrated result
  - **Act**: run `fix` on actionable in-scope findings, or go back to planning if integration review shows that the original requested scope is still not satisfied
- invoke `flag-compiler-bug` for real out-of-scope compiler defects found during integration
- repeat until no actionable integrated items remain and the original scope is honestly satisfied

The orchestrator worktree's `.agents/state/review-report.md` is the integration source of truth.
The orchestrator's `.agents/state/ralph-loop/overview.md` is the integration state source of truth.

`STEERING.md` must be checked at the start of every major iteration. If it changes the priority, scope, or urgency of the work, the orchestrator must update `slices.json`, affected `scope.md` / `tasks.json`, and continue from the new direction rather than pretending the original ordering still applies.

Do not stop at "worker green." Cross-slice regressions and consistency problems belong to the orchestrator.

### 7. Finish with ship-ready artifacts

When code is ready:

- produce a concise done summary
- draft the commit message with `write-commit-message`
- draft the PR description with `create-pr-description`

For RFC-driven work, only the parent loop drafts or owns the final PR description. Child loops must not produce PRs of their own.

If the user says "greenlight for PR", "green light for PR", or otherwise approves PR publication for RFC-driven work, treat RFC finalization as part of the pre-PR gate. Before reporting the branch ready for review or opening/marking a PR ready:

- run a final scope check against the governing RFC and issue
- if the implemented branch covers the full RFC scope, use `bump-rfc` to move the RFC from `In Progress` to `Implemented`
- ensure every `Progress Checklist` item is checked before the bump; if an item is incomplete, do not claim full RFC scope
- ensure the PR body includes a closing keyword for the RFC issue, such as `Closes #NNN`
- rebuild docs after moving the RFC into `closed/implemented/`
- report any reason the RFC cannot be bumped before asking for human review

Do not leave a fully implemented RFC in `In Progress` while presenting the PR as review-ready. If the branch implements only part of the RFC, say that directly and leave the RFC active.

Only run the actual commit / push / PR creation flow if the user explicitly asked for those actions.

## Quality bar

Treat these as default expectations, not optional polish:

- full requested scope, not a narrow interpretation that happens to pass tests
- Boy Scout cleanup within touched files
- tests proportional to risk and surface area
- architectural fit with existing boundaries
- user-facing docs and release notes when the repo rules require them
- version checks up front and a dev-version bump (`-dev.N` -> `-dev.(N+1)`) at minimum for implementation work on the active dev line

If the task teaches a durable lesson about orchestration, testing, or worktree hygiene, consider `add-learning` before finishing.

## Relationship to other skills

- `start-work`: use once to resolve context and branch strategy before decomposition
- `review-rfc`: use first in RFC intake mode
- `bump-rfc`: use to move a settled RFC into `Planned` and then `In Progress` before phase execution, and to move fully implemented RFC-driven work to `Implemented` before PR review/publication; stop and ask the user if open questions or blockers remain
- `create-plan`: each worker uses it for its own slice; the orchestrator may also use it if the whole task still needs a settled plan
- `orchestrate-parallel-work`: use it for decomposition, worker ownership, and isolation
- `review`: report-only detector for smaller worker slices and local integration checks
- `review-orchestrate`: preferred detector for broad integrated outputs or slices that are themselves wide enough to justify specialized reviewers
- `fix`: mandatory repair pass after review findings
- `review-and-fix`: allowed as a convenience wrapper when a worker or the orchestrator wants the combined loop explicitly
- `write-commit-message`: use for the final commit text
- `create-pr-description`: use for the final PR body

## Nesting rule

Use this shape:

- parent `ralph-loop`
- child `ralph-loop`s for major RFC phases when justified
- `orchestrate-parallel-work` leaf workers inside a child phase when needed

Do not recurse `ralph-loop` indefinitely. A child loop is a phase owner, not another top-level orchestrator.

## Validation checklist

- [ ] If the input was an RFC, `review-rfc` ran first
- [ ] If the input was an RFC, unresolved questions and process blockers were surfaced to the user before bumping
- [ ] If the input was an RFC, `bump-rfc` moved it to `In Progress` only after design was settled and work was actually being picked up
- [ ] If the input was an RFC, child loops were derived from implementation phases or checklist groups
- [ ] Child loops did not spawn further `ralph-loop` children
- [ ] Scope was restated and confirmed before execution
- [ ] Backend choice was explicit
- [ ] RFC lifecycle state and implementation plan/checklist were confirmed before coding started
- [ ] Every implementation worker had a clean worktree and non-overlapping ownership
- [ ] Every implementation worktree lived under `/Users/danny/Development/encero/tmp`
- [ ] Every slice had a durable folder under `.agents/state/ralph-loop/<slice-id>/`
- [ ] The orchestrator maintained `.agents/state/ralph-loop/slices.json`
- [ ] The orchestrator checked `.agents/state/ralph-loop/STEERING.md` at each major iteration
- [ ] Every slice kept explicit `scope.md`, `plan.md`, `tasks.json`, `tasks.md`, `status.md`, and `handoff.md`
- [ ] Docs/version baseline was established from repo source-of-truth metadata before implementation
- [ ] Every worker ran a real plan -> do -> check -> act loop
- [ ] Every slice used only the allowed explicit states: `planned`, `doing`, `checking`, `replan_required`, `blocked`, `done`
- [ ] Scope failures were routed back to planning instead of being treated as ordinary defect cleanup
- [ ] The orchestrator ran its own integration plan -> do -> check -> act loop
- [ ] Every worker maintained `.agents/state/review-report.md` in its worktree
- [ ] The orchestrator maintained `.agents/state/review-report.md` in the integration worktree
- [ ] The orchestrator maintained `.agents/state/ralph-loop/overview.md`
- [ ] User-visible behavior changes updated authored user docs, not only RFCs/release notes
- [ ] Active dev version was re-checked and bumped by one dev increment at minimum for implementation work
- [ ] Child loops did not draft PRs or final commit artifacts of their own
- [ ] Final gate passed or remaining failures were reported concretely
- [ ] If the implementation satisfies the full RFC, `bump-rfc` moved it to `Implemented` before the PR was presented as review-ready
- [ ] Commit/PR artifacts were drafted
- [ ] No commit/push/PR action was taken without explicit user permission
