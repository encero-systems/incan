---
name: fleet-audit
description: Report which local git worktrees under tmp/ are safe to close, need a human look, or are explicitly protected. Use when the user says /fleet-audit, asks what worktrees can be cleaned up, or wants a status check across all Ralph-loop/spike worktrees before running closeout on any of them.
---

# Fleet Audit — Incan Worktrees

## Purpose

`/fleet-audit` gives a read-only, fleet-wide status report across every worktree under the shared `tmp/` root — something `closeout` deliberately does not do, since `closeout` only acts on one PR/branch at a time. This skill never removes, moves, or modifies anything. It exists to tell you (or an orchestrator deciding what to hand to `closeout`) what's actually going on before anyone touches a worktree.

## When to use this vs `closeout`

- Use `fleet-audit` first, to see the whole picture.
- Use `closeout` second, worktree by worktree, only on things `fleet-audit` reported as `safe-to-close` — and still verify PR-merged status yourself per `closeout`'s own rules before removing anything.
- Never use `fleet-audit`'s output as authorization to force-remove anything. It classifies; it does not approve deletion.

## How it works

Runs `~/Development/encero/.agents-state/scripts/fleet_audit.py`, which:

1. Lists every worktree via `git worktree list --porcelain`.
2. Skips (reports as `protected`) any worktree with a `.agents/state/KEEP` marker file — that marker means a human already decided this one is not up for evaluation, full stop.
3. For everything else, checks: branch ancestry against `origin/main`, matching GitHub PR state via `gh pr list` (catches squash/rebase merges the ancestor check alone would miss), uncommitted **tracked** file edits specifically (not just untracked build junk), and Ralph-loop slice status via the same parsing Codex's session hook already uses (`~/.codex/hooks/ralph_state_impl.py`'s `summarize_state`).
4. Classifies each as `protected`, `safe-to-close`, or `needs-review`, with a one-line reason.

## Workflow

1. Run:

   ```bash
   python3 ~/Development/encero/.agents-state/scripts/fleet_audit.py --repo-root . --repo-slug encero-systems/incan
   ```

   If that script or the private workspace state root (`~/Development/encero/.agents-state/`) doesn't exist on this machine, say so plainly and stop — do not attempt to reimplement the logic inline.

2. Present the report grouped by status, exactly as the script outputs it. Do not editorialize the `protected` group — those are closed questions.
3. For `safe-to-close` entries, note that the next step is a human-approved `closeout` run per worktree, not automatic removal.
4. For `needs-review` entries, summarize the *reason* field per worktree so the user can decide quickly (tracked edits vs. non-terminal loop state vs. no signal at all) rather than re-reading the whole report.
5. If asked to mark something as protected on the spot, write a one-line reason to `<worktree>/.agents/state/KEEP` (create `.agents/state/` first if it doesn't exist) — do not mark something protected without a reason.

## Output format

```md
## Fleet Audit — <date>

### Protected (N)
- <worktree>: <reason>

### Safe to close (N)
- <worktree> (branch): <reason> — run /closeout to remove

### Needs review (N)
- <worktree> (branch): <reason>
```
