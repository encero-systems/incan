---
name: review-incan-source-quality
description: Review touched Incan source for Python-quality readability, idiomatic Incan structure, dogfooding integrity, public API polish, and source-level anti-patterns. Use for `.incn` stdlib, examples, language-surface implementations, or when the user asks whether Incan code reads like well-written Python rather than Rust-shaped scaffolding.
---

# Review Incan Source Quality

## Purpose

`/review-incan-source-quality` is a report-only reviewer for touched `.incn` source.

The standard is not merely "has comments" or "passes tests". The standard is:

- the code should read like well-written Python;
- the source should use Incan as the implementation language, not as a thin facade;
- public APIs should feel authored for users;
- private helpers should make the algorithm clearer, not more mechanical;
- comments should explain intent, protocol invariants, or boundary quirks.

Own:

- `.incn` readability and source organization
- stdlib dogfooding quality
- evidence that implementation choices were based on current Incan capability, not stale assumptions
- Pythonic naming, helper shape, and control flow
- public API docstrings and examples
- comments that clarify non-obvious behavior
- anti-patterns that make Incan source look generated, Rust-shaped, or backend-driven

Do not own:

- broad architecture placement decisions
- Rust prose or rustdoc quality
- test style outside `.incn` test fixtures/examples
- docs truthfulness outside comments/docstrings embedded in source
- final branch-clean judgment

## Output artifact

Write a slice report at:

- `.agents/state/review-report.incan-source-quality.md`

Do not write to the canonical `.agents/state/review-report.md`.

## Review standard

Treat touched Incan source as user-facing language showcase code, especially under `crates/incan_stdlib/stdlib/`, examples, fixtures that teach behavior, and RFC-backed language features.

Good Incan source should have:

- top-down structure that exposes the public contract before implementation details;
- names that describe domain concepts rather than backend mechanics;
- small helpers that remove real complexity;
- direct `?`, `if let`, RFC 070 `Result` combinators, early return, and value-enum/model usage where those make the code simpler;
- public docstrings with `std.fs`-style shape: summary, semantic notes, and `Args`, `Returns`, or `Example` sections where useful;
- comments for bit layouts, protocol invariants, compiler boundary workarounds, or surprising tradeoffs;
- ordinary Rust interop only where it imports existing primitives/crates and the `.incn` source still owns the behavior.

Flag Incan source that has:

- custom Rust backends hidden behind a thin `.incn` wrapper when the behavior should be dogfooded;
- `@rust.extern`, `rusttype`, or `rust.module` used to avoid writing expressible Incan behavior;
- design narrowing or backend fallback justified by “Incan cannot do this” without local examples, tests, or probe evidence;
- sentinel initialization such as `value = 0` only to satisfy later branch assignment;
- verbose `match` blocks that just rewrap a `Result` where `?` would read naturally;
- verbose `match` blocks that only transform one `Result` branch where RFC 070 combinators such as `map`, `map_err`, `and_then`, or `or_else` would state the intent directly;
- unnecessary type noise when inference or a local helper would be clearer;
- Rust-shaped names, ownership workarounds, `.clone()`, `.to_string()`, or manual conversion scaffolding leaking into `.incn`;
- helpers that hide one obvious operation without adding meaning;
- stringly or byte-twiddling logic without named intent;
- comments that narrate the next line instead of explaining why;
- public APIs that expose compiler/backend vocabulary instead of a Pythonic user-facing surface;
- generated-looking code in authored stdlib or examples.

## Workflow

1. Derive scope from touched `.incn` files in the current worktree plus any `.incn` files named by the user.
2. For stdlib or RFC-backed work, compare source shape against nearby established modules such as `std.fs`, `std.io`, `std.collections`, or the relevant domain module.
3. Check whether the implementation claims or implies a current Incan limitation. If so, verify that the branch records local precedent, tests, or probe evidence for that limitation.
4. Inspect public declarations first: module docstring, public types, public functions, method names, argument order, return shape, and examples.
5. Inspect implementation helpers next: helper names, control-flow readability, branch shape, conversion noise, and whether helpers clarify or obscure.
6. Inspect comments/docstrings last as part of source quality, not as a separate docs-only pass.
7. For each finding, explain what a Pythonic/Incan-native version would make clearer. Do not demand style churn when the existing shape is already direct and readable.
8. Stay report-only unless the user explicitly asks for fixes.

## Slice report shape

Keep findings first. Only list clean surfaces when the clean call is useful because the file is a visible Incan surface or had prior risk.

```md
# Review Slice Report

- role: incan-source-quality
- worker: <agent-id or stable label>
- status: in_progress | clean | blocked

## Scope
- assigned files:
  - crates/incan_stdlib/stdlib/uuid.incn

## Findings

- [ ] warning | source-quality | Rust-shaped sentinel read | crates/incan_stdlib/stdlib/uuid.incn:117
  The function initializes a placeholder byte and overwrites it from a match arm. A direct helper returning `Result[u8, UuidError]` would read like authored Incan rather than generated Rust-shaped control flow.

## Reviewed Clean Surfaces
- crates/incan_stdlib/stdlib/fs/path.incn — used as style baseline
```

Finding severities:

- `blocker`: source violates an explicit dogfooding or implementation-boundary requirement.
- `error`: source is misleading, generated-looking, or exposes backend shape in a user-facing API.
- `warning`: source works but is below the Python-quality readability bar.
- `note`: cleanup is optional but useful if the file is already being edited.

If there are no findings, say so explicitly.
