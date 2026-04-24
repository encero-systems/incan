---
name: ralph-loop
description: Run a high-thoroughness end-to-end implementation loop using clean worker worktrees, per-slice planning, implementation, review-and-fix iteration, orchestrator integration, and final commit/PR drafting. Use when the user explicitly wants a Ralph/Wiggum-style loop, autonomous multi-agent execution, a delegated feature/bugfix carried from confirmed scope through review-ready completion, or RFC-driven implementation that should be reviewed, bumped to In Progress, decomposed into phases, and executed through nested sub-loops. Supports Codex subagents by default and external CLI workers such as OpenCode when explicitly requested and available.
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
- Require every worker to plan, implement, verify, review, and fix within its owned slice.
- Require the orchestrator to perform its own integration review/fix loop after collecting worker output.
- Do not commit, push, or open a PR unless the user explicitly asked for that. When not explicitly asked, still draft the commit message and PR description as ready-to-use artifacts.

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

### 5. Give each worker a strict end-to-end contract

Each worker must own a non-overlapping slice with:

- exact goal
- owned files or directories
- explicit non-goals
- dedicated worktree path under `/Users/danny/Development/encero/tmp`
- verification command
- expected result format

For RFC-driven work, prefer one child loop per implementation phase or tightly related checklist group.

Each worker must perform this loop inside its slice:

1. Build a short slice plan using `create-plan`.
2. Implement the slice end to end.
3. Run targeted verification for the slice.
4. Run `review` on the slice.
5. Apply every actionable blocker or warning that is in scope.
6. If the slice hits a likely compiler bug that is out of scope, invoke `flag-compiler-bug` before reporting the blocker or choosing a workaround.
7. Re-run verification and `review`.
8. Repeat until no actionable in-scope items remain, or report a concrete blocker.

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
- reconcile naming, docs, tests, and architectural seams across slices
- ensure user-facing docs were updated for user-visible behavior, not only RFC text or release notes
- verify the repo version baseline again before finish and bump `-dev.N` by one at minimum for implementation work on the active dev line
- update RFC progress state and checklist items as phases land
- move the accepted work into the orchestrator worktree cleanly
- run the repo-level gate
- run `review` on the integrated result
- apply actionable findings
- invoke `flag-compiler-bug` for real out-of-scope compiler defects found during integration
- repeat review and verification until no actionable integrated items remain

Do not stop at "worker green." Cross-slice regressions and consistency problems belong to the orchestrator.

### 7. Finish with ship-ready artifacts

When code is ready:

- produce a concise done summary
- draft the commit message with `write-commit-message`
- draft the PR description with `create-pr-description`

For RFC-driven work, only the parent loop drafts or owns the final PR description. Child loops must not produce PRs of their own.

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
- `bump-rfc`: use to move a settled RFC into `Planned` and then `In Progress` before phase execution, but stop and ask the user if open questions or blockers remain
- `create-plan`: each worker uses it for its own slice; the orchestrator may also use it if the whole task still needs a settled plan
- `orchestrate-parallel-work`: use it for decomposition, worker ownership, and isolation
- `review`: mandatory for both worker slices and final integrated output
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
- [ ] Docs/version baseline was established from repo source-of-truth metadata before implementation
- [ ] Every worker ran plan -> implement -> verify -> review -> fix loops
- [ ] The orchestrator ran its own integration review/fix loop
- [ ] User-visible behavior changes updated authored user docs, not only RFCs/release notes
- [ ] Active dev version was re-checked and bumped by one dev increment at minimum for implementation work
- [ ] Child loops did not draft PRs or final commit artifacts of their own
- [ ] Final gate passed or remaining failures were reported concretely
- [ ] Commit/PR artifacts were drafted
- [ ] No commit/push/PR action was taken without explicit user permission
