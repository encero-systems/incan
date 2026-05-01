---
name: closeout
description: Safely close out completed Incan work after a PR is merged by verifying merge status, syncing the base branch, removing task-owned local worktrees/assets, deleting merged local/remote branches, pruning refs, and reporting dirty or ambiguous leftovers. Use when the user says /closeout, asks to clean up after a merged PR, or wants local RFC/issue branch/worktree cleanup.
---

# Closeout — Incan Project

## Purpose

`/closeout` cleans up local implementation state after a PR has merged. It is for removing task-owned worktrees, local branches, remote topic branches, stale refs, and temporary agent artifacts without touching unrelated user work.

This skill is intentionally conservative. A clean closeout is less important than not deleting active work.

## Inputs

The user may provide:

- A PR number or URL.
- A branch name.
- An issue/RFC number.
- A worktree path.
- A free-text instruction such as "close out the RFC 028 branch".

If no target is provided, infer it from the current branch, current worktree path, recent PR context, or task-owned temp directory names. If inference is ambiguous, ask the user which PR or branch to close out.

## Hard rules

- **Merge first.** Do not remove branches, remote refs, or worktrees until the PR is confirmed merged into its base branch.
- **Inventory first.** List candidate branches, worktrees, untracked files, and agent artifacts before removing anything.
- **Preserve user work.** Never delete a dirty worktree, branch with unmerged commits, or untracked files that are not clearly task-owned or explicitly declared disposable by the user.
- **Use git-safe deletion.** Prefer `git worktree remove`, `git branch -d`, and `git push origin --delete <branch>` over filesystem deletion.
- **No broad destructive cleanup.** Do not use `git reset --hard`, `rm -rf`, or `git clean -fd` as a default cleanup tool.
- **No force unless explicit.** `git branch -D`, forced worktree removal, and deleting ambiguous assets require direct user confirmation or an unambiguous closeout instruction that task-owned scratch worktrees should go.
- **Do not delete the active worktree.** If the current directory is the worktree being removed, switch command context to the main repository or another safe parent first.
- **Respect unrelated dirt.** Existing dirty files in the main repo or unrelated worktrees are blockers for those paths only; report them, do not "fix" them.
- **Ralph worker worktrees are scratch assets.** After the orchestrator PR is merged, task-owned worker worktrees created for that implementation are disposable when the user asks to close out the task. Dirty status in those worker slices should trigger an inventory and confirmation path, not a permanent stop.

## Workflow

### Step 1: Orient

Collect the current repository state:

```bash
git rev-parse --show-toplevel
git status --short --branch
git branch --show-current
git remote -v
git worktree list --porcelain
```

If the task references a PR, fetch its metadata through the GitHub connector when available. If using `gh`, be prepared for invalid local auth and fall back to GitHub connector data or plain git merge checks.

Record:

- repo root,
- current branch,
- target branch,
- target PR,
- base branch,
- local worktree path(s),
- remote tracking branch,
- any dirty files.

### Step 2: Verify the PR is merged

Use the strongest available source:

1. GitHub PR metadata: PR state is merged, with base branch and head branch recorded.
2. Git ancestry after fetching: the target branch tip is an ancestor of the base branch.
3. Local merged-branch status: `git branch --merged <base>` shows the target branch.

Recommended git fallback:

```bash
git fetch origin <base-branch> <target-branch>
git merge-base --is-ancestor <target-branch> origin/<base-branch>
```

If the PR is open, closed-unmerged, unknown, or the branch is not an ancestor of the base branch, stop after reporting status. Do not clean up.

### Step 3: Sync the base branch

Once merged, update the base branch in the main worktree if it is clean:

```bash
git fetch origin --prune
git switch <base-branch>
git pull --ff-only
```

If the base worktree is dirty, skip the switch/pull and report the dirty files. Cleanup of separate task worktrees can still proceed if those task worktrees are clean and merge status is confirmed.

### Step 4: Build the cleanup inventory

Identify only task-owned candidates:

- worktrees whose path matches the task branch, issue, RFC, or known temp worktree path,
- worker worktrees created for the same task by `ralph-loop` or `orchestrate-parallel-work`,
- local branches matching the merged PR head branch,
- local worker branches for the same task,
- remote branch `origin/<target-branch>` for the merged PR,
- local agent state created inside the task worktree,
- ignored or untracked generated artifacts that clearly belong to the task.

Use dry-run commands for asset cleanup:

```bash
git clean -nd
git status --short --ignored
```

Do not treat every untracked file as disposable. RFC drafts, docs changes, local notes, snapshots, and generated assets in the main worktree may be intentional user work.

### Step 5: Remove clean task worktrees

For each candidate worktree:

1. Check status inside that worktree.
2. Confirm it is tied to the merged target branch.
3. Ensure no other branch is actively checked out there.
4. Remove it with git:

```bash
git -C <main-repo> worktree remove <worktree-path>
```

If removal fails because the worktree is dirty, do not force-remove it. Report the path and dirty files.

Exception: for task-owned worker worktrees created as scratch slices by `ralph-loop` or `orchestrate-parallel-work`, dirty status is expected after the merged orchestrator branch has integrated the work. If the user has confirmed closeout or says those workers should go, remove them with:

```bash
git -C <main-repo> worktree remove --force <worker-worktree-path>
```

Record the removed worker path and branch in the closeout report.

### Step 6: Delete merged branches

Delete the local branch only after no worktree is using it:

```bash
git branch -d <target-branch>
```

Delete the remote topic branch only when it is the PR head branch and the PR is merged:

```bash
git push origin --delete <target-branch>
```

If branch deletion reports unmerged commits, stop and report the exact git output. Do not use `-D` unless the user explicitly confirms that those commits are disposable.

For confirmed disposable worker branches whose worktrees were force-removed as scratch assets, delete with:

```bash
git branch -D <worker-branch>
```

Use this only for branches that match the closed task and were created as local worker slices, not for arbitrary feature branches.

### Step 7: Clean task-local artifacts

Prefer worktree removal to manual file deletion. If artifacts remain outside the removed worktree, delete only files that are all of:

- untracked or ignored,
- clearly generated by the task,
- not in a shared state location used by unrelated work,
- shown to the user in the inventory.

For ambiguous `.agents/state/*`, temp directories, or generated docs assets, ask before deletion unless they are inside the removed task worktree.

### Step 8: Verify cleanup

Run:

```bash
git worktree list
git branch --list <target-branch>
git branch -r --list origin/<target-branch>
git status --short --branch
```

If the base worktree was updated, also verify it is on the expected branch and clean except for pre-existing unrelated user changes.

## Output format

Report the closeout as:

```md
## Closeout — <PR / branch / task>

### Removed
- <worktree / branch / remote ref / artifact>

### Kept
- <dirty or ambiguous item intentionally preserved>

### Verification
- `<command>` — <result>

### Blocked
- <only if something could not be safely cleaned>
```

If nothing was removed because the PR is not merged yet, say that directly and include the merge status source.

## Common stop conditions

Stop and ask or report status when:

- the PR is not merged,
- the PR cannot be identified confidently,
- the branch is not merged into the base branch,
- a candidate user-owned or unrelated worktree is dirty,
- local `gh` auth is invalid and no other reliable PR status source is available,
- the candidate branch name does not match the PR head branch,
- cleanup would touch unrelated user changes,
- only forced deletion would finish cleanup for anything other than confirmed disposable task worker slices.

## Escalation

If sandbox permissions block cleanup, request approval for the exact command and target. Do not request broad approval for destructive command families. Never suggest a persistent prefix rule for `rm`, forced deletion, or arbitrary shell execution.
