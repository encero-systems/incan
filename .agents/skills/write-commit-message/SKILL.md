---
name: write-commit-message
description: Draft commit titles and optional commit bodies using the workspace commit naming convention. Use when the user asks for a commit message, commit text, commit title, or wants commit wording in the format `chore|bugfix|feature - <issue_id(s)> <short description>`.
---

# Write Commit Message

## Workflow

1. Identify the repository and inspect the actual change set before drafting anything.
2. Infer the change type from the diff:
   - `bugfix` for correctness fixes and regressions
   - `feature` for new behavior or newly implemented surface area
   - `chore` for maintenance, docs-only changes, refactors without new behavior, or repository housekeeping
3. Infer issue ids from the branch name first, then from explicit issue references in the current task context if needed.
4. Write the title in exactly this format:

```text
<type> - <short description> (<#issue_id(s)>)
```

5. Keep the short description concrete:
   - specific to the actual diff
   - no trailing punctuation
   - no vague filler like `updates` or `misc fixes`
6. If the user also wants a commit body, write:
   - one short sentence on what changed
   - one short sentence on why
   - optional flat bullets only if there are multiple distinct changes worth calling out

## Issue Id Rules

- Use numeric ids joined by commas when there are multiple issues:

```text
bugfix - preserve method result types for locals bound from calls (#252, #255) 
```

- Do not prepend `#` unless the user explicitly asks for that style.
- If no issue id is discoverable, say so instead of inventing one.

## Output Rules

- If the user asks for a commit message or commit text, return the commit title by itself unless they also asked for a body.
- If the user asks for both, return the title first and then the body.
- Do not wrap the final title in commentary.
- If the requested type conflicts with the actual diff, follow the diff and say so.

## Examples

```text
bugfix - preserve something important for locals bound from calls (#42)
```

```text
feature - add Session builder and DataFusion backend seam (#5)
```

```text
chore - tighten RFC 008 optimizer boundary wording (#18)
```
