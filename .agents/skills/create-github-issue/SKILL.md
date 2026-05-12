---
name: create-github-issue
description: Drafts a GitHub issue title and body using the target repository's issue templates under .github/ISSUE_TEMPLATE. Use when the user asks to create, draft, or file a GitHub issue, bug report, feature request, chore, documentation issue, or RFC proposal, or wants issue text that matches the repo's template.
---

# GitHub issue draft (repository templates)

## Workflow

1. **Identify the repository root** — Use the path the user gives, or infer from context (e.g. `incan/`, `InQL/`). If unclear, ask which repo the issue is for.

2. **Discover templates** — List `.github/ISSUE_TEMPLATE/*.yml` in that repository. **Ignore `config.yml`** (it only configures the template picker, not form fields).

3. **Choose the template** — Match the user's intent to a file by reading each YAML's `name` and `description`:
   - Bug → usually `bug_report.yml`
   - Feature → `feature_request.yml`
   - Refactor / CI / deps / cleanup → `chore.yml`
   - Docs → `documentation.yml`
   - RFC / large language or tooling change → `rfc_proposal.yml`
   If multiple fit or none fit, list the available `name` values and ask the user to pick.

4. **Read the selected YAML** — Parse the form from disk. Do not assume field names or area dropdowns match another repository; **InQL, Incan, and other repos differ** (e.g. "Area" options).

5. **Gather facts from the user or codebase** — Reproduction steps, expected vs actual, versions, logs, links to files/RFCs, acceptance criteria. If the user is reporting work discovered while coding, use file paths, commands run, and error text from context.

6. **Produce the draft** — See [Output format](#output-format). For YAML `body` block semantics (markdown vs textarea vs dropdown vs checkboxes), use [reference.md](reference.md).

7. **Optional: related PR or branch** — If the issue tracks follow-up work, mention the branch or PR link in the body where the template has a freeform section.

## Fallbacks

- **No `*.yml` forms** — Use `.github/ISSUE_TEMPLATE/*.md` if present; mirror its headings and guidance.
- **No templates at all** — Use a minimal structure: **Summary**, **Steps / context**, **Expected vs actual** (bugs) or **Problem**, **Proposal**, **Acceptance criteria** (features), and state that the repo has no issue templates.

## Output format

Return markdown the user can paste into GitHub (blank issue or “Open a blank issue”), plus metadata lines:

1. **Suggested title** — One line. Use the YAML `title` prefix when present (e.g. `bug - short subject`).
2. **Labels** — If the YAML defines `labels:`, list them for the user to add in the GitHub UI.
3. **Issue type** — If the YAML defines `type:` (e.g. Bug, Feature), mention it for GitHub’s issue type field when applicable.
4. **Body** — For each form block in order:
   - Render `markdown` blocks as-is.
   - Render `textarea` / `input` / `dropdown` / `checkboxes` as `## {label}` sections with filled content (see [reference.md](reference.md)).

Use complete sentences. Do not leave required sections empty without calling that out.

## Examples

### Example: Bug (conceptual)

**User:** "Draft an issue for incan: the compiler crashes on empty match arms."

**Steps:** Open `incan/.github/ISSUE_TEMPLATE/bug_report.yml`, map Area / Summary / Reproduction / Output / Environment.

**Fragment of output:**

```markdown
**Suggested title:** bug - Compiler panic on empty match arms

**Labels:** bug

## Area

- Compiler (frontend/backend/codegen)

## Summary

Expected: Typechecker or parser should report a clear diagnostic for empty match arms.
Actual: Compiler panics with ...

## Reproduction steps

1. Create `repro.incn` with ...
2. Run `incan build repro.incn`
...
```

### Example: Feature (conceptual)

**User:** "Feature request for InQL: add substrait export helper."

**Steps:** Use `InQL/.github/ISSUE_TEMPLATE/feature_request.yml` (or the repo’s equivalent). Fill Problem statement, Proposed solution, Alternatives, Scope — using InQL-specific areas from that file’s dropdown.

## Quality checklist

- [ ] Template file was read from the **correct repo**; `config.yml` was not used as the form.
- [ ] Dropdown and checkbox options match **that file’s** YAML, not another project’s.
- [ ] Required sections are filled or explicitly flagged as missing.
- [ ] Title prefix and labels match the YAML when present.
