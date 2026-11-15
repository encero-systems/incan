# Agent Resources

This folder contains subagents and reference material for AI agents working on the Incan compiler.

## Subagents

| File | Purpose |
| --- | --- |
| `test-suite.md` | Test orchestrator — analyzes the diff, runs focused contract selectors and affected whole families, reports results. Delegated to automatically when validating changes. |

## Reference material

| File | Purpose |
| --- | --- |
| `learnings.md` | Hard-won insights from past RFC implementations and issue resolutions. Consult before starting work on any RFC implementation or parser/typechecker/lowering change. |
| `anti-patterns.md` | Every anti-pattern the repository's rules name, each with its source, a search that finds its violations and their count. Rerun a rule's search before and after clearing it. |

## Adding learnings

When an implementation teaches a durable lesson about architecture, testing, or pitfalls, use the `/add-learning` skill to append new insights. It handles section matching, formatting, and deduplication.

## Skills worth discovering

| Skill | Purpose |
| --- | --- |
| `flag-compiler-bug` | Pause when a likely compiler defect is found, minimize the repro, check for duplicates, decide blocking vs workaround, and raise or draft the bug. |
