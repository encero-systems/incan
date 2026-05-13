---
name: review
description: Perform a thorough, Incan-project-aware code review. Use when the user asks for a review, code critique, PR review, or says /review. Checks Rust anti-patterns, compiler pipeline completeness, error handling, test coverage, style conventions, and RFC compliance specific to the Incan compiler codebase.
---

# Code Review — Incan Compiler

For broad dirty worktrees or explicitly delegated review, prefer `review-orchestrate` over this generic reviewer. Use `/review` for smaller or local review passes.

## Persistent report

`/review` must keep a live report in the current worktree at:

- `.agents/state/review-report.md`

Do not keep the findings list only in memory. Create or update this file as you review.

The report is the shared source of truth for `/fix`, `/loop`, `/review-and-fix`, and `ralph-loop`.

The report must use this exact top-level shape:

```md
# Review Report

- branch: <branch or worktree>
- status: in_progress | clean | blocked

## Scope
- user request:
- issue / RFC:
- reviewed worktree: true

## Activity
- [21:54] review started
- [21:55] scope derived from issue 73 / RFC 015 / dirty worktree
- [21:57] found 2 warnings in src/project_lifecycle/version.rs
- [22:01] running make fmt
- [22:04] make pre-commit passed

## Files

### <path/to/file_a>
- status: pending | clean | findings | fixed | blocked
- checks:
  - scope_audit: pass | fail | n/a
  - rust_prose_audit: pass | fail | n/a
  - markdown_prose_audit: pass | fail | n/a
  - rfc_reference_audit: pass | fail | n/a
  - docs_claims_audit: pass | fail | n/a
  - public_docs_audit: pass | fail | n/a
  - test_style_audit: pass | fail | n/a
  - release_note_inventory_audit: pass | fail | n/a
- notes:
  - touched and reviewed

### <path/to/file_b>
- status: findings
- checks:
  - scope_audit: pass
  - rust_prose_audit: fail
  - markdown_prose_audit: n/a
  - rfc_reference_audit: n/a
  - docs_claims_audit: n/a
  - public_docs_audit: pass
  - test_style_audit: n/a
  - release_note_inventory_audit: n/a
- findings:
  - [ ] blocker | <label> | <line>
    <why it matters>

### <path/to/file_c>
- status: fixed
- checks:
  - scope_audit: pass
  - rust_prose_audit: pass
  - markdown_prose_audit: n/a
  - rfc_reference_audit: n/a
  - docs_claims_audit: n/a
  - public_docs_audit: pass
  - test_style_audit: n/a
  - release_note_inventory_audit: n/a
- findings:
  - [x] warning | <label> | <line>
    <what changed>

### <path/to/file_d>
- status: blocked
- checks:
  - scope_audit: pass
  - rust_prose_audit: fail
  - markdown_prose_audit: n/a
  - rfc_reference_audit: n/a
  - docs_claims_audit: n/a
  - public_docs_audit: pass
  - test_style_audit: n/a
  - release_note_inventory_audit: n/a
- findings:
  - [ ] external_blocker | <label> | <command or line>
    <why it is blocked>

## Verification
- <command> — <result>
```

Every touched/reviewed file in scope must appear under `## Files`. Start each file as `status: pending`; do not mark a file `clean` before you actually audit it. Do not keep a separate mental list of "files I looked at". If a finding changes state during the review, update the report file instead of relying on a rewritten summary later.

For each file, fill out the applicable checks explicitly:

- `scope_audit`: expected scope vs delivered scope for that file
- `rust_prose_audit`: touched `.rs` prose comments and rustdoc
- `markdown_prose_audit`: touched `.md` prose quality
- `rfc_reference_audit`: user-facing docs leaking RFC references
- `docs_claims_audit`: docs/CLI/examples claiming unimplemented behavior
- `public_docs_audit`: public or non-trivial docs/comments match behavior
- `test_style_audit`: panic helpers / unwrap / expect / bad test style in touched tests
- `release_note_inventory_audit`: implemented user-facing work reflected in release-note inventory where applicable

Use `n/a` only when the check genuinely does not apply to that file.

`## Activity` is append-only. Add a short timestamped entry every time you:

- start a review pass,
- derive scope,
- discover a new finding,
- run a meaningful verification command,
- decide to classify something as blocked,
- finish a pass as clean or blocked.

A prose summary that omits `## Activity` or `## Files` is invalid output. Do not replace the report with a narrative summary at the end.

Each new `/review` invocation starts a fresh report for the current run. Do not inherit findings, statuses, or activity entries from an older run. At the start of the review, overwrite `.agents/state/review-report.md` with a fresh scaffold using the required sections, then populate it from the current worktree state.

## Workflow

1. Identify scope from the **current worktree**, not just committed history: inspect `git status --short`, `git diff main...HEAD`, and any files the user names. Review staged, unstaged, and untracked files that are part of the current task; do not silently narrow the review to committed diffs when the worktree is dirty.
2. Derive the **expected scope** before judging the implementation. Use, in order: the user's request in the thread, the issue number / branch name / short description when available, the governing RFC or design doc, and the generated user-facing docs/CLI reference/examples on the branch. Write down, at least mentally, what behavior the branch claims to implement.
3. Check for both directions of scope drift:
   - **missing in-scope behavior** that the issue/RFC/docs imply should exist but does not, and
   - **out-of-scope additions** or scope creep that the governing spec did not ask for.
4. Determine which pipeline stages are touched: Parser · Typechecker · Lowering · Emission.
5. If the change is user-facing, stdlib-facing, or architectural, also consult `AGENTS.md`, `architecture.md`, `layering.md`, `readable-maintainable-rust.md`, and `extending_language.md` as relevant.
6. If review uncovers a likely compiler bug outside the current change scope, trigger `flag-compiler-bug`: minimize the repro, judge blocking vs workaround, check whether the bug is already filed, and then raise or draft the bug before continuing.
7. Work through the checklists below in order.
8. **Stay in review mode.** `/review` is report-only unless the user explicitly asks for edits inside the same request. Do not silently fix findings by default; that responsibility belongs to `/fix` or `/review-and-fix`.
9. Do an explicit **prose audit** on touched documentation-bearing files before you finalize findings:
   - for every touched `.rs` file containing `///`, `//!`, or prose `//`, inspect the touched prose blocks directly rather than assuming you would notice bad wrapping incidentally;
   - for every touched `.md` file, inspect the touched paragraphs directly for short-prosed or mechanically chopped text.
10. Record findings in `.agents/state/review-report.md` as you go. Do not wait until the end to reconstruct the review from memory.
11. At the start of the run, reset `.agents/state/review-report.md` to a fresh scaffold for the current review. Do not preserve stale entries from prior runs.
12. Maintain `## Activity` as a live audit trail while you work. Do not backfill it only at the end.
13. Maintain the report as a per-file ledger: every touched/reviewed file gets an explicit entry, status, and checklist.
14. A file may move from `pending` to `clean` only after the applicable checks are filled in and all are `pass` or `n/a`.
15. If the report is missing `## Scope`, `## Activity`, `## Files`, or `## Verification`, the review is incomplete. Fix the report before you declare any result.
16. Output a structured report (see **Output format**) describing the current review state of the worktree.
17. Before declaring the review clean, work through the **mandatory closing checklist** below and make sure the persistent report matches it. Do not rely on memory or a vague sense that the branch is "basically done".

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
- [ ] **Code comment prose is paragraph-shaped** — prose in `//`, `///`, and `//!` comments should be written as normal paragraphs, not manually width-managed line-by-line. For Rust comments, write the paragraph naturally and let `make fmt` do the width wrapping. Split prose into separate paragraphs only for real semantic breaks, not to manage width or visual shape. Prose in `.md` files is never hard-wrapped.
- [ ] **Rust prose comments are not manually chopped** — in touched `.rs` files, `///`, `//!`, and prose `//` comments must not be manually broken into clause-by-clause or narrow-column lines. The acceptable shape is: a natural paragraph that `make fmt` wraps, or multiple paragraphs when the content has a real meaning break. Treat pre-broken short-line rustdoc as a documentation defect and fix it during review.
- [ ] **Touched Rust prose blocks were explicitly audited** — do not rely on incidental reading. In every touched `.rs` file with rustdoc/prose comments, inspect the touched comment blocks directly for manual chopping, narrow-column wrapping, or fake paragraph breaks.
- [ ] **Markdown prose is not short-prosed** — in touched `.md` files, prose paragraphs should read like natural paragraphs, not a stack of short broken lines or mechanically chopped clauses. This is distinct from the repo's "no hard wrap in docs-site markdown" rule: even in non-docs-site markdown, avoid narrow-column prose unless structure genuinely requires it.
- [ ] **Touched Markdown prose was explicitly audited** — do not assume a broad doc read will surface every chopped paragraph. Inspect the touched paragraphs directly in each changed `.md` file.
- [ ] **Changed public APIs have updated docs** — doc comments/rustdoc should still match behavior, invariants, and examples after the change.
- [ ] **Touched Incan declarations have descriptive docstrings** — use the Python-quality rule: every touched `.incn` module, class, model, enum, trait, function, and method needs a descriptive docstring, including private/internal helpers. This is stricter than Rust prose rules and mirrors the expected ruff/pylint/mypy-style Python documentation discipline.
- [ ] **Touched non-trivial Rust functions/methods are documented** — not just public APIs. Private Rust helpers may skip rustdoc only when they are genuinely tiny and self-evident.
- [ ] **Docs explain intent and constraints** — especially for public types, traits, derives, runtime adapters, and non-obvious lowering/emission behavior.
- [ ] **Docs-site changes follow repo rules** — no hard wrap under `workspaces/docs-site/`; if docs are touched, consider whether `mkdocs build --strict` should pass.
- [ ] **User-facing docs do not lean on RFC references** — pages under user docs (tutorials, how-to, reference, release notes, tooling/language guides) should explain behavior directly. Flag direct RFC references in user-facing prose unless the page is itself an RFC/contributor-facing document or the RFC link is part of an explicit release-note inventory.
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

## Checklist 7b — Claimed scope vs delivered scope

- [ ] **Expected scope was derived explicitly** — from the user request, issue/branch metadata, RFC/design docs, and generated docs/examples on the branch.
- [ ] **Promised user-facing behavior exists** — if the CLI reference, tutorials, generated scaffolds, release notes, or examples claim a behavior, the implementation actually provides it.
- [ ] **No missing surface hidden by passing tests** — do not stop at internal consistency; check whether the branch actually completes the feature it claims to implement.
- [ ] **No docs-generated fiction** — docs may explain or simplify behavior, but they must not advertise commands, flags, flows, defaults, or UX that the code does not implement.
- [ ] **Issue/branch intent is satisfied** — if the issue number, branch slug, or short description implies a concrete feature slice, verify that slice is materially complete or call out what is still missing.

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
- [ ] **Release-note inventories stay complete** — when the branch implements or materially completes an RFC/user-facing feature, verify the current release notes mention it in the implemented/features inventory where this repo expects that summary to live.
- [ ] **Generated language reference is current** — if the change touches `crates/incan_core/src/lang/`, `crates/incan_core/src/bin/generate_lang_reference.rs`, or `workspaces/docs-site/docs/language/reference/language.md`, run `cargo run -p incan_core --bin generate_lang_reference`, inspect the resulting `language.md` diff, and require that generated diff to be committed or explicitly reported as blocked.
- [ ] **Compiler bugs discovered during review are surfaced explicitly** — if you find a likely compiler defect that should not be fixed inside the current change, invoke `flag-compiler-bug` instead of burying it in review notes.
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

### Follow-up bugs
<drafted issue or explicit recommendation to file one, when review uncovered a likely compiler bug outside scope>

### Summary
One or two sentences on overall quality and merge-readiness.
```

- **Blocker 🔴** — must fix before merge (clippy deny, `.unwrap()`, pipeline gap, missing test for new feature).
- **Warning 🟡** — should fix (anti-pattern, style violation, missing doc comment on a touched public or non-trivial item).
- **Suggestion 🟢** — nice to have (minor readability, opportunistic Boy Scout improvement).
- **Note 💡** — observation, question, or context (no change required).

Omit sections that have no items. If there are zero blockers and zero warnings, say so explicitly in the summary.

## Mandatory closing checklist

Before you write "no blockers", "no warnings", or equivalent clean-review language, explicitly verify all of the following:

- [ ] `.agents/state/review-report.md` exists, matches the current review state, and includes every touched/reviewed file in scope.
- [ ] `.agents/state/review-report.md` contains `## Scope`, `## Activity`, `## Files`, and `## Verification`.
- [ ] `## Activity` is a real timestamped log of the run, not a rewritten end summary.
- [ ] No file is marked `clean` without a filled-in applicable checklist; untouched placeholders stay `pending`, not `clean`.
- [ ] Dirty-worktree files were reviewed too, including untracked files that are part of the task.
- [ ] Touched `.rs` files do not contain manually chopped rustdoc or prose comments; each prose block is either one natural paragraph wrapped by `make fmt` or multiple paragraphs separated for a real semantic reason.
- [ ] Every touched `.rs` file with prose comments was explicitly audited block-by-block, not just skimmed.
- [ ] Touched `.md` files do not contain short-prosed or mechanically chopped paragraphs.
- [ ] Every touched `.md` file was explicitly audited paragraph-by-paragraph where prose changed.
- [ ] Touched tests do not introduce panic helpers, panic-style control flow, `.unwrap()`, or `.expect()`.
- [ ] User-facing docs do not casually lean on RFC references unless the page is explicitly RFC/contributor-facing or the RFC appears in an explicit release-note inventory.
- [ ] Docs, CLI help, examples, scaffolds, and release notes do not claim behavior the code does not implement.
- [ ] Release-note inventories were updated where the repo expects them for implemented user-facing work.
- [ ] If language registries or generated language-reference inputs changed, `cargo run -p incan_core --bin generate_lang_reference` was run and any resulting `workspaces/docs-site/docs/language/reference/language.md` diff was reviewed and committed.
- [ ] `make fmt` was run after fixes.
- [ ] `make pre-commit` was run after the final fix pass, or you explicitly report why it was not run.

If any item is unchecked, do not claim the review is clean. Report the remaining gap explicitly.

## Verification failure policy

A failed broad verification command does **not** invalidate the review.

If `make fmt`, `make pre-commit`, `mkdocs build --strict`, or another broad check fails for reasons unrelated to the
current local findings, you must still:

1. keep reviewing the worktree,
2. run the narrow relevant verification you still can,
3. report the broad-check failure as residual risk.

Do **not** abandon the review merely because a broad verification command failed.

If the user explicitly asked for edits as part of the review request, use `/fix` or `/review-and-fix` rather than improvising your own repair loop.

Only stop short of a stronger verification claim when the gap is clearly one of:

- out of scope,
- risky without user confirmation,
- externally blocked, or
- a separate compiler bug that should be surfaced via `flag-compiler-bug`.

For every unresolved verification gap or blocked check, say which of those categories applies.

## Default behavior

When the user invokes `/review` or otherwise asks for a review without explicitly saying "report only" or "do not change code", you should:

1. review the code,
2. run relevant verification,
3. update `.agents/state/review-report.md`,
4. produce findings only,
5. recommend `/fix` or `/review-and-fix` if actionable findings remain.

Before declaring the loop clean, do one explicit final sweep for the common misses this skill is meant to catch:

- dirty-worktree files that were never reviewed because they were untracked or unstaged,
- promised scope that was never actually implemented even though the docs/branch title imply it,
- manually chopped Rust prose comments in touched `.rs` files,
- short-prosed Markdown paragraphs in touched `.md` files,
- direct RFC references in user-facing docs, and
- missing release-note inventory updates for implemented user-facing work, and
- stale generated language reference output after registry or generator changes.

The default verification bar for a strong review is:

- `make fmt`
- `make pre-commit`

If either command was not run, say so explicitly in the final review report and treat that as residual risk rather than silently claiming the branch is clean.
