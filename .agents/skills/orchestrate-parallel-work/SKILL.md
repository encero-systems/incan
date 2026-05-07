---
name: orchestrate-parallel-work
description: Decompose a task into safe parallel sub-agent slices with explicit ownership, optional worktree isolation, per-worker plans, verification, and orchestrator-led integration. Use when the user explicitly asks for delegation, sub-agents, or parallel work and the task can be split into non-overlapping scopes.
---

# Orchestrate Parallel Work

## When to use

Use this skill only when both are true:

- The user explicitly asks for sub-agents, delegation, or parallel work.
- The task can be split into clean, non-overlapping ownership slices.

Good fits:

- Multiple RFCs that can be edited independently
- Separate modules or directories with disjoint write scopes
- Implementation, tests, and docs that can be owned separately
- Large issue work where one slice can proceed while another is being reviewed or integrated

Do not use this skill for:

- One tightly coupled design problem with shared unresolved decisions
- Small tasks where orchestration overhead exceeds the work
- Multiple workers editing the same files or the same API surface
- Work that still depends on first settling one blocking question locally

## Core principle

The orchestrator owns decomposition, dependency ordering, consistency, and integration.

Workers own bounded slices, produce their own small plan, execute within that scope, verify their slice, and report back. Workers do not invent scope, do not expand into adjacent areas, and do not integrate other workers' changes.

## Workflow

### Step 1: Decide whether parallelism is actually justified

Before spawning anything:

- Identify the end-state the user wants.
- Identify the next critical-path task that must stay local.
- Separate truly independent sidecar work from blocking work.

If the next local step depends on the result, keep that task local. Delegate only work that materially advances the goal without blocking the immediate next action.

### Step 2: Define slices with explicit ownership

For each worker, define:

- The exact goal
- Owned files, directories, RFCs, or modules
- Explicit non-goals and boundaries
- Required verification command
- Expected output format

Good ownership examples:

- RFC `066` only
- `crates/incan-parser/**` only
- docs under `workspaces/docs-site/docs/**` only
- tests under one named test module only

Bad ownership examples:

- "parser stuff"
- "all stdlib cleanup"
- any scope that overlaps another worker's write set

### Step 3: Choose isolation level

Use the lightest isolation that is honest about the risk:

- Context-only isolation is enough for read-heavy analysis or text-only RFC work with no overlap.
- Separate git worktrees are preferred for real parallel implementation work, especially when workers will edit code or run repo-local verification.

If using worktrees:

- Create one worktree per worker from the same base branch.
- Give each worker a distinct worktree path.
- Keep branch naming predictable and tied to the slice.

Do not pretend isolated agent context is the same thing as a git worktree when true branch or file isolation matters.

### Step 4: Spawn workers with a strict contract

Each worker prompt must include:

- Their owned scope
- The files or directories they may edit
- The files or directories they must not edit
- Whether they are in a dedicated worktree
- The verification command they must run
- The output they must return

Require every worker to return:

- `Plan`: 2-5 bullets for their slice
- `Changed files`: exact paths
- `Verification`: command run and result
- `Open questions/blockers`: only if still unresolved
- `Result`: concise summary of what changed

Workers must be told:

- they are not alone in the repo
- they must not revert others' work
- they must adapt to concurrent changes rather than overwrite them
- they must not commit or push unless the user explicitly asked for that

### Step 5: Keep orchestration disciplined

The orchestrator should:

- monitor progress without busy-waiting
- continue doing non-overlapping local work while workers run
- only wait when blocked on a worker result
- review returned changes before integrating them
- resolve cross-slice inconsistencies in terminology, contracts, and tests

Do not spawn a "watcher" review agent by default. A separate review agent is justified only when the integration risk is real, such as cross-cutting correctness, API consistency, or final review of a large merged result.

### Step 6: Integrate centrally

The orchestrator owns the final merge of outcomes:

- review worker outputs
- reconcile overlapping assumptions
- adjust wording, API names, or tests for consistency
- run any required top-level verification
- summarize final status for the user

Workers do not integrate each other.

## Worker plan template

Every worker should begin by producing a tiny plan in this shape:

```md
## Slice plan
- Goal:
- Owned scope:
- Verification:
- Risks/blockers:
```

And end in this shape:

```md
## Slice result
- Changed files:
- Verification:
- Open questions:
- Summary:
```

## Parallelization heuristics

Parallelize by:

- RFC file or proposal
- crate or package
- compiler stage when boundaries are already clean
- docs vs implementation vs tests
- disjoint top-level directories

Do not parallelize by:

- arbitrary percentage split
- shared API surfaces with unresolved naming or semantics
- "one worker codes, one worker reviews" before any concrete output exists

## Relationship to other skills

- Use `start-work` first when you need issue/RFC context, a base branch, or learnings.
- Use `create-plan` first when the main task still needs a settled implementation plan.
- Use `ralph-loop` when the user wants a heavier end-to-end loop with per-worker planning, review/fix iteration, orchestrator integration review, and final commit/PR drafting.
- Use this skill after that preparation, when the task is clearly decomposable.

If the task cannot be decomposed cleanly, do not force parallelism. Keep the work local.
