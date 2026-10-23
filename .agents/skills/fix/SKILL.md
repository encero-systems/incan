---
name: fix
description: Fix actionable in-scope findings in the current Incan worktree, usually after /review. Use when the user asks to fix review findings, clean up reported issues, apply in-scope review feedback, or says /fix.
---

# Fix — Incan Compiler

## Purpose

`/fix` is the repair half of the review workflow. It consumes concrete findings from `/review` or obvious unresolved worktree issues and fixes them directly when they are in scope and safe.

The primary source of truth is the persistent report in the current worktree:

- `.agents/state/review-report.md`

Use `/fix` when:

- `/review` already identified blockers or warnings,
- the user says "fix the findings",
- the user wants review cleanup without a fresh diagnostic pass.

Do not use `/fix` as a substitute for first-pass diagnosis when the problem is still unclear. Use `/review` first.

`/fix` is different from `/review`: it should consume the current report, not wipe it. If the report is stale or obviously belongs to a previous run, rerun `/review` instead of trying to repair against bad state.

## Workflow

1. Start from the current dirty worktree and the concrete findings already on record from `/review`, inline review comments, or the user's instructions.
2. Read `.agents/state/review-report.md` first when it exists.
3. For orchestrated reviews, treat `## Findings` as the default task list.
4. For generic single-review reports, fall back to `## Files` only if there is no top-level `## Findings` list.
5. Fix only findings that are:
   - in scope,
   - safe without extra user confirmation,
   - concrete enough to act on.
6. Do not widen scope. Avoid opportunistic refactors beyond what is needed to resolve the findings cleanly.
7. Update `.agents/state/review-report.md` as findings move between `open`, `fixed`, and `blocked`.
8. Append short timestamped entries to `## Activity` as you work. Log each major action instead of relying on a final prose recap.
9. After edits, run `make fmt`.
10. Run the smallest relevant contract selector (one case or area) from `workspaces/contracts/README.md`; run the affected whole families at commit points.
11. Run `make pre-commit-fast` (formatting, spelling, rustdoc, and skill-table checks).
12. If `make pre-commit-fast` fails for reasons unrelated to your fixes, keep the local fixes and report the failure as residual risk in the report file.
13. If findings remain unresolved, classify each as one of:
   - out of scope,
   - risky without user confirmation,
   - external blocker,
   - separate compiler bug.
14. Preserve findings even when they are non-blocking or design-oriented. `design-tension`, `maintainability`, and docs drift are still findings; they may simply move to a lower-priority fix queue instead of being dropped.

## Fixing rules

- Preserve the user's worktree. Never revert unrelated changes.
- Prefer small direct edits over broad rewrites.
- Use `incan inspect codegraph PATH --query defs TERM` or `--query callers TERM` for declaration and call relationships before grep.
- Extend an existing case before adding one; add a case only when no case in the same area can carry the behavior. The whole suite stays under about 3,000 tests. Never add a case per bug: name it in `# behavior:`. Each library case has one producer Loaf carrying all its declarations.
- Change design records only when a decision changes, in a sentence or two. Keep decisions, boundaries, consequences, and non-goals under about 1,000 words; mechanisms belong in commit messages and rustdoc, never a paragraph per commit.
- Run `make us-english-check` before every commit.
- Keep docs and code aligned. If you fix behavior, update user-facing docs that claim the old behavior. If behavior is out of scope, correct the docs instead of inventing implementation.
- For Rust prose comments, write natural paragraphs and let `make fmt` wrap them. Do not manually chop prose into short lines.
- For Markdown, keep prose as natural paragraphs. Do not introduce short-prosed or mechanically chopped text.
- Do not fix boundary bugs with local-only coverage. If a finding involves import, reexport/facade, package consumer, dependency-owned type, test batch, vocab/desugarer, formatter, generated Rust, Rust metadata, or downstream behavior, add the relevant boundary coverage or document why that boundary is impossible for this fix.
- Do not add a new split-brain workaround while fixing a split-brain bug. Prefer moving the decision into the canonical planner, metadata surface, registry, or shared helper that all relevant paths already consume.

## Verification

`/fix` should usually run:

- `make fmt`
- the smallest relevant contract selector during development, then affected whole families at commit points
- `make pre-commit-fast` (`make fmt-check`, `make us-english-check`, `make rustdoc-gate`, and `make agents-doc-sync`)

Use `incan run workspaces/contracts/src/main.incn -- INCAN workspaces/contracts/fixtures SELECTOR`, substituting the compiler under test and selector. Oven owns builds and execution; never run Cargo. Verify command names against the Makefile and `workspaces/oven/src/main.incn`.

If `make pre-commit-fast` fails because of unrelated sandbox or network issues, say so explicitly in `.agents/state/review-report.md`. That does not justify leaving local findings unfixed.

The report must remain a structured artifact. Do not replace it with a narrative summary. If `## Activity` is missing, repair the report before you finish. For orchestrated runs, the canonical report should stay thin and findings-driven; do not recreate a giant per-file checklist there.

When consuming an orchestrated report, update:

- `## Findings`
- `## Disagreements` when relevant
- `## Verification`

Do not invent new findings unless the edit work uncovers a tightly related issue needed to finish the requested fix cleanly.
Classification on findings is for handling, not suppression.

## Output format

Produce a concise report with:

```md
## Fix — <subject / branch / files>

### Fixed
<item> — <file>:<line> — <what changed>

### Verification
<command> — <result>

### Residual risk
<anything still unresolved, with classification>
```

If nothing needed fixing, say so directly.
