# GitHub issue template mapping (YAML forms)

Repositories that use [issue forms](https://docs.github.com/en/communities/using-templates-to-encourage-useful-issues-and-pull-requests/syntax-for-issue-forms) store them under `.github/ISSUE_TEMPLATE/` as `*.yml` files.

## Files to skip

- **`config.yml`** — Chooses which templates appear in the picker; it is not a form.
- **`PULL_REQUEST_TEMPLATE*`** — Belongs to PRs, not issues.

## Top-level keys (useful for output)

| Key | Use |
|-----|-----|
| `name` | Human-readable template name (picker label). |
| `description` | Short blurb; helps pick the right template. |
| `title` | Default title **prefix** (e.g. `bug - `); suggest full title = prefix + concise subject. |
| `labels` | Optional list; tell the user to apply these when opening the issue. |
| `type` | Optional GitHub issue type (if the repo uses issue types). |

## `body` block types

For each item in `body:`:

| `type` | Output |
|--------|--------|
| `markdown` | Include `attributes.value` as normal markdown (intro, links, policy). |
| `textarea` / `input` | Section `## {label}` then filled prose. Use `description` as a hint for what to write. |
| `dropdown` | Section `## {label}` then chosen option(s). If `multiple: true`, use a bullet list. |
| `checkboxes` | Section `## {label}` then `- [x]` / `- [ ]` per option from user intent; respect `required` on options when the user confirms. |

If `validations.required: true` and the user gave no content, note that the section must be completed before submit.

## No YAML forms

If the repo only has `.github/ISSUE_TEMPLATE/*.md` (legacy templates), read the file and mirror its sections and any HTML comments that describe fields.

## Blank issues

If `blank_issues_enabled: true` in `config.yml`, a single markdown body built from the YAML is valid for a blank issue; remind the user to set title and labels manually to match the template.
