# 3. Proposals: issues vs RFCs

This chapter explains how we decide between filing:

- a GitHub issue (bug/feature/chore), or
- an RFC (design record) for a larger change.

The goal is to keep the project moving while making sure new language features are deliberate and reviewed.

## The short rule

- **Bugs / chores**: file an issue in github. No RFC required.
- **New language features** (syntax, semantics, type system behavior): **RFC required**.
- **Tooling features**: issue by default; RFC when the change affects stable contracts or needs design discussion.

If a change alters how `.incn` behaves for users, assume “RFC required” unless a maintainer says otherwise.

## What is an RFC (in this repo)?

RFCs are PEP-style design records (proposal + rationale + trade-offs). They are not canonical user docs, but user docs may link to them for background.

Start here:

- RFC index: [RFCs](../../../RFCs/index.md)
- Template: [RFC template](../../../RFCs/TEMPLATE.md)

## How to file

### 1) File an issue (bug/feature/chore)

Use an issue when you can describe the work without needing agreement on a new language design. Pick the template that best fits your situation:

<!-- markdownlint-disable MD033 MD013-->
- **Bug report**:   <small>a bug in compiler/tooling (e.g. codegen emits invalid Rust for an edge case).</small> Include a minimal `.incn` repro, expected output, and actual output.

- **Feature request**:   <small>a feature you want (but no language design required).</small> Include the problem statement, a concrete example, and acceptance criteria (“done when…”).

- **Chore**:   <small>maintenance work (no user-visible change).</small> Include the motivation and a small checklist of the intended changes.

- **Documentation**:   <small>missing documentation, typos or grammar/style issues, or drift (doc claims a feature exists but it’s planned/bugged/wrong), </small> Include the doc link, proposed wording, and a tracking issue link.

- **Question**:   <small>you’re unsure which direction is correct and want to engage the community for feedback or assistance.</small> Include context, what you already tried, and 1–2 concrete questions you want answered.

- **Task**:   <small>a scoped piece of work (e.g. performance regression in a hot path).</small> Include before/after timings, benchmark notes, and the suspected hotspot.
<!-- markdownlint-enable MD033 MD013-->

### 2) Write an RFC (new language features)

Write an RFC when you propose a new user-visible language capability or meaning change, such as:

- new keyword / syntax form
- new type system rule
- new standard library contract that effectively becomes part of the language

Essentially, anytime the 'look-and-feel' of the language itself would be affected, an RFC is required.

#### How to file an RFC (workflow)

Use the RFC template and follow the detailed workflow here: [Writing RFCs (how-to)](../../how-to/writing_rfcs.md)

An RFC should answer:

- What problem are we solving?
- Why is this the right solution?
- What are alternatives?
- What is the migration story?
- What are the acceptance criteria?

## Next

Next chapter: [04. Your first change: add a builtin](04_add_a_builtin.md).
