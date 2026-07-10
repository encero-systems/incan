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
- For milestone, RFC-wide, or compiler-boundary work, define the acceptance contract before implementation starts. The contract should name required boundary parity coverage, downstream acceptance lanes, docs/generated-reference gates, performance/progress gates, and any criteria that must be satisfied before an RC or publish step.
- For language or stdlib work that is meant to be implemented in Incan, dogfood Incan rather than creating a thin `.incn` facade over a custom Rust backend. Direct `from rust::` imports of existing crates/primitives are acceptable when the `.incn` module still owns the behavior; bespoke `incan_stdlib::feature` Rust modules that hide the feature logic are not acceptable unless the maintainer explicitly asks for that backend boundary.
- Do not let workers assume Incan cannot express something. Before choosing Rust/backend fallback or narrowing a design, workers must inspect current `.incn` precedents, parser/typechecker/codegen tests, or run a small probe and record the specific capability evidence.
- Every implementation must land in a fresh worktree rooted under `/Users/danny/Development/encero/tmp` so VS Code picks it up; do not implement in `/tmp` or in the main repo checkout.
- Treat each slice as a managed work packet with durable state on disk, not as a chat thread that has to remember its own scope.
- Treat loop identity as durable state too. A resumed loop must prove that persisted state matches the current user request before using it to choose work.
- Treat cross-repo consumer work as a verification lane unless the user explicitly puts that repository's implementation work in scope.
- Require every worker to plan, implement, verify, review, and fix within its owned slice.
- Consolidate accepted worker output onto the orchestrator branch before any commit, push, or PR work. Worker worktrees are temporary execution sandboxes, not final publication branches.
- Retire worker worktrees after their `done` output has been integrated and verified unless the orchestrator records a concrete reason to keep one.
- Require the orchestrator to perform its own explicit plan -> do -> check -> act integration loop after collecting worker output.
- Require workers and the orchestrator to maintain a persistent review report at `.agents/state/review-report.md` inside each worktree so findings survive across loops.
- Require touched `.incn` source to pass `review-incan-source-quality`; Incan implementation work is not done when it merely compiles if the authored source reads like Rust-shaped scaffolding instead of well-written Python.
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

## Scope and state isolation

Every Ralph loop must have a persisted loop identity before workers are spawned or resumed. Record it in the orchestrator overview and use it to filter durable state:

- `loop_id`: stable slug for this run, usually `<repo>-<issue-or-rfc-or-milestone>-<short-purpose>`
- target repository or repositories
- primary implementation repository
- base branch or resolved starting commit
- issue/RFC/milestone allowlist
- explicit non-goals
- permitted downstream or consumer verification lanes
- whether cross-repo implementation is allowed

Use `STATE_ROOT` to mean the active loop's state directory. For new loops, prefer a namespaced root:

```text
.agents/state/ralph-loop/<loop-id>/
```

Legacy top-level state such as `.agents/state/ralph-loop/slices.json` may be read for migration or handoff, but must not silently drive a new loop unless its recorded loop identity matches the current request.

On every resume, stop-hook continuation, compaction, or long-running restart:

1. Re-read the latest user instruction.
2. Re-read `STATE_ROOT/overview.md`, `STATE_ROOT/slices.json`, and `.agents/state/review-report.md`.
3. Compare persisted loop identity to the latest user instruction.
4. Classify each slice as:
   - `in_scope`: same loop identity and allowed implementation repo
   - `verification_lane`: downstream/consumer repo explicitly named as validation only
   - `out_of_scope`: different repo, issue, milestone, branch, or objective
5. Schedule only `in_scope` implementation slices.

Out-of-scope or historical slices may be cited as background evidence, but they must not be advanced, committed, pushed, or turned into PRs from the current loop. If the user wants to pick them up, start or resume a separate Ralph loop with its own loop identity.

Consumer repositories such as InQL may be used to reproduce, verify, or acceptance-test an Incan compiler/runtime change only when listed as a verification lane. Adding product features, docs CI, repo automation, or cleanup in that consumer repo is a separate implementation loop unless the user explicitly asks for it.

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
- primary implementation repository
- issue / RFC / branch context
- milestone allowlist, if milestone-driven
- explicit goals
- explicit non-goals
- downstream repositories that are verification lanes only
- cross-repo implementation permission, if any
- whether the task is RFC-wide or only a subset of RFC phases
- whether commit / push / PR creation were requested
- whether the user wants Codex subagents or an external backend such as OpenCode

If scope is ambiguous, stop and ask a short numbered list of missing decisions.

After confirming scope, write the loop identity into `STATE_ROOT/overview.md` before creating or resuming slices. If existing durable state lacks a matching loop identity, do not continue it as part of this loop.

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

Create durable slice state under each worktree. The paths below are relative to `STATE_ROOT`; for new loops `STATE_ROOT` should normally be `.agents/state/ralph-loop/<loop-id>`:

- `overview.md` for orchestrator-wide state, including loop identity and scope boundaries
- `STEERING.md` for operator overrides, urgency changes, and mid-run guidance
- `slices.json` for the orchestrator-owned slice registry
- `<slice-id>/scope.md`
- `<slice-id>/plan.md`
- `<slice-id>/tasks.json`
- `<slice-id>/tasks.md`
- `<slice-id>/status.md`
- `<slice-id>/handoff.md`

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

For milestone, RFC-wide, or compiler-boundary work, also write an `## Acceptance Contract` section into `STATE_ROOT/overview.md` before implementation starts. Include:

- the local/direct behavior that must work,
- import, reexport/facade, package-consumer, test-batch, dependency, vocab, or generated-Rust boundaries that can observe the behavior,
- downstream acceptance lanes such as InQL when the surface is exercised there,
- performance or progress-output expectations when the change touches prewarm, test runner, metadata, or CI paths,
- docs, rustdocs, generated-reference, release-note, or RFC lifecycle gates,
- explicit non-applicable boundaries, with a short reason.

Do not defer this acceptance contract to release-branch closeout. The release branch should verify the contract, not discover it.

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
    "loop_id": "incan-557-std-environ",
    "slice_id": "lifecycle-env",
    "repo": "encero-systems/incan",
    "scope_role": "implementation",
    "owner": "<worker name or id>",
    "worktree": "/Users/danny/Development/encero/tmp/...",
    "status": "doing",
    "next_action": "Implement T2"
  }
]
```

Use `scope_role: "verification"` for downstream acceptance lanes. Verification slices may run checks and collect evidence, but they must not mutate consumer product code unless the loop identity explicitly allows cross-repo implementation.

### 5. Give each worker a strict end-to-end contract

Each worker must own a non-overlapping slice with:

- loop identity
- repository and scope role (`implementation` or `verification`)
- exact goal
- owned files or directories
- explicit non-goals
- forbidden repositories or paths
- dedicated worktree path under `/Users/danny/Development/encero/tmp`
- dedicated slice folder under `STATE_ROOT/<slice-id>/`
- verification command
- expected result format

For RFC-driven work, prefer one child loop per implementation phase or tightly related checklist group.

Each worker must perform this loop inside its slice:

1. **Plan**
   - Build a short slice plan using `create-plan`.
   - For `.incn`, stdlib, or language-surface work, perform capability intake before ruling out an Incan-native design. Record the local precedent, test, or probe result in `plan.md`.
   - Write `scope.md`, `plan.md`, `tasks.json`, and `tasks.md`.
   - Break the slice into concrete tasks inside `tasks.json` and keep `tasks.md` as the readable companion.
   - Set initial slice state in `status.md` as `planned`.
2. **Do**
   - Implement the next planned task set, not the whole universe.
   - Update `tasks.json`, `tasks.md`, and `status.md` as task ownership and progress change.
   - Use `doing` as the active execution state.
3. **Check**
   - Run targeted verification for the slice.
   - Run `review-incan-source-quality` when the slice touches `.incn` source.
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
The slice folder under `STATE_ROOT/<slice-id>/` must also stay current throughout this loop.

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
- inspect `slices.json` and ensure every in-scope slice has an honest terminal or active state
- ignore or migrate historical slices whose `loop_id`, repo, branch, issue, milestone, or scope role does not match the current loop identity
- reconcile naming, docs, tests, and architectural seams across slices
- ensure user-facing docs were updated for user-visible behavior, not only RFC text or release notes
- verify the repo version baseline again before finish and bump `-dev.N` by one at minimum for implementation work on the active dev line
- update RFC progress state and checklist items as phases land
- move the accepted work into the orchestrator worktree cleanly
- retire each completed worker worktree after its accepted work is present in the orchestrator worktree and the integrated verification gate has passed
- run the repo-level gate
- run **Plan -> Do -> Check -> Act** on the integrated result:
  - **Plan**: confirm the combined slice outputs still satisfy the original end-state and create/update orchestrator task state in `STATE_ROOT/overview.md`
  - **Do**: integrate the accepted worker results
  - **Check**: run verification, `review-incan-source-quality` for touched `.incn` source, plus `review` or `review-orchestrate` on the integrated result
  - **Act**: run `fix` on actionable in-scope findings, or go back to planning if integration review shows that the original requested scope is still not satisfied
- invoke `flag-compiler-bug` for real out-of-scope compiler defects found during integration
- repeat until no actionable integrated items remain and the original scope is honestly satisfied

The orchestrator worktree's `.agents/state/review-report.md` is the integration source of truth.
The orchestrator's `STATE_ROOT/overview.md` is the integration state source of truth.

Worker cleanup is part of integration, not optional closeout. For every slice marked `done`:

1. Confirm `handoff.md`, the worker changed-file list, and the accepted patch are reflected in the orchestrator worktree.
2. Run the relevant integrated verification before removing the worker sandbox.
3. Record integration metadata in `slices.json`, such as `integrated_at`, `integrated_by`, and `worktree_removed`.
4. If the worker worktree is clean, remove it with `git worktree remove <worker-path>`.
5. If the worker worktree is dirty only because of files already integrated into the orchestrator branch or task-owned scratch state, record the inventory in `overview.md` or `slices.json`, then remove that Ralph-owned worker worktree with `git worktree remove --force <worker-path>`.
6. After the worktree is removed, delete the disposable worker branch only when it is task-owned and the orchestrator branch contains the accepted work.

Do not force-remove a worker worktree with unknown or unintegrated changes. Do not push while accepted slice work exists only in a worker worktree or worker branch.

`STEERING.md` must be checked at the start of every major iteration. If it changes the priority, scope, or urgency of the work, the orchestrator must update `slices.json`, affected `scope.md` / `tasks.json`, and continue from the new direction rather than pretending the original ordering still applies.

If `STEERING.md` or a resume hook mentions slices from another loop identity, treat them as out of scope until the user explicitly says to resume that separate loop.

Do not stop at "worker green." Cross-slice regressions and consistency problems belong to the orchestrator.

### 7. Finish with ship-ready artifacts

When code is ready:

- confirm all accepted worker slices have been consolidated into the orchestrator branch
- confirm completed worker worktrees are removed, or record a concrete reason for any kept worker worktree
- produce a concise done summary
- draft the commit message with `write-commit-message`
- draft the PR description with `create-pr-description`

For RFC-driven work, only the parent loop drafts or owns the final PR description. Child loops must not produce PRs of their own.

If the user says "greenlight for PR", "green light for PR", or otherwise approves PR publication for RFC-driven work, treat RFC finalization as part of the pre-PR gate. Before reporting the branch ready for review or opening/marking a PR ready:

- run a final scope check against the governing RFC and issue
- if the implemented branch covers the full RFC scope, use `bump-rfc` to move the RFC from `In Progress` to `Implemented`
- ensure every `Progress Checklist` item is checked before the bump; unchecked items are scope failures, not residual PR risks
- if any checklist item is incomplete, set the orchestrator state to `replan_required`, update the relevant slice/task state, and continue the Plan -> Do -> Check -> Act loop instead of publishing
- ensure the PR body includes a closing keyword for the RFC issue, such as `Closes #NNN`
- rebuild docs after moving the RFC into `closed/implemented/`
- report any reason the RFC cannot be bumped before asking for human review

Do not leave a fully implemented RFC in `In Progress` while presenting the PR as review-ready. Do not use `Refs #NNN` as an escape hatch for RFC-driven Ralph-loop work unless the user explicitly changed the scope to a partial implementation. If the branch implements only part of the RFC, say that directly, leave the RFC active, and do not present the PR as a full-RFC closeout.

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
- `review-incan-source-quality`: required detector for touched `.incn` source; use it to enforce the well-written-Python readability bar and dogfooding integrity before declaring a slice or integration clean
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
- [ ] Loop identity was written before workers were spawned or resumed
- [ ] Existing durable state was filtered by loop identity before choosing work
- [ ] Backend choice was explicit
- [ ] RFC lifecycle state and implementation plan/checklist were confirmed before coding started
- [ ] Every implementation worker had a clean worktree and non-overlapping ownership
- [ ] Every implementation worktree lived under `/Users/danny/Development/encero/tmp`
- [ ] Every slice had a durable folder under `STATE_ROOT/<slice-id>/`
- [ ] The orchestrator maintained `STATE_ROOT/slices.json`
- [ ] Every `slices.json` entry has `loop_id`, `repo`, and `scope_role`
- [ ] Consumer/downstream repositories were used only as verification lanes unless explicitly allowed for implementation
- [ ] The orchestrator checked `STATE_ROOT/STEERING.md` at each major iteration
- [ ] Every slice kept explicit `scope.md`, `plan.md`, `tasks.json`, `tasks.md`, `status.md`, and `handoff.md`
- [ ] Docs/version baseline was established from repo source-of-truth metadata before implementation
- [ ] Incan-language/stdlib slices recorded capability evidence before ruling out Incan-native implementation
- [ ] Every worker ran a real plan -> do -> check -> act loop
- [ ] Every slice used only the allowed explicit states: `planned`, `doing`, `checking`, `replan_required`, `blocked`, `done`
- [ ] Scope failures were routed back to planning instead of being treated as ordinary defect cleanup
- [ ] The orchestrator ran its own integration plan -> do -> check -> act loop
- [ ] Accepted worker output was consolidated onto the orchestrator branch before final artifacts
- [ ] Completed worker worktrees were removed, or each kept worker worktree has a recorded reason
- [ ] No accepted slice work exists only in a worker worktree or disposable worker branch
- [ ] Every worker maintained `.agents/state/review-report.md` in its worktree
- [ ] Every worker or orchestrator touching `.incn` source ran `review-incan-source-quality`
- [ ] The orchestrator maintained `.agents/state/review-report.md` in the integration worktree
- [ ] The orchestrator maintained `STATE_ROOT/overview.md`
- [ ] User-visible behavior changes updated authored user docs, not only RFCs/release notes
- [ ] Active dev version was re-checked and bumped by one dev increment at minimum for implementation work
- [ ] Child loops did not draft PRs or final commit artifacts of their own
- [ ] Every RFC `Progress Checklist` item was checked or the orchestrator stayed in `replan_required`
- [ ] Final gate passed or remaining failures were reported concretely
- [ ] If the implementation satisfies the full RFC, `bump-rfc` moved it to `Implemented` before the PR was presented as review-ready
- [ ] Full-RFC PR bodies use a closing keyword such as `Closes #NNN`; partial-scope PRs explain the remaining RFC scope instead
- [ ] Commit/PR artifacts were drafted
- [ ] No commit/push/PR action was taken without explicit user permission
