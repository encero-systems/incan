# Plan template (copy-paste)

Replace bracketed sections. Remove sections that do not apply. Point **Gate** at the target repo’s real commands (see SKILL.md).

```markdown
# Plan: [short title] (#issue)

## Goal / root cause

[1–3 sentences]

## TDD

### 1. Red

- [ ] Extend an existing case: [path]; add one only if no area case can carry the behavior
- [ ] Confirm failure with: `[repo test command + filter]`

### 2. Green

- [ ] Implement in: [paths]

### 3. Refine

- [ ] Review observable expectations against the contract and run the affected whole families at commit points
- [ ] Keep the suite under about 3,000 tests; name guarded bugs in `# behavior:` and use one producer Loaf per library case

## Documentation

- [ ] Release notes: [path] — [section] bullet + #issue
- [ ] Other docs: [path or none]
- [ ] Design records, if a decision changed: decision, boundaries, consequences, non-goals; under about 1,000 words, no mechanism log
- [ ] `mkdocs build --strict` from [docs root] if applicable

## Files to touch (expected)

| File | Change |
|------|--------|
| `...` | ... |

## Gate

| Step | Command |
|------|---------|
| Format | `make fmt` (Incan) / `make fmt-check` (IncQL) / … |
| Focused contracts | `incan run workspaces/contracts/src/main.incn -- INCAN workspaces/contracts/fixtures SELECTOR` (Incan; substitute compiler and case/area selector) |
| Commit contracts | Same runner with affected whole-family selectors (Incan) |
| Fast gate | `make pre-commit-fast` (Incan) / [verified target repo gate] |
| Before every commit | `make us-english-check` (Incan) |

## Success criteria

- [ ] Tests and docs gates pass
- [ ] All Gate commands succeed

## Suggested commit message (maintainer)

`[type](scope): [imperative summary] (#NNN)`
```
