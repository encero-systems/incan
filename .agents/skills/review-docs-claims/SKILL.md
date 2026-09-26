---
name: review-docs-claims
description: Review user-facing docs, CLI reference text, examples, and scaffolds for truthfulness, prose quality, and RFC leakage. Use when a broad review needs a dedicated docs/claims pass or when the user asks for docs-truth review specifically.
---

# Review Docs Claims — Incan Compiler

## Purpose

`/review-docs-claims` is a narrow report-only reviewer for touched user-facing documentation and examples.

Own:

- docs/CLI/examples/scaffolds claiming unimplemented behavior
- Divio page intent and public-reference completeness
- user-facing RFC leakage outside explicit inventories
- touched markdown prose quality
- release-note inventory consistency when applicable

Do not own:

- Rust comment prose
- architectural placement
- test style
- final branch-clean judgment

## Output artifact

Write a slice report at:

- `.agents/state/review-report.docs-claims.md`

Do not write to the canonical `.agents/state/review-report.md`.

## Reference pages state the contract only

A page under a `reference/` directory states the public contract and nothing else: the exact rule, the types, signatures, parameters, returns and defaults, each refusal with its stable diagnostic code, and small examples annotated with what is accepted or refused. Read every changed sentence of a reference page against the list below. Each hit is a `blocker` finding, even when the sentence is true, MkDocs passes, and the rest of the page is good. The fix is to delete the sentence, restate it as contract, or move it to the matching how-to (for advice) or explanation (for rationale and mechanics) page and link to it.

- **Quoted diagnostic messages.** The contract is the refusal and its code (`INCAN-T0001`), never the message wording (`Non-exhaustive match: missing patterns for _`). Message text changes without notice; a reference that quotes it goes stale.
- **Where the implementation reaches.** Lists such as "applies to the same module, imported source modules, the standard library and compiled libraries" describe coverage, not a rule. State the rule; if something is outside it, that is a limitation to fix or to state once as a rule, not an inventory.
- **Implementation gaps phrased as rules.** "Is not judged by this rule", "is not checked yet", "a trait's default body is not judged" expose the checker's current reach. Remove them or turn them into a stated contract.
- **Compiler-internal vocabulary.** Columns, pattern matrix, facts, projections, recorded identities, passes, lowering, the emitter. A reader of the language contract never meets these.
- **Mechanics.** How the compiler arrives at a result: how bounds are inferred ("bounds another method of the same model put there", "receives those bounds in the same way…"), how a diagnostic is formatted ("spelled with wildcard payloads…"), what the generated Rust looks like.
- **Example comments that narrate.** A comment on a reference example says accepted or refused and, if needed, the contract reason in a few words (`# refused: T does not declare Clone`). It does not explain the mechanism (`# U receives Display (from show) and Clone`).
- **Advice and rationale.** "Write X instead", "use Y when", "prefer", "because…", "so that…".
- **Comparisons.** With Python, Rust or any other language ("as Python spells it", "like Rust's `Display`").
- **Time-bound wording.** "Currently", "today", "yet", "for now", "no longer", "-style message".
- **Issue and RFC numbers** in reference text.

## Workflow

1. Review the touched user-facing `.md` files, CLI help surfaces, examples, and scaffolds assigned by the orchestrator.
   Identify each page's Divio intent using the repository's `AGENTS.md` docs rules: tutorial, how-to, reference, or explanation. Check the content against that intent, not just its directory. For a reference, compare its inventory against the relevant public source surface and verify signatures, parameters, returns, defaults, errors, constraints, and lookup structure. A walkthrough or overview under `reference/` is a finding even when every sentence is true and MkDocs passes. Then read every changed sentence of a reference page against *Reference pages state the contract only* below: completeness and truth are necessary but not enough. Small illustrative examples are allowed; do not require four separate pages for every feature.
2. Check actual implementation against the docs. Prefer the current code and current tests over optimistic prose, stale assumptions, or superseded branch history.
3. RFC text is still canonical, but if the current branch deliberately diverges and the divergence is explicitly documented with a coherent reason, report that as a documented deviation rather than blindly calling it fiction.
4. Flag:
   - mixed or incorrect Divio intent, including task recipes or implementation narratives replacing API reference material
   - any sentence on a reference page that goes beyond the contract (see *Reference pages state the contract only*)
   - incomplete reference contracts or missing public APIs within the page's stated scope
   - docs-generated fiction
   - user-facing RFC references outside explicit inventory contexts
   - short-prosed or mechanically chopped markdown
   - stale release-note inventory for implemented user-facing work
5. If behavior is out of scope, prefer correcting the docs rather than inventing implementation in the report.

## Slice report shape

Keep slice reports findings-first. Do not enumerate every clean file with a full checklist. Only record:

- actual findings,
- and, optionally, a short `## Reviewed Clean Surfaces` section for risky or non-obvious clean calls.

```md
# Review Slice Report

- role: docs-claims
- worker: <agent-id or stable label>
- status: in_progress | clean | blocked

## Scope
- assigned files:
  - workspaces/docs-site/docs/foo.md

## Findings

- [ ] blocker | docs | docs-generated fiction | workspaces/docs-site/docs/foo.md:76
  Claims `requires-incan` is enforced today, but the branch does not implement that.

## Reviewed Clean Surfaces

- workspaces/docs-site/docs/bar.md — docs/claims reviewed; no findings
```

If there are no findings, say so explicitly.
