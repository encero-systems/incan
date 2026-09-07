---
name: start-work
description: Prepare to work on a GitHub issue or RFC. Use when the user says /start-work, asks to start an issue, begin an RFC implementation, or pick up a task. Creates the branch, gathers context, and checks learnings.
---

# Start Work — Incan Project

## Input

The user provides one of:

- A GitHub issue number (e.g. `#165`, `165`)
- A GitHub issue URL (e.g. `https://github.com/dannys-code-corner/incan/issues/165`)
- An RFC number (e.g. `RFC 031`)
- A free-text description of the task

If none is provided, ask the user what they want to work on.

---

## Git commits

**Commit your own work.** Use the repo's message convention (`chore|bugfix|feature - <issue_id(s)> <description>`), push the branch, and open the PR when the work is ready. Do not leave finished work uncommitted waiting for the maintainer.

Still off-limits on your own initiative:

- anything that overwrites or deletes uncommitted work — `git checkout -- <path>`, `git restore <path>`, `git clean`, `git reset --hard`, `stash drop`
- force-pushing a shared branch, or any branch whose PR has already merged
- `git rebase` of a branch that is already pushed — sync it with a merge commit instead, so ancestry survives and no force-push is needed

Two habits keep pushing safe. Re-check a PR's state immediately before pushing to its branch: a squash-merge silently strands anything pushed afterwards. And prove work reached the integration branch by content (`git show origin/<dev-line>:<file> | grep <symbol>`), never by PR status — a stacked PR reports `MERGED` once its own base absorbs it, which says nothing about the dev line.

This policy applies whenever this skill is used (and is the default for Incan work even without `/start-work`).

---

## Workflow

### Step 1: Fetch issue/RFC context

**If an issue number or URL was given:**

```bash
gh issue view <NNN> --repo dannys-code-corner/incan
```

Extract: title, labels, body, linked RFC (if any).

**If an RFC number was given:**

- Read the RFC file: `workspaces/docs-site/docs/RFCs/<NNN>_*.md`
- Look for a linked GitHub issue in the `Issue:` header field.
- If an issue exists, also fetch it with `gh issue view`.

**If a free-text description was given:**

- Search for a matching open issue: `gh issue list --repo dannys-code-corner/incan --search "<description>" --state open`
- If a match is found, confirm with the user. If not, proceed without an issue link and note that one should be created.

### Step 2: Determine branch name

Construct the branch name using the convention: `<type>/<issue>-<slug>`

**Type** is determined by issue labels:

| Label                           | Type      |
| ------------------------------- | --------- |
| `feature`, `RFC`, `enhancement` | `feature` |
| `bug`                           | `bugfix`  |
| anything else (or no issue)     | `chore`   |

**Issue** is the GitHub issue number. If no issue exists, omit the number prefix.

**Slug** is derived from the issue title or RFC title:

- Lowercase
- Replace spaces and special characters with hyphens
- Truncate to ~50 characters at a word boundary
- For RFC implementations, prefer the pattern: `implement-rfc-<NNN>-<short-title>`

Examples:

- Issue #165 "Implement RFC 031: Library System Phase 1" with label `feature` -> `feature/165-implement-rfc-031-library-system-phase-1`
- Issue #88 "Vocab drift guardrails" with label `chore` -> `chore/88-vocab-drift-guardrails`
- Issue #42 "Parser crash on empty match" with label `bug` -> `bugfix/42-parser-crash-on-empty-match`

### Step 3: Create and checkout the branch

```bash
# Ensure main is up to date
git fetch origin main

# Create branch from origin/main
git checkout -b <branch-name> origin/main
```

If the branch already exists locally or on the remote, ask the user whether to:

- Check out the existing branch (`git checkout <branch-name>`)
- Delete and recreate it from main

### Step 4: Check learnings

Read `.agents/learnings.md` and check whether any section is relevant to the task. Specifically:

- If the task involves **lowering, emission, or codegen regressions** -> read `General pipeline pitfalls` and `Testing strategy`
- If the task involves **Rust interop, `import rust.*`, `rusttype`, or extern functions** -> read `RFC 041 (first-class Rust interop) implementation notes` and `Generic bounds and extern functions`
- If the task involves **stdlib, soft keywords, or `std.*` imports** -> read `Stdlib and registry patterns`
- If the task involves **imports, parser bracket handling, warnings, or formatter** -> read `Parser and lexer patterns` and `Wiring: CLI and LSP`
- If the task involves **docs, release notes, or RFC movement/renames** -> read `Docs and RFC tooling`

If a relevant section exists, summarize the key takeaways for the user.

### Step 4a: Record v0.6 ledger start

If the resolved issue belongs to the `0.6 Release` milestone, read the current ledger and update format on [#1074](https://github.com/encero-systems/incan/issues/1074). Before the first production edit, publish an append-only `Active` ledger update naming:

- the owning issue and branch (or planned branch);
- the bounded scope and acceptance contract;
- known dependencies and blockers; and
- the first targeted verification command or evidence lane.

Do not edit another contributor's ledger comment. If the task does not authorize a GitHub write, show the exact update text to the maintainer and wait for publication before editing production code.

When work becomes blocked, ready for integration, materially rescaled, or complete, publish the corresponding follow-up update on #1074. Completion updates must name actual verification evidence; never infer it from a green legacy path or an unrun command.

### Step 5: Check for parallel work opportunities

If the task clearly decomposes into independent slices and the user explicitly wants delegation or parallel work, stop after gathering context and hand off to `orchestrate-parallel-work`.

Do not improvise ad hoc multi-agent coordination inside this skill. This skill is for task setup, not swarm orchestration.

### Step 6: Draft the initial acceptance contract

If the issue, RFC, or task touches milestone scope, release scope, compiler boundaries, package imports, vocab, formatter, test runner, generated Rust, Rust metadata, or downstream-facing behavior, draft the initial acceptance contract before proposing next steps.

The contract should name:

- direct/local behavior that must work,
- import, reexport/facade, package-consumer, dependency-owned type, test-batch, vocab, formatter, generated-Rust, or Rust-metadata boundaries that can observe the behavior,
- downstream acceptance lanes such as IncQL when relevant,
- docs/generated-reference/rustdoc/release-note gates,
- performance or progress-output expectations when relevant.

For simple local tasks, say `acceptance contract: local only` and why no boundary lane applies.

### Step 7: Check for related RFCs

If the task references an RFC:

- Read the RFC document
- Check its status (Draft / Planned / In Progress / Done)
- If the RFC has a Progress Checklist, summarize what's done and what remains

### Step 8: Report to the user

Provide a concise summary:

```
## Ready to work

**Branch**: `<branch-name>` (created from `origin/main`)
**Issue**: #<NNN> — <title>
**RFC**: RFC <NNN> — <title> (status: <status>)
**Relevant learnings**: <list or "none">
**Acceptance contract**: <boundary/downstream/doc/perf gates, or "local only" with reason>

### Context
<1-3 sentence summary of what the task involves>

### Next steps
<Suggested first actions based on the issue/RFC>

**Proposed commit message**: `<one line; include in this same summary for the maintainer to use when they commit>`
```

---

## Edge cases

- **No GitHub CLI (`gh`)**: Fall back to reading the RFC file directly. Note that the issue could not be fetched and ask the user for context.
- **Dirty working tree**: uncommitted changes you did not make belong to the maintainer. Report them before switching branches and let them decide; never discard or revert them.
- **Branch already exists with divergent history**: Always ask before overwriting.
