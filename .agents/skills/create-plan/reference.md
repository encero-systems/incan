# Plan template (copy-paste)

Replace bracketed sections. Remove sections that do not apply. Point **Gate** at the target repo’s real commands (see SKILL.md).

```markdown
# Plan: [short title] (#issue)

## Goal / root cause

[1–3 sentences]

## TDD

### 1. Red

- [ ] Add failing test: [path or new file]
- [ ] Confirm failure with: `[repo test command + filter]`

### 2. Green

- [ ] Implement in: [paths]

### 3. Refine

- [ ] Update goldens/snapshots per repo docs (e.g. `INSTA_UPDATE=1 ...`)

## Documentation

- [ ] Release notes: [path] — [section] bullet + #issue
- [ ] Other docs: [path or none]
- [ ] `mkdocs build --strict` from [docs root] if applicable

## Files to touch (expected)

| File | Change |
|------|--------|
| `...` | ... |

## Gate

| Step | Command |
|------|---------|
| Format | `make fmt` (Incan) / `make fmt-check` (InQL) / … |
| Full gate | `make pre-commit` (Incan) / `make ci` (InQL) / … |
| Smoke | `make smoke-test` (Incan) / … |

## Success criteria

- [ ] Tests and docs gates pass
- [ ] All Gate commands succeed

## Suggested commit message (maintainer)

`[type](scope): [imperative summary] (#NNN)`
```
