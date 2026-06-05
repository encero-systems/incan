---
name: flag-compiler-bug
description: Detect, triage, de-duplicate, and raise compiler bug reports when an agent encounters a likely compiler defect during implementation, review, testing, or issue work. Use when a task surfaces an ICE, panic, wrong accept/reject behavior, miscompile, lowering/emission bug, diagnostic bug, formatter/LSP/frontend drift, or other likely compiler/tooling defect; pause current work, minimize the repro, decide whether the bug blocks the task or has a valid workaround, check whether it is already reported, and then use create-github-issue to raise the bug.
---

# Flag Compiler Bug

This skill exists so agents self-discover the behavior. Do not wait for the user to explicitly say "file a compiler bug" if the evidence is strong enough.

## Core rule

If you encounter a likely compiler bug:

1. pause and verify the observation
2. reduce it to the smallest honest repro you can
3. decide whether it blocks the current task or can be worked around without lying to yourself
4. check whether the bug is already filed
5. raise or draft the bug with `create-github-issue`
6. resume the original task only if the workaround is real and the remaining scope still makes sense

Minimal repro is the most important artifact. If the repro is weak, the report is weak.

## What counts as a compiler bug

Treat these as likely compiler bugs unless evidence shows otherwise:

- compiler panic or internal error
- valid program rejected contrary to RFCs, language docs, or established behavior
- invalid program accepted silently
- wrong lowering or emission that changes semantics
- wrong generated Rust or runtime behavior caused by compiler output
- incorrect or misleading diagnostic from parser, typechecker, lowering, or tooling
- formatter, CLI, LSP, or snapshot behavior drifting from shared frontend expectations

Do not flag a compiler bug when the issue is more likely:

- user misuse with an expected diagnostic
- an RFC gap or not-yet-implemented feature that is explicitly out of scope
- an already-known issue with no new repro or impact
- an infrastructure flake unrelated to compiler semantics

## Workflow

### 1. Verify and localize

Capture:

- exact local command for your private working notes, then derive a sanitized public command before filing
- exact observed output, panic text, or wrong behavior
- affected stage if inferable: parser, typechecker, lowering, emission, runtime boundary, formatter, CLI, or LSP
- current branch / commit / task context

If you are not confident it is a compiler bug, say so explicitly and explain the uncertainty.

### 2. Minimize the repro

Reduce the case aggressively:

- remove unrelated declarations
- inline imports or dependencies when possible
- shrink to one file if possible
- keep only the command needed to reproduce

Prefer a tiny repro over a "realistic" one. If minimization changes the bug surface, keep both and say why.

### 3. Judge blocking vs workaround

Decide which of these is true:

- **Blocking**: the current task cannot be completed honestly without fixing the compiler bug first.
- **Workaround exists**: there is a logically sound path that preserves the task's intent, even if less elegant.
- **Non-blocking follow-up**: the bug is real but does not stop the current implementation.

Do not call something a workaround if it quietly changes semantics, dodges test coverage, or narrows the task in a way the user did not ask for.

### 4. Check for duplicates

Before raising a new issue, search existing issues in the relevant tracker using:

- panic text or diagnostic snippet
- affected syntax or feature name
- likely stage
- related RFC number, issue number, or file path when useful

Search open issues first, then recent closed issues if the match looks plausible.

If a likely duplicate exists:

- link it
- explain why it matches
- do not open a new issue unless the current repro materially expands the scope

### 5. Raise the bug

Use `create-github-issue` with the bug template in the correct repository.

For compiler/tooling defects in this workspace, that is usually the `incan/` repository even if the bug surfaced while working in `InQL/`.

Include:

- minimal repro
- expected vs actual behavior
- sanitized command, using repo-relative paths and tool names instead of local absolute binary paths
- logs / panic text / snapshot diff if relevant
- affected stage
- blocker status
- workaround, if any
- environment and commit context
- related issue, RFC, branch, or task

Before creating the issue, run the `create-github-issue` public text safety gate on the exact title/body you will publish. Do not publish absolute local paths such as `/Users/...`, `/home/...`, `/private/...`, `/tmp/...`, or commands that expose a local checkout path. If the private reproduction used a local compiler binary, publish a generic equivalent such as `incan run path/to/repro.incn` and keep commit/version information in the Environment section.

If the current workflow permits creating the GitHub issue directly, do that after the duplicate check. Otherwise return the ready-to-file draft.

### 6. Return to the original task

If the bug is blocking, stop the original task and report that clearly.

If a real workaround exists, continue the task and explicitly record:

- the workaround used
- the bug that was raised or linked
- residual risk from continuing

## Output format

```md
## Compiler bug triage
- Likely bug: yes | no | uncertain
- Stage:
- Blocking status:
- Workaround:
- Repro:
- Existing issue:
- New issue draft or link:
- Next action on original task:
```

## Quality bar

- Repro is minimal and copy-pastable.
- Duplicate search is explicit, not assumed.
- Public issue text is sanitized before the first GitHub create/update call.
- Blocking vs workaround judgment is stated plainly.
- The original task is either paused honestly or resumed with a real workaround.
