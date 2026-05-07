---
name: loop
description: Orchestrate a named multi-skill loop such as /loop /review /fix until clean or blocked. Use when the user explicitly asks to loop skills, rerun a review/fix cycle, or says /loop with one or more skill names.
---

# Loop — Skill Orchestrator

## Purpose

`/loop` is a thin orchestrator. It does not define domain checks itself. It repeatedly runs a detector skill and a repair skill until the detector reports no remaining blockers/warnings or the work becomes blocked.

The loop should preserve state in the current worktree's persistent report file:

- `.agents/state/review-report.md`

Typical use:

- `/loop /review /fix`

That means:

1. run `/review`,
2. if it reports actionable findings, run `/fix`,
3. run `/review` again,
4. repeat until clean or blocked.

## Contract

- The first named skill should be a detector/reviewer.
- The second named skill should be a repair/fix skill.
- If the user provides more skills, run them in the given order, then loop back to the first detector.

## Stopping conditions

Stop the loop when one of these is true:

1. the detector reports no blockers or warnings,
2. remaining findings are explicitly classified as:
   - out of scope,
   - risky without user confirmation,
   - external blocker,
   - separate compiler bug,
3. the user interrupts or redirects the work.

## Loop discipline

- Keep each cycle concrete: detector output, repair pass, detector rerun.
- Do not let a detector silently become a fixer or a fixer silently become a detector. Respect each skill's role.
- Use `.agents/state/review-report.md` as the durable state between cycles. Do not rely on conversational memory to remember which findings are still open.
- Require the report to stay structured on every cycle: `## Scope`, `## Activity`, `## Files`, and `## Verification` must remain present. A rewritten prose summary is not valid loop state.
- Require file states to be honest loop state: files start `pending`, then move to `clean`, `findings`, `fixed`, or `blocked` only as the checklist evidence justifies.
- When `/loop` starts a new detector-first run, the detector should initialize a fresh report scaffold for that run. Preserve state across cycles within the same loop, not across unrelated invocations.
- If a broad verification command fails for unrelated reasons, continue the loop when local findings are still fixable.
- Summarize each cycle briefly so the user can see whether the loop is converging.

## Output format

Produce a compact progress report:

```md
## Loop — <skills>

### Cycle 1
<detector result>
<fix result>

### Cycle 2
...

### Final state
<clean / blocked / needs user direction>
```
