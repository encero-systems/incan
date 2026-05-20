# Writing RFCs (how-to)

This guide is for contributors writing an RFC (design record) in this repo.

RFC stands for “Request for Comments”.

!!! warning "Before you start"

    Always check whether there is already an RFC or a proposal RFC-issue for the feature you want to propose.

Start here:

- [Proposals: issues vs RFCs](../tutorials/book/03_proposals_issues_vs_rfcs.md)
- [RFC index](../../RFCs/index.md)

## When you should write an RFC

Write an RFC when the *user-facing meaning* of the language changes, for example:

- new syntax/keyword
- new semantics (what code means)
- new type system rule or inference behavior
- a stdlib contract that becomes “part of the language” (stable surface users rely on)
- a tooling change that affects a stable user-facing contract (for example: a new CLI command/flag/config)

## Workflow

1. **(Optional) Start with an issue**
File an “RFC proposal” issue to align on the problem and scope.

2. **Create the RFC file**
    - Copy the template: [RFC template](../../RFCs/TEMPLATE.md)
    - Create a new file under `workspaces/docs-site/docs/RFCs/` named: `NNN_short_slug.md`
      (example: `017_match_guards.md`)
    - Pick the next available `NNN` from the [RFC index](../../RFCs/index.md).

3. **Fill in the RFC**
    Keep it focused: one coherent proposal. Make sure you cover the following:
    - motivation + examples
    - precise rules (semantics/type rules/edge cases)
    - alternatives considered
    - migration/compatibility
    - acceptance criteria (“done when…”)
    - implementation plan + touchpoints (frontend/backend/stdlib/tooling/tests)

4. **Open a PR**
    - Link the PR in the RFC’s “Status” section.
    - Expect iteration: RFCs usually evolve over review.

5. **Discuss**
Use the PR discussion (and/or the linked issue) to gather feedback and converge.

## After acceptance

- If implemented: move the RFC to `workspaces/docs-site/docs/RFCs/closed/implemented/`.
- If deferred: keep it in place but update the status to `Deferred` and summarize why.

## Closed RFCs (implemented / superseded / rejected)

RFCs are organized so contributors can see “what’s still open” at a glance:

- `workspaces/docs-site/docs/RFCs/` — open RFCs (draft/planned/in progress/blocked/deferred)
- `workspaces/docs-site/docs/RFCs/closed/implemented/` — implemented RFCs
- `workspaces/docs-site/docs/RFCs/closed/superseded/` — superseded RFCs (replaced by a newer RFC)
- `workspaces/docs-site/docs/RFCs/closed/rejected/` — rejected / withdrawn RFCs (“won’t do”)

When an RFC is superseded or rejected, move it into the appropriate `closed/` folder and update its status accordingly (for example: `Superseded by RFC 018`).

## Tips for a good RFC

- Prefer concrete examples over abstract prose.
- Write the *reference-level rules* as if someone will implement them.
- Call out non-goals explicitly.
- If it’s too big: split it into a sequence of smaller RFCs.
