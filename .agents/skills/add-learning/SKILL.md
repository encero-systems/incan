---
name: add-learning
description: Add a new learning to the agent learnings file. Use when the user says /add-learning, asks to record a lesson, or when an implementation produced a reusable insight worth preserving for future agents.
---

# Add Learning — Incan Project

## When to use

Add a learning when an implementation taught a **durable, generalizable lesson** that a future agent would get wrong without it. Good learnings are:

- Pitfalls where code passes one stage but fails another
- Non-obvious wiring requirements (e.g., "if you change X, you must also update Y")
- Patterns that look correct but produce subtle bugs
- Architectural constraints that aren't obvious from the code alone

Do **not** add:

- Implementation details specific to a single feature that are already in the code
- API documentation (that belongs in rustdoc)
- Temporary workarounds or known bugs (those belong in GitHub issues)

## Workflow

### Step 1: Identify the right section

Read `.cursor/agents/learnings.md` and determine which existing section the learning belongs in:

| Section | Add here if the learning is about... |
| --- | --- |
| General pipeline pitfalls | Typechecker/lowering/emission interactions, `Program` struct, type display |
| Testing strategy | Which tests to write, test coverage gaps, snapshot patterns |
| Parser and lexer patterns | Token handling, bracket depth, warning infrastructure, soft keywords |
| Stdlib and registry patterns | `STDLIB_NAMESPACES`, stub vs wiring, runtime facades |
| Wiring: CLI and LSP | Warning surfacing, feature gates, command coverage |
| Generic bounds and extern functions | Bounds storage, extern diagnostics, shared helpers |

If no existing section fits, create a new one with a descriptive heading.

### Step 2: Write the learning

Format as a bold-label bullet:

```markdown
- **Concept in 3-5 words**: Explanation of the insight, why it matters, and what goes wrong without it. Include the triggering context (RFC number, issue number) in parentheses if applicable.
```

Guidelines:

- Lead with the **principle**, not the specific feature that taught it.
- Keep it to 1-2 sentences. If it needs more, it might be too specific.
- Include enough context that an agent can act on it without reading the original RFC/issue.

### Step 3: Append to the file

Add the bullet to the appropriate section in `.cursor/agents/learnings.md`. Maintain alphabetical or logical ordering within the section.

### Step 4: Verify

Read back the file to confirm:

- The new bullet is in the right section
- It doesn't duplicate an existing learning
- It's generalizable (would help with future work, not just the current task)
