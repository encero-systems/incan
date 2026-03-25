---
name: bump-rfc
description: Transition an Incan RFC from one status to the next. Use when the user asks to promote, advance, or finalize an RFC, or says /bump-rfc. Handles Draft → Planned, Planned → In Progress, and In Progress → Implemented transitions.
---

# Bump RFC — Incan Project

## Overview

RFCs move through four statuses. Each transition requires specific file edits, GitHub actions, and prerequisites.

| Status        | Meaning                                         |
| ------------- | ----------------------------------------------- |
| `Draft`       | Active design work; unresolved questions remain |
| `Planned`     | Design settled; ready for implementation        |
| `In Progress` | Active implementation underway                  |
| `Implemented` | Implementation complete and shipped             |

Read the RFC's current `Status:` field, then follow the matching section below.

---

## Draft → Planned

### Prerequisites

Before bumping, verify:

- [ ] All "Unresolved questions" have accepted answers — no open bullets without resolution.
- [ ] At least one review round has been completed (use `/review-rfc` if not done).
- [ ] GitHub issue exists and is linked in the RFC header.
- [ ] User has confirmed the bump (ask if unclear).

### File changes

1. Rename the section `## Unresolved questions` → `## Design Decisions`. Keep all the Q&A content; just rename the heading.
2. Remove the closing `<!-- Rename this section to "Design Decisions"... -->` comment from the end of the file.
3. Update the header: `Status: Draft` → `Status: Planned`.

### GitHub actions

```bash
# Remove RFC label, add feature label
gh api repos/dannys-code-corner/incan/issues/<NNN>/labels/RFC -X DELETE
gh api repos/dannys-code-corner/incan/issues/<NNN>/labels -X POST --input - <<'EOF'
{"labels":["feature"]}
EOF

# Post a status comment
gh issue comment <NNN> --repo dannys-code-corner/incan --body \
  "RFC has moved to **Planned** status. Design is settled; implementation can begin."
```

---

## Planned → In Progress

### Prerequisites

- [ ] At least one implementation PR has been opened or a contributor has picked up the work.
- [ ] User has confirmed the bump.

### File changes

After the `## Layers affected` section, add two new sections: `## Implementation Plan` and `## Progress Checklist`.

**`## Implementation Plan`** — concrete phases, not internal file paths. Model after the RFC's "Layers affected" section, but task-oriented. Example shape:

```markdown
## Implementation Plan

### Phase 1: Parser + AST

- Add syntax support for `<new construct>` in the lexer and parser.
- Extend the AST to represent `<new construct>` as a typed node.
- Add formatter support for the new syntax.

### Phase 2: Typechecker

- Validate `<new construct>` at declaration sites.
- Resolve references in expression positions.
- Emit span-precise diagnostics for invalid usage.

### Phase 3: Lowering + Emission

- Lower `<new construct>` to the IR representation.
- Emit correct Rust code for all cases.

### Phase 4: Stdlib + Tests

- Add stdlib declarations (if applicable).
- Add parser, typechecker, codegen snapshot, and integration tests.
- Update docs.
```

**`## Progress Checklist`** — fine-grained `- [ ]` items, grouped by area. These will be ticked as PRs land. Example shape:

```markdown
## Progress Checklist

### Spec / design

- [ ] Lock down edge cases for `<X>` and add to "Design Decisions".

### Parser / AST

- [ ] Lexer: emit new token for `<keyword>`.
- [ ] Parser: parse `<construct>` in declaration/expression position.
- [ ] AST: represent as `<NodeKind>` with correct span.
- [ ] Formatter: round-trip `<construct>` stably.

### Typechecker

- [ ] Validate at declaration sites.
- [ ] Resolve in expression positions.
- [ ] Error: `<diagnostic name>` for invalid usage.

### Lowering / IR

- [ ] Lower `<construct>` to `<IrNode>`.

### Emission

- [ ] Emit correct Rust for `<construct>`.

### Stdlib / Runtime

- [ ] Declare in `stdlib/<module>.incn` (if applicable).
- [ ] Wire Rust backing type in `incan_stdlib` (if applicable).

### Tests

- [ ] Parser unit test for `<construct>`.
- [ ] Typechecker unit test: valid usage.
- [ ] Typechecker unit test: invalid usage → correct diagnostic.
- [ ] Codegen snapshot test: `<construct>` in expression position.
- [ ] Integration test (end-to-end compile + run).

### Docs

- [ ] Update relevant docs-site pages.
- [ ] Add release notes entry.
```

3. Update the header: `Status: Planned` → `Status: In Progress`.

### GitHub actions

Post the implementation plan to the GitHub issue so contributors can track it there too:

```bash
gh issue comment <NNN> --repo dannys-code-corner/incan --body \
  "RFC has moved to **In Progress**. Implementation plan added to the RFC document and pasted below for reference.

<paste the Implementation Plan + Checklist content here>"
```

---

## In Progress → Implemented

### Prerequisites

- [ ] All `- [ ]` items in the Progress Checklist are `- [x]`.
- [ ] Feature is merged to `main` and tests pass.
- [ ] Release version is known.
- [ ] User has confirmed the bump.

### File changes

1. Update the header: `Status: In Progress` → `Status: Implemented`.
2. Fill the release field with the actual Incan release version:
   - If the RFC uses `Shipped in:`, set `Shipped in: vX.Y`.
   - If the RFC uses `Implemented version:`, set `Implemented version: X.Y.Z`.
3. Move the RFC document into `workspaces/docs-site/docs/RFCs/closed/implemented/` and keep the same filename.
4. Optionally rename `## Progress Checklist` → `## Implementation log` to signal it is historical rather than active.

### GitHub actions

```bash
# Close the issue
gh issue close <NNN> --repo dannys-code-corner/incan \
  --comment "Implementation complete. Shipped in vX.Y."
```

### Release notes

Add an entry to `workspaces/docs-site/docs/release_notes/<vX_Y>.md` following the project's release notes style:

```markdown
- **<Area>**: <One-line description of the feature> (RFC NNN, #<issue>)
```

---

## Quick reference

| From | To | Key file change | Key GitHub action |
| ------------- | ------------- | ------------------------------------------------- | ------------------------------------------- |
| `Draft` | `Planned` | Rename "Unresolved questions" → "Design Decisions"; update Status | Relabel `RFC` → `feature` |
| `Planned` | `In Progress` | Add Implementation Plan + Progress Checklist; update Status | Post plan as issue comment |
| `In Progress` | `Implemented` | Update Status; fill release field; move file to `closed/implemented/`; optionally rename checklist to log | Close issue with release version |
