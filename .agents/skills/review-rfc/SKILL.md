---
name: review-rfc
description: Review an Incan RFC document for formatting, structural, and content issues before GitHub submission. Use when the user asks to review an RFC, check an RFC for problems, prepare an RFC for submission, or says /review-rfc.
---

# Review RFC — Incan Project

## Workflow

1. Read the RFC document in full.
2. Read the `Status:` field in the header — it controls which checklists apply (see **Status-aware checks** below).
3. Verify the RFC number is not already taken — check `workspaces/docs-site/docs/RFCs/` for conflicts.
4. Work through the universal checklists (1–5), then the status-specific checklist for the RFC's current status.
5. **Fix issues directly** (edit the file); RFC review is an editing task, not just a report.
6. If no GitHub issue exists yet, ask the user before creating one (see **Issue creation**). Once created, record the issue URL in the RFC header's `Issue:` field.
7. If review materially changes RFC filenames, numbers, or status/location (for example superseding or splitting an RFC), update RFC cross-references and regenerate the docs-site RFC snippets/index.
8. **Ask before semantic restructures.** Mechanical fixes, formatting cleanup, section-order repairs, and obvious lifecycle-field corrections should be applied directly. But if review suggests splitting an RFC, superseding it, renumbering it, or materially changing its scope/central design claim, stop and get explicit user confirmation before making that restructuring edit.

---

## Status-aware checks

The RFC lifecycle has four statuses. Different sections are required — or forbidden — at each stage:

| Status | Implementation Plan section | Checklist section | `Shipped in:` | "Unresolved questions" or "Design Decisions" |
| ------------- | --------------------------- | ----------------- | ------------- | -------------------------------------------- |
| `Draft` | ❌ Must NOT be present | ❌ Must NOT be present | `—` | Required: "Unresolved questions" |
| `Planned` | ❌ Must NOT be present | ❌ Must NOT be present | `—` | Required: "Design Decisions" (renamed) |
| `In Progress` | ✅ Must be present | ✅ Must be present | `—` | "Design Decisions" (already renamed) |
| `Done` | ✅ Present | ✅ All items `[x]` | ✅ Filled | "Design Decisions" (already renamed) |

---

## Checklist 1 — Structural completeness (all statuses)

- [ ] Header block present with all eight fields: Status, Created, Author(s), Related, Issue, RFC PR, Written against, Shipped in.
- [ ] `Written against:` reflects the Incan version that was current when the RFC was authored (never a future or planned version).
- [ ] `Shipped in:` is `—` for Draft/Planned/In Progress; only filled for Done.
- [ ] Sections follow canonical order: Summary → (Core model) → Motivation → Goals → Non-Goals → Guide-level explanation → Reference-level explanation → Design details → Alternatives considered → Drawbacks → (Implementation architecture) → **Layers affected** → (Implementation Plan + Checklist, In Progress/Done only) → Unresolved questions / Design Decisions.
- [ ] **Draft/Planned only: No section named "Implementation Plan", "Suggested rollout", or similar.** Prescriptive task steps belong in the GitHub issue or are added when the RFC moves to In Progress.
- [ ] **In Progress/Done only: "Implementation Plan" section present** with phases or layer-grouped tasks.
- [ ] **In Progress/Done only: "Checklist" (or "Progress Checklist") section present** with `- [ ]` / `- [x]` items.
- [ ] Closing comment present at end of file for Draft: `<!-- Rename this section to "Design Decisions" once all questions have been resolved... -->`.

---

## Checklist 2 — RFC cross-references (all statuses)

- [ ] No `[RFC NNN]` markdown reference-link syntax anywhere (header or body).
- [ ] All RFC cross-references use plain text: `RFC NNN` or `RFC NNN (description)`.
- [ ] No dangling link definitions at the bottom of the file for RFC references.

---

## Checklist 3 — Formatting (all statuses)

- [ ] No hard-wrapped prose paragraphs. Lines must not end mid-sentence at ~80–120 chars. Reflow each paragraph to a single unbroken line.
- [ ] Incan code blocks use the `incan` language tag (not `python`, `rust`, or untagged).
- [ ] Tables render cleanly with no broken Markdown.
- [ ] No `<!-- REVIEW: ... -->` comments left unaddressed in the file.

---

## Checklist 4 — Content quality (all statuses)

- [ ] Summary is a single tight paragraph stating the central claim.
- [ ] Goals and Non-Goals are balanced — Non-Goals make explicit what is out of scope.
- [ ] Guide-level explanation uses realistic, typed Incan code examples that are clear and well-explained without being excessive. It should be immediately obvious to the reader what the feature achieves.
- [ ] Prose flows naturally with no duplicated concepts across sections.
- [ ] Reference-level explanation uses normative language (`must`, `must not`, `should`, `may`).
- [ ] "Layers affected" describes impacts (not task steps); lists only layers that are actually affected.
- [ ] "Alternatives considered" includes a rationale for rejection for each alternative.
- [ ] **No prescriptive implementation prose in the design sections.** The RFC must not reference specific internal files, function names, struct fields, or data structures in the design sections. Those belong in the Implementation Plan / GitHub issues. If found in design sections, rewrite as a normative contract statement or remove.
- [ ] **Ambition check.** RFCs should be end-to-end and favor complete solutions over incremental stubs. Flag if the RFC is too dismissive or handwavy about hard parts. Equally, if the RFC is ambitious, verify that the ambition is well-motivated, clearly explained, and not excessive.
- [ ] **Coupling check.** If an RFC bundles a general language feature with a specific stdlib or product surface, challenge whether those concerns should stay together. If the coupling is not clearly justified in the document, flag it or recommend a split/supersession path.

---

## Checklist 5 — Confidentiality (all statuses)

- [ ] No internal or unreleased project names. Replace with generic descriptions: "future query language surfaces", "purpose-built libraries", etc.
- [ ] No links or citations to internal paths (`__strategy__/`, research notes, pre-RFC documents, or any folder outside the public repository). Describe the concept inline instead.

---

## Checklist 5b — RFC graph hygiene (all statuses)

- [ ] If an RFC is renamed, renumbered, split, or superseded, update inbound references in active RFCs and docs so they point at the new target rather than the stale file.
- [ ] If an RFC moves between active and `closed/` folders, regenerate `workspaces/docs-site/docs/_snippets/rfcs_refs.md` and `workspaces/docs-site/docs/_snippets/tables/rfcs_index.md`.
- [ ] If the RFC index generator fails locally, fix the generator or note the blocker; do not leave the docs index knowingly stale after RFC lifecycle edits.

---

## Checklist 6 — Draft-specific

- [ ] "Unresolved questions" section present with at least one open design question.
- [ ] Closing comment present: `<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->`
- [ ] Status is `Draft` (not `Planned` while questions are still open).

---

## Checklist 7 — Planned-specific

- [ ] "Unresolved questions" has been renamed to "Design Decisions".
- [ ] All questions in "Design Decisions" have accepted answers (no open bullets without resolution).
- [ ] GitHub issue is labeled `feature` (not `RFC`).

---

## Checklist 8 — In Progress-specific

- [ ] "Implementation Plan" section present after "Layers affected", with phases or layer-grouped steps.
- [ ] "Checklist" (or "Progress Checklist") section present with `- [ ]` / `- [x]` items, grouped by area (e.g. Spec, Parser/AST, Typechecker, Lowering/IR, Emission, Stdlib/Runtime, Tests, Docs).
- [ ] At least one checklist item is `- [x]` (otherwise the RFC has not actually started).
- [ ] `Shipped in:` is still `—`.

---

## Checklist 9 — Done-specific

- [ ] All checklist items are `- [x]` (no open `- [ ]` items).
- [ ] `Shipped in:` is filled with the actual release version (not `—`, not `TBD`).
- [ ] Release notes updated in `workspaces/docs-site/docs/release_notes/`.

---

## Common issues found in practice

|                                      Issue                                      |                                              Fix                                              |
| ------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------- |
| `[RFC NNN]` reference links without definitions                                 | Convert to plain text `RFC NNN`                                                               |
| Section named "Implementation plan" in a Draft or Planned RFC                  | Remove or defer to GitHub issue; keep only "Layers affected"                                  |
| Missing "Implementation Plan" + "Checklist" in an In Progress RFC              | Add both sections; use `/bump-rfc` skill to generate them from "Layers affected"              |
| Prose references specific files or functions (`calls.rs`, `FunctionInfo`, etc.) | Rewrite as a normative contract statement or remove (OK in Implementation Plan, not in design)|
| Hard-wrapped prose (lines ending mid-sentence)                                  | Reflow each paragraph to a single line                                                        |
| `Shipped in:` filled for a Draft/Planned/In Progress RFC                        | Set to `—`; only fill once the feature is released                                            |
| `Shipped in:` still `—` for a Done RFC                                          | Fill with the actual release version                                                          |
| Missing "Layers affected" section                                               | Add with affected layers from Parser, Typechecker, Lowering, Emission, Stdlib, Formatter, LSP |
| "Unresolved questions" not renamed in Planned RFC                               | Rename to "Design Decisions"; ensure all questions have answers                               |
| `<!-- REVIEW: ... -->` comment left in file                                     | Address the comment and remove it                                                             |
| Internal project name mentioned                                                 | Replace with a generic description                                                            |
| Link to internal path (`__strategy__/`, research notes)                         | Remove the link; describe the concept inline                                                  |
| Code blocks untagged or wrongly tagged                                          | Add `incan` language tag                                                                      |
| One RFC mixes a general feature and a specific library proposal                 | Recommend split/supersession or justify the coupling explicitly                               |
| RFC file moved or status changed but docs index still points at old path        | Update references and regenerate RFC snippets/index                                           |

---

## Issue creation

Only create an issue if one does not already exist. Always ask the user first.

Create the issue with `gh issue create`. Include at minimum: a one-paragraph summary, the motivation, and the path to the RFC document. Use the `RFC` label and any relevant area labels (`incan language semantics`, `incan compiler`, `incan stdlib`, `incan tooling`).

```bash
gh issue create \
  --title "RFC NNN: <Title>" \
  --label "RFC" \
  --body "..."
```

See `.github/ISSUE_TEMPLATE/rfc_proposal.yml` for the fields the template covers — use those as a guide for what to include in the body.

Once the issue is created, record its URL in the RFC's `Issue:` header field.

---

## RFC lifecycle

| Status | Meaning | Trigger |
| ------------- | ----------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| `Draft` | Active design work; unresolved questions remain | Initial state for all new RFCs |
| `Planned` | Design settled; ready for implementation | All questions answered, review complete, user confirms (use `/bump-rfc`) |
| `In Progress` | Active implementation underway | At least one PR open; Implementation Plan + Checklist added (use `/bump-rfc`) |
| `Done` | Implementation complete and shipped | All checklist items checked, `Shipped in:` filled, release notes updated (use `/bump-rfc`) |

Use the `/bump-rfc` skill to perform status transitions correctly.
