# Ralph Loop Reference

Use these templates to keep worker and orchestrator outputs consistent without bloating `SKILL.md`.

## Worker prompt contract

```md
You own one bounded slice of a larger Ralph-loop task.

Owned scope:
- <files / directories / modules>

Do not edit:
- <paths outside your scope>

Worktree:
- <absolute path>

Verification:
- <exact command>

Requirements:
1. Produce a short slice plan using `create-plan`.
2. Implement the slice end to end.
3. Run verification.
4. Run `review` on your slice.
5. Apply actionable findings that are in scope.
6. Re-run verification and `review` until no actionable in-scope items remain or you hit a real blocker.

Constraints:
- You are not alone in the repo.
- Do not revert others' work.
- Do not expand your scope.
- If you are a child loop under an RFC parent loop, do not draft a PR description, do not propose a standalone PR, and do not own RFC lifecycle edits.
- If you are a child loop and need more decomposition, use `orchestrate-parallel-work` only for leaf workers in your owned scope; do not spawn another `ralph-loop`.
- Do not commit or push unless explicitly told.

Return:
## Slice plan
- Goal:
- Owned scope:
- Verification:
- Risks/blockers:

## Slice result
- Changed files:
- Verification:
- Review loop summary:
- Open questions:
- Summary:
```

## Orchestrator integration checklist

```md
## Integration checklist
- If this is RFC-driven work, update the RFC's Progress Checklist as phases land.
- Confirm every worker stayed within scope.
- Read every worker's changed-file list before integrating.
- Normalize terminology, API names, and doc wording across slices.
- Re-run repo-level verification after integration.
- Run `review` on the combined output.
- Apply actionable findings.
- Repeat until no actionable integrated items remain.
- Draft commit message and PR description.
```

## Stop conditions

Worker stop condition:

- no remaining actionable blocker or warning within owned scope, or
- a concrete blocker remains that cannot be solved without changing scope

Orchestrator stop condition:

- integrated output passes the required gate
- integrated review has no remaining actionable items
- remaining risk, if any, is explicitly documented
