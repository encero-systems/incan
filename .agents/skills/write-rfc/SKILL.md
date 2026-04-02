---
name: write-rfc
description: Draft a new RFC document for the Incan language project. Use when the user asks to write, create, or draft an RFC, or wants to propose a new language feature or infrastructure change for Incan.
---

# Write RFC — Incan Project

## Canonical section order

Every Incan RFC must follow this section order. Omit optional sections only when they genuinely add nothing.

1. YAML-style header block
2. **Summary** — one tight paragraph; the central claim
3. **Core model** *(optional)* — for complex RFCs: numbered foundation + mechanism list
4. **Motivation** — the pain points that make this RFC necessary
5. **Goals** — bullet list of what this RFC does
6. **Non-Goals** — explicit exclusions (equally important as Goals)
7. **Guide-level explanation** — how users think about and use the feature
8. **Reference-level explanation** — precise normative rules
9. **Design details** — syntax, semantics, interaction with existing features, compatibility
10. **Alternatives considered** — what was rejected and why
11. **Drawbacks** — honest trade-offs
12. **Implementation architecture** *(non-normative, optional)* — recommended internal approach
13. **Layers affected** — which compiler/tooling layers are touched
14. **Unresolved questions** — open design questions

---

## RFC numbering

Check `workspaces/docs-site/docs/RFCs/` for the highest existing RFC number and increment by one. Do not reuse a number, even for a closed or superseded RFC.

---

## Header block template

```markdown
# RFC NNN: Title

- **Status:** Draft
- **Created:** YYYY-MM-DD
- **Author(s):** Name (@handle)
- **Related:**
    - RFC NNN (brief description)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.X
- **Shipped in:** —
```

Rules:

- "Related" entries are **plain text** — `RFC NNN (description)` — never markdown reference links.
- Leave Issue and RFC PR as `—` until filed.
- `Written against` is the Incan version that was current when the RFC was drafted — the version whose syntax and semantics the RFC assumes. It records context, not intent. It never changes after the RFC is accepted.
- `Shipped in` is left as `—` until the feature is actually released. Never set it speculatively to a planned version — that belongs on the GitHub issue.

---

## "Layers affected" section

This section describes *what is touched*, not *how to implement it step by step*. Prescriptive task lists belong in GitHub issues, not RFCs.

Never name this section "Implementation plan", "Suggested rollout", or similar.

```markdown
## Layers affected

- **Parser / AST**: new syntax or AST nodes needed
- **Typechecker / Symbol resolution**: semantic checks or symbol table changes
- **IR Lowering**: how the new construct lowers to IR
- **Emission**: what changes in generated Rust output
- **Stdlib / Runtime (`incan_stdlib`)**: runtime-side changes
- **Formatter**: new syntax needs formatter support
- **LSP / Tooling**: completion, hover, diagnostics impact
```

Omit layers that are genuinely unaffected.

---

## RFC cross-references

Never use markdown reference-link syntax for RFC cross-references. Always use plain text.

- ✅ `RFC 005 (Rust interop)` or just `RFC 005`
- ❌ `[RFC 005]` — renders as broken literal text without a link definition

---

## Formatting rules

- **No hard wraps** in prose paragraphs. Each paragraph is a single unbroken line; let the renderer wrap. Hard line-breaks at ~100 chars cause WYSIWYG display issues in docs.
- Code blocks in Incan examples use the `incan` language tag.
- Rustdoc `///`/`//!` comments inside code examples: ≤ 120 chars per line.

---

## RFCs are not implementation tickets

An RFC describes **what** the language should do and **why** — not how the compiler internals should be changed to achieve it.

Allowed:

- Describing the user-facing surface (syntax, semantics, type rules, error messages)
- Sharing motivating research, prior art, or design alternatives
- Defining normative rules a future implementer must satisfy
- Non-normative architecture notes that describe a *recommended shape* (clearly labelled as such)

Not allowed:

- "Change `check_expr/calls.rs` to handle …"
- "Add a field to `FunctionInfo` named …"
- "In `lower/decl/functions.rs`, match on …"
- Any prose that reads like a diff or a task list targeting specific files, functions, or data structures

If you find yourself writing file paths or function names, stop and ask: am I describing the *contract*, or am I writing the *implementation*? The RFC owns the contract. The implementation lives in the issue tracker and the code.

---

## Normative language in reference-level explanations

Use RFC-style normative language:

- `must` / `must not` — required/prohibited behavior
- `should` / `should not` — recommended behavior
- `may` — permitted but not required

---

## Unresolved questions

- List one open design question per bullet.
- Each should be answerable before Draft → Planned.
- End the file with:

```html
<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
```

---

## Confidentiality

Never mention internal project names (any unreleased project) in RFC documents. Use generic descriptions such as "future query language surfaces" or "purpose-built libraries".

This also covers internal file references: do not link to or cite internal paths such as `__strategy__/`, research notes, pre-RFC documents, or any folder not part of the public repository surface. If inspiration came from an internal document, describe the concept in the RFC itself without referencing the source.

---

## Pre-submission checklist

- [ ] RFC number is one higher than the current highest in `workspaces/docs-site/docs/RFCs/`
- [ ] Status is `Draft` (all new RFCs start as Draft)
- [ ] Sections in canonical order
- [ ] Header block complete (all eight fields, including `Written against` and `Shipped in: —`)
- [ ] "Related" entries use plain text, not `[RFC NNN]` links
- [ ] No hard-wrapped prose paragraphs
- [ ] "Layers affected" present (not "Implementation plan")
- [ ] No prescriptive implementation prose (no internal file paths, function names, or struct fields)
- [ ] No confidential project names or internal path references
- [ ] Unresolved questions present with closing comment
- [ ] Incan code blocks use `incan` language tag
- [ ] Reference-level explanation uses normative language
