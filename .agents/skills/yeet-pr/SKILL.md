---
name: yeet-pr
description: Publish local Incan or Encero repository changes to GitHub using project standards. Use when the user asks to yeet, publish, commit and PR, push a branch and open a PR, or create a draft PR from local changes while requiring the repo-local commit-message and PR-description conventions.
---

# Yeet PR

Use this skill to publish a local branch without bypassing project standards. It wraps staging, committing, pushing, and opening a draft PR around the repository's own `write-commit-message` and `create-pr-description` skills.

## Required Standards

Before drafting a commit or PR body, read and follow these skills from the target repository when present:

- `.agents/skills/write-commit-message/SKILL.md`
- `.agents/skills/create-pr-description/SKILL.md`

If either skill is missing, stop and say which standard is unavailable instead of falling back to generic wording.

## Workflow

1. **Confirm the target repo and scope**
   - Run `git status -sb`.
   - Inspect changed filenames and enough diff to classify the work.
   - If the worktree contains unrelated changes, do not stage everything. Ask which files belong in the PR.
   - If there are existing staged changes, inspect `git diff --cached --name-only` and treat them as user-owned until proven in scope.

2. **Establish the branch and base**
   - Determine the current branch with `git branch --show-current`.
   - If on `main`, `master`, or the remote default branch, create a feature branch before committing.
   - Fetch the target base branch when possible.
   - If the branch is behind the base, prefer rebasing or merging before opening the PR. If conflicts occur, resolve them, rerun relevant checks, and mention the conflict resolution.

3. **Use the commit-message standard**
   - Follow `.agents/skills/write-commit-message/SKILL.md` exactly.
   - Infer issue IDs from the branch name first, then from explicit task context.
   - Use the resulting title as the commit subject.
   - Do not invent issue IDs. If none are discoverable, stop and ask for the intended issue or confirm an issue-less commit.

4. **Stage and commit intentionally**
   - Stage only in-scope files, preferably by explicit path.
   - Use `git add -A` only when the whole worktree is confirmed in scope.
   - Commit with the standards-compliant subject.
   - Do not amend or rewrite user commits unless the user explicitly asks.

5. **Verify after the final history shape**
   - If checks already ran before a rebase/merge, rerun at least focused checks after the rebase/merge.
   - Prefer the repo's documented gate when feasible. For Incan, use `make pre-commit` unless the user asked for a narrower publish.
   - If a check fails from sandbox or credential limitations, rerun with the required approval path or report the blocker clearly.

6. **Use the PR-description standard**
   - Follow `.agents/skills/create-pr-description/SKILL.md` exactly.
   - Locate the repository PR template.
   - Use the final diff against the chosen base branch and the final commit log.
   - Include RFC lifecycle handling when the diff touches RFC files.
   - Preserve the template checkboxes and closing issue references required by the skill.

7. **Choose the GitHub path**
   - Prefer local `git` for branch/commit/push operations.
   - Prefer the GitHub app/connector for PR creation, PR metadata, comments, labels, reviewers, and other GitHub API operations.
   - Do not make `gh` the default PR-creation path. In Codex sandboxed environments, the user's GitHub token is often not exposed to `gh`, so `gh auth status` can fail even when the GitHub connector is available and authenticated.
   - If `gh auth status` fails with an invalid/missing token inside the sandbox, treat that as a tooling limitation. Use the GitHub connector where possible instead of asking the user to re-authenticate.
   - Use `gh` only for local git-adjacent information that the connector cannot provide, or as a fallback after confirming `gh auth status` is valid in the current execution context.

8. **Push**
   - Push the current branch with tracking: `git push -u origin <branch>`.
   - If push fails because the remote branch moved, fetch and resolve rather than force-pushing by default.
   - Force-push only when the branch is task-owned and the user explicitly approved rewriting it.

9. **Open a draft PR**
   - Prefer the GitHub app/connector for PR creation once the branch is pushed.
   - Derive `repository_full_name` from `origin` and `head_branch` from the current branch.
   - Use the remote default branch as `base_branch` unless the user specified another target.
   - Use the standards-compliant PR title and body from the prior steps.
   - Create a draft PR by default. Mark ready only if the user explicitly asked for ready-for-review.
   - If the connector cannot create the PR, use `gh pr create --draft` only when `gh auth status` is valid in the current execution context; otherwise stop and report that PR creation is blocked by CLI auth while preserving the pushed branch.

## Safety Rules

- Never publish unrelated local changes silently.
- Never skip `write-commit-message` or `create-pr-description` because another GitHub skill has its own simpler wording.
- Never open a PR with a generic autogenerated body when the repo template exists.
- Never ask the user to run `gh auth login` merely because sandboxed `gh` cannot see a token when the GitHub connector can perform the needed GitHub operation.
- Never leave a command running after reporting completion.
- Report any dirty leftovers outside the committed PR scope.

## Final Report

End with:

- Branch name
- Commit SHA and subject
- PR URL
- Base branch
- Verification commands and results
- Any residual risks, dirty files, or auth/tooling fallbacks
