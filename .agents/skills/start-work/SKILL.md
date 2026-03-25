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

Read `.cursor/agents/learnings.md` and check whether any section is relevant to the task. Specifically:

- If the task involves **field metadata, aliases, or model features** -> read the RFC 021 section
- If the task involves **Rust interop, `import rust.*`, or extern functions** -> read the RFC 005 section
- If the task involves **stdlib, soft keywords, or `std.*` imports** -> read the RFC 022 section
- If the task involves **imports, parser bracket handling, or formatter** -> read the Issue #116 section
- If the task involves **generics, trait bounds, or extern diagnostics** -> read the RFC 023 section

If a relevant section exists, summarize the key takeaways for the user.

### Step 5: Check for related RFCs

If the task references an RFC:

- Read the RFC document
- Check its status (Draft / Planned / In Progress / Done)
- If the RFC has a Progress Checklist, summarize what's done and what remains

### Step 6: Report to the user

Provide a concise summary:

```
## Ready to work

**Branch**: `<branch-name>` (created from `origin/main`)
**Issue**: #<NNN> — <title>
**RFC**: RFC <NNN> — <title> (status: <status>)
**Relevant learnings**: <list or "none">

### Context
<1-3 sentence summary of what the task involves>

### Next steps
<Suggested first actions based on the issue/RFC>
```

---

## Edge cases

- **No GitHub CLI (`gh`)**: Fall back to reading the RFC file directly. Note that the issue could not be fetched and ask the user for context.
- **Dirty working tree**: Warn the user about uncommitted changes before switching branches. Ask whether to stash, commit, or abort.
- **Branch already exists with divergent history**: Always ask before overwriting.
