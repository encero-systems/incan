---
name: review-test-style
description: Review touched tests and test-adjacent code for style, Result-returning patterns, panic helpers, unwrap/expect use, and obvious coverage gaps. Use when a broad review needs a dedicated test pass or when the user asks for test-review specifically.
---

# Review Test Style — Incan Compiler

## Purpose

`/review-test-style` is a narrow report-only reviewer for touched tests and code changes that imply missing test coverage.

Own:

- panic helpers and panic-style control flow in touched tests
- unwrap/expect in tests
- fallible tests not returning `Result`
- obvious missing targeted coverage for new behavior

Do not own:

- rustdoc prose
- docs truthfulness
- architecture
- final branch-clean judgment

## Output artifact

Write a slice report at:

- `.agents/state/review-report.test-style.md`

Do not write to the canonical `.agents/state/review-report.md`.

## Workflow

1. Review the touched tests and the code changes that should have tests.
2. Flag:
   - `.unwrap()` / `.expect()` in tests
   - `panic!`-driven helper/control flow where `Result` + assertions would be clearer
   - missing targeted tests for new CLI, lifecycle, or semantic behavior
   - stale snapshots or missing integration coverage when behavior changed end-to-end
3. Only flag a coverage gap when the new behavior or branch is not exercised anywhere in the touched tests. If integration coverage already exercises the behavior, do not demand redundant parser/unit duplication without a concrete reason.
4. Keep this pass focused on test quality and coverage, not general code quality.

## Slice report shape

Keep slice reports findings-first. Only list clean surfaces when they are materially useful to the orchestrator.

```md
# Review Slice Report

- role: test-style
- worker: <agent-id or stable label>
- status: in_progress | clean | blocked

## Scope
- assigned files:
  - tests/integration_tests.rs

## Findings

- [ ] warning | test-gap | panic helper | src/cli/mod.rs:84
  Test helper uses panic-oriented control flow instead of `Result` + assertions.

## Reviewed Clean Surfaces

- tests/integration_tests.rs — touched lifecycle coverage reviewed; no additional findings
```

If there are no findings, say so explicitly.
