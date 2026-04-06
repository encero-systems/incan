---
name: review
description: Perform a thorough, Incan-project-aware code review. Use when the user asks for a review, code critique, PR review, or says /review. Checks Rust anti-patterns, compiler pipeline completeness, error handling, test coverage, style conventions, and RFC compliance specific to the Incan compiler codebase.
---

# Code Review — Incan Compiler

## Workflow

1. Identify scope: run `git diff main...HEAD` (or read the files specified by the user).
2. Determine which pipeline stages are touched: Parser · Typechecker · Lowering · Emission.
3. If the change is user-facing, stdlib-facing, or architectural, also consult `AGENTS.md`, `architecture.md`, `layering.md`, `readable-maintainable-rust.md`, and `extending_language.md` as relevant.
4. Work through the checklists below in order.
5. Output a structured report (see **Output format**).

---

## Checklist 1 — Blockers (must fix before merge)

These fail CI or panic at runtime. Flag every occurrence.

- [ ] **No `.unwrap()` or `.expect()`** — anywhere, including tests and examples.
  Use `?`, `.map_err(|e| miette!(...))`, or explicit `match`.
  The only accepted exemption is `#[cfg(test)]` blocks in `backend::project` submodules that already carry `#[allow(clippy::unwrap_used)]`.
- [ ] **Test functions that do fallible work return `Result`** — `fn my_test() -> Result<(), Box<dyn std::error::Error>>` + `?`, never `.unwrap()`.
- [ ] **`cargo clippy` clean** — run `cargo clippy --all-targets --all-features` mentally or literally; flag any obvious clippy violations you can identify from the diff.

---

## Checklist 2 — Compiler pipeline completeness

Only applies when the diff touches a language feature (not a pure refactor or docs change).

- [ ] **Feature flows through all relevant stages.** If a new AST node is added:
  - Parsed and stored in the AST?
  - Validated in the typechecker (`check_decl`, `check_expr`, or `collect`)?
  - Lowered in `src/backend/ir/lower/`?
  - Emitted in `src/backend/ir/emit/`?
- [ ] **Out-of-scope features are rejected at the typechecker**, not silently passed to lowering to fail later. Rejection should emit a typed diagnostic from `crates/incan_syntax/src/diagnostics/catalog/errors/`.
- [ ] **Stdlib changes** (`crates/incan_stdlib/stdlib/`) have matching Rust-side backing in `crates/incan_stdlib/src/` and are registered in `STDLIB_NAMESPACES` (`crates/incan_core/src/lang/stdlib.rs`).

---

## Checklist 3 — Rust quality (anti-patterns)

|             Check             |               Bad                             |             Preferred              |
| ----------------------------- | --------------------------------------------- | ---------------------------------- |
| Parameter types               | `&String`, `&Vec<T>`, `&Box<T>`               | `&str`, `&[T]`, `&T`               |
| Type casting                  | `x as u32` (silent truncation)                | `x.try_into()?` or `From`/`Into`   |
| Visibility                    | `pub` on everything                           | `pub(crate)` or private by default |
| Wildcard imports              | `use foo::*`                                  | `use foo::{Bar, Baz}`              |
| Collecting to iterate         | `.collect::<Vec<_>>()` then loop              | chain iterators directly           |
| Owned clone to appease borrow | `.clone()` as a reflex                        | restructure ownership or borrow    |
| Stringly-typed APIs           | `fn set(role: &str)`                          | typed enum / newtype               |
| Code duplication              | identical logic in multiple arms              | extract shared helper before/after |
| Shared-state escape hatch     | `Rc<RefCell<_>>` / `Arc<Mutex<_>>` by default | restructure ownership first        |

- [ ] No instances of the "Bad" column introduced.
- [ ] Every `.clone()` is justified (crossing API boundary, shared ownership, or genuinely necessary).
- [ ] **Important return values use `#[must_use]`** when silently ignoring them would hide a bug.
- [ ] **No overcomplicated solutions** — if the implementation is hard to follow, ask whether a simpler model (fewer types, fewer indirections, a plain function instead of a trait) would cover the actual requirements without loss of correctness.
- [ ] **No shortcuts or overfitting** — the solution should handle the general case described by the RFC or issue, not just the specific example that was tested. Watch for magic constants, hardcoded paths, special-cased logic that only works for the motivating input, or skipped validation that "wasn't needed yet".
- [ ] **Macro discipline** — macros should remove boilerplate, not hide core logic, invariants, or control flow.
- [ ] **Async stays at the edges** — avoid making pure logic async and prefer structured, cancellable concurrency over fire-and-forget task spawning.

---

## Checklist 4 — Error handling

- [ ] Public API errors use a typed error enum (`thiserror`), not `Result<T, String>`.
- [ ] Async functions that do blocking I/O use `tokio::fs` or `spawn_blocking`, not `std::fs`.
- [ ] `miette` diagnostics carry a span and a human-readable message. The error variant lives in the catalog under the right module (`errors/`, `warnings/`).

---

## Checklist 5 — Style and readability

- [ ] **Section headers** (`// ---- Context: ... ----`) in functions ≥ 30 lines or ≥ 3 logical blocks.
- [ ] **Rustdoc line length ≤ 120 chars** for `///` and `//!` comments. Prose in `.md` files is never hard-wrapped.
- [ ] **Changed public APIs have updated docs** — doc comments/rustdoc should still match behavior, invariants, and examples after the change.
- [ ] **Touched non-trivial functions/methods are documented** — not just public APIs. Private helpers may skip rustdoc only when they are genuinely tiny and self-evident.
- [ ] **Docs explain intent and constraints** — especially for public types, traits, derives, runtime adapters, and non-obvious lowering/emission behavior.
- [ ] **Docs-site changes follow repo rules** — no hard wrap under `workspaces/docs-site/`; if docs are touched, consider whether `mkdocs build --strict` should pass.
- [ ] **Boy Scout Rule** — did the author leave touched code at least as clean as they found it? Flag stale TODOs, misleading variable names, missing doc comments, or unused imports that could have been fixed in-scope.
- [ ] No comments that merely narrate what the code does (`// Increment the counter`). Only intent, trade-offs, or non-obvious constraints.

---

## Checklist 6 — Tests

- [ ] **New functionality has tests.** For typechecker changes: unit tests in the `#[cfg(test)]` block. For codegen changes: a fixture in `tests/codegen_snapshots/` and a corresponding snapshot.
- [ ] **Snapshots updated** if codegen changed. Command: `INSTA_UPDATE=1 cargo test --test codegen_snapshot_tests`.
- [ ] **Integration tests** (`tests/integration_tests.rs`) updated if the change affects end-to-end behavior.
- [ ] Both typechecker-level tests (semantic validation) AND codegen snapshot tests (end-to-end) exist for any pipeline feature.
- [ ] **Compiler/runtime parity risks are tested** — when behavior exists in both compile-time checks and runtime helpers, add or verify parity coverage for the edge case.
- [ ] **Tooling parity is preserved** — syntax, diagnostics, formatter, CLI, and LSP behavior stay aligned when shared frontend behavior changes.

---

## Checklist 7 — RFC compliance (if applicable)

- [ ] The implementation matches what the RFC specifies — no accidental scope creep, no missing requirements.
- [ ] Items explicitly marked "out of scope" or "not supported" in the RFC are rejected at the typechecker with a clear diagnostic, not silently accepted.
- [ ] RFC number is referenced in relevant doc comments or commit message.

---

## Checklist 8 — Architecture and layering

- [ ] **Changes live in the correct layer/crate** — `incan_syntax` stays syntax-only; `incan_core` stays pure/deterministic; orchestration layers stay thin.
- [ ] **Layering rules are preserved** — `incan` must not depend on `incan_stdlib` except as a dev-dependency; shared policy belongs in `incan_core`, runtime glue belongs in `incan_stdlib`.
- [ ] **No duplicated policy across layers** — if parser/typechecker/lowering/CLI/LSP need the same rule, prefer a shared helper, registry, or semantic pack.
- [ ] **Registry-driven behavior stays registry-driven** — stdlib namespaces, soft keywords, surface semantics, and runtime requirements should extend the canonical registries rather than add hardcoded special cases.
- [ ] **Runtime/compiler boundaries stay clean** — generated-program helpers belong in runtime crates; compiler logic belongs in compiler crates; avoid hidden drift between the two.
- [ ] **Compiler/runtime parity uses `incan_core`** — shared semantics, canonical error text, and policy should come from `incan_core` rather than duplicated logic or string literals.
- [ ] **Single source of truth is preserved** — do not recreate ad hoc registries, handwritten mirrors, or one-off resolution rules when a canonical table/module already exists.
- [ ] **Language features use the right path** — prefer stdlib functions or builtins over new syntax unless control flow, evaluation rules, or typing rules genuinely require syntax; for import-activated features, prefer the semantics-pack path over ad hoc keyword handling.

---

## Checklist 9 — Contributor hygiene

- [ ] **User-facing changes update the right docs** — rustdoc, docs-site pages, examples, and release notes stay aligned when behavior changes.
- [ ] **Repo learnings are captured when warranted** — if the change taught a durable lesson about architecture, testing, or pitfalls, consider whether `AGENTS.md` should be updated.

---

## Output format

Produce a report with:

```md
## Review — <subject / branch / files>

### Blockers 🔴
<item> — <file>:<line> — <explanation and fix>

### Warnings 🟡
<item> — <file>:<line> — <explanation and fix>

### Suggestions 🟢
<item> — <file>:<line> — <optional improvement>

### Notes 💡
<observation or question that doesn't require a change>

### Summary
One or two sentences on overall quality and merge-readiness.
```

- **Blocker 🔴** — must fix before merge (clippy deny, `.unwrap()`, pipeline gap, missing test for new feature).
- **Warning 🟡** — should fix (anti-pattern, style violation, missing doc comment on a touched public or non-trivial item).
- **Suggestion 🟢** — nice to have (minor readability, opportunistic Boy Scout improvement).
- **Note 💡** — observation, question, or context (no change required).

Omit sections that have no items. If there are zero blockers and zero warnings, say so explicitly in the summary.
