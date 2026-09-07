# Agent Instructions for Incan Development

Incan is a Python-like language that compiles to Rust. The compiler itself is written in Rust and generates native Rust code via an IR-based pipeline. This document contains guidance for AI agents working on the codebase.

> **CRITICAL — NO `.unwrap()` / `.expect()` ANYWHERE.** This is the single most important rule.
> Multiple modules enforce `#![deny(clippy::unwrap_used)]` and `#![deny(clippy::expect_used)]`.
> This applies to **all** code — production, tests, examples. No exceptions, no shortcuts.
> Use `?` with `Result`-returning test functions, or propagate errors explicitly.
> See [Error handling in tests](#error-handling-in-tests) for the correct pattern.

> **CRITICAL — DRIVE THE GOAL TO COMPLETION.** When the maintainer sets a goal, pursue it autonomously until it is delivered or provably impossible. Commit, push, and open PRs on your own initiative. Do not stop to report progress, seek reassurance, or hand back a decision the evidence already settles. If one part turns out to be impossible, finish every other part first, then state the blocking mechanism in a sentence or two — not a status essay.
>
> **A blocker is a claim that needs proof.** Read the owning issue’s own scope and “Done when” list bullet by bullet before calling anything blocked: one gated bullet does not gate the issue. Quote the exact spec text that is silent or contradictory. If you cannot quote it, keep working. Check that what you assume is missing is actually missing — grep for the type or function before declaring it unbuilt.
>
> **Do not manufacture decisions.** Ask only when the answer changes what you do next *and* the evidence genuinely does not settle it. Sizing your own PRs, reclaiming regenerable build caches, choosing between equivalent spellings, and ordinary cleanup are not the maintainer’s call. When a message asks *why* you did something, answer the question — do not start changing it.
>
> **CRITICAL — THE USER DECIDES WHAT IS RELEVANT.** Scope, PR boundaries, and which files “belong” on a branch are **the maintainer’s call**, not the agent’s. Never label work as “unrelated PR noise,” “cleanup,” or “hygiene” as a reason to remove or revert it.
>
> **FORBIDDEN without explicit user approval that quotes the exact paths or commands:** anything that overwrites or deletes uncommitted work — including `git checkout -- <path>`, `git restore <path>`, `git clean`, `git reset --hard`, `stash drop`, or equivalent — and force-pushing a shared branch or one whose PR has already merged. If you believe files should be split, reverted, or left out of a PR, **state that and ask**; do not run destructive git operations on your own initiative.
>
> **Commits and pushes are yours.** Commit your own work using the repo’s message convention, push the branch, and open the PR when it is ready. Two rules keep that safe: re-check a PR’s state immediately before pushing to its branch, because a squash-merge silently strands any later push; and prove work reached the integration branch by content (`git show origin/<dev-line>:<file> | grep <symbol>`), never by PR status alone. Sync a pushed branch with a merge commit rather than a rebase, so ancestry survives and no force-push is needed.

## Key References

| Document                 | Path                                                                                 |
| ------------------------ | ------------------------------------------------------------------------------------ |
| Rust coding conventions  | [`workspaces/docs-site/docs/contributing/explanation/readable-maintainable-rust.md`] |
| Project architecture     | [`workspaces/docs-site/docs/contributing/explanation/architecture.md`]               |
| Layer boundaries         | [`workspaces/docs-site/docs/contributing/explanation/layering.md`]                   |
| Writing RFCs             | [`workspaces/docs-site/docs/contributing/how-to/writing_rfcs.md`]                    |
| Contributor guide        | [`CONTRIBUTING.md`]                                                                  |
| GitHub issue templates   | [`.github/ISSUE_TEMPLATE/`]                                                          |
| Implementation learnings | [`.agents/learnings.md`]                                                             |

[`workspaces/docs-site/docs/contributing/explanation/readable-maintainable-rust.md`]: workspaces/docs-site/docs/contributing/explanation/readable-maintainable-rust.md
[`workspaces/docs-site/docs/contributing/explanation/architecture.md`]: workspaces/docs-site/docs/contributing/explanation/architecture.md
[`workspaces/docs-site/docs/contributing/explanation/layering.md`]: workspaces/docs-site/docs/contributing/explanation/layering.md
[`workspaces/docs-site/docs/contributing/how-to/writing_rfcs.md`]: workspaces/docs-site/docs/contributing/how-to/writing_rfcs.md
[`CONTRIBUTING.md`]: CONTRIBUTING.md
[`.github/ISSUE_TEMPLATE/`]: .github/ISSUE_TEMPLATE/
[`.agents/learnings.md`]: .agents/learnings.md

Skills (`.agents/skills/`) and learnings (`.agents/learnings.md`) live under **this repository’s** `.agents/` directory and are committed here. Working state and review-run artifacts (`.agents/state/`) live in the same directory but are **not** committed — they're gitignored, per-machine scratch, except `.agents/state/findings-ledger.md` which is a symlink into a private workspace-level log (see `.agents/skills/review/SKILL.md` and `.agents/skills/ralph-loop/SKILL.md` for how it's used).

## v0.6 execution ledger

For every implementation issue in the `0.6 Release` milestone, [#1074](https://github.com/encero-systems/incan/issues/1074) is the central execution ledger. The RFC and owning issue remain the semantic authority; the ledger records delivery state, dependencies, and evidence across the programme.

- Post a structured `Active` update on #1074 once the work has a branch or PR, identifying the issue, intended scope, dependencies, and verification plan. Posting it is not a gate on starting: begin the work, then record it.
- Post another update when the work is blocked, ready for integration, materially rescaled, or completed. Do not call v0.6 work ready to merge or complete without its ledger evidence.
- Use the update format published on #1074. Updates are append-only comments so concurrent contributors do not overwrite each other; do not edit another contributor's update.
- Keep public updates reproducible and repository-scoped: no local absolute paths, private environment details, or claims that unrun verification passed.

This requirement applies to contributors and automated implementation workflows alike. If the current task does not authorize a GitHub write, put the update text in the PR description and carry on — do not pause the work waiting to publish it.

Keep updates to the evidence: what changed, what proves it, what is left, and the mechanism blocking anything that is. Do not narrate process.

## General Workflow

1. **Branch from main**: Create a feature branch using the naming convention `<type>/<issue>-<slug>`, where type is `feature`, `chore`, or `bugfix`. Examples: `feature/165-implement-rfc-031-library-system-phase-1`, `chore/88-vocab-drift-guardrails`, `bugfix/42-fix-parser-crash`. Use the `/start-work` skill to automate this.
2. **Follow RFCs**: RFCs in `workspaces/docs-site/docs/RFCs/` are the spec — implement exactly what they say.
3. **Run tests**: `make test` must pass before considering work complete. Run targeted tests during development; run the full suite when you finish.
4. **Update snapshots**: `INSTA_UPDATE=1 cargo test --test codegen_snapshot_tests` to update changed snapshots.
5. **Boy Scout Rule**: Leave every file you touch in better shape than you found it — fix stale TODOs, missing doc comments, unused imports, misleading names.
6. **Documentation gate (mandatory)**: Before finalizing any change, audit every touched Rust module and ensure rustdocs are present and accurate for all new/changed functions and methods in changed Rust source files. This is enforced mechanically by `scripts/check_changed_rustdocs.py` through `make pre-commit-fast` and `make pre-commit`.

### Pattern intake before edits

Before touching production code, tests, or docs for a non-trivial change, do a short pattern intake:

- Identify the active area: parser, typechecker, lowering, emission, stdlib, CLI/tooling, docs, or tests.
- Read 2-3 nearby files that already implement the same kind of behavior. Prefer same-stage and same-domain precedents over generic Rust examples.
- Name the source-of-truth boundary or registry when one exists, such as the RFC, stdlib registry, diagnostics catalog, ownership policy, or CLI contract.
- State which verification path must prove the change, including whether parser/typechecker coverage, codegen snapshots, integration tests, docs build, or feature-specific builds are required.

Do not substitute broad advice like "follow Rust best practices" for local precedent. Incan patterns are stage- and boundary-specific; copying a shape from the wrong compiler layer is a common way to create drift.

### Rust compiler error intake

When debugging a Rust compiler error, capture the full error context before proposing a fix:

- the exact command that failed;
- the complete diagnostic, including error code, notes, help text, and secondary spans;
- active feature flags or build mode, especially default build vs. `rust-metadata`;
- the relevant function signature and nearby type definitions;
- the local files or tests that establish the intended pattern.

Classify the root cause before editing: lifetime/borrow across boundary, trait bound, feature gate/build-mode mismatch, orphan/coherence rule, missing import, or pipeline-stage wiring. Avoid applying a local `.clone()`, `.into()`, `.as_ref()`, or type annotation workaround until the owning boundary is clear.

### Common commands

| Command                                                   | Purpose                                                               |
| --------------------------------------------------------- | --------------------------------------------------------------------- |
| `make build`                                              | Debug build (fast)                                                    |
| `make release`                                            | Optimized build                                                       |
| `make test`                                               | Run all tests                                                         |
| `make fmt`                                                | Format Rust code (`cargo +nightly fmt`)                               |
| `make lint`                                               | Run clippy                                                            |
| `make check`                                              | Format check + clippy                                                 |
| `make pre-commit-fast`                                    | Fast local gate (format check + `cargo check`)                        |
| `make pre-commit`                                         | Full local gate (full checks + smoke-test-fast)                       |
| `make smoke-tests`                                        | Full smoke test: tests + release canary + examples + benchmarks-incan |
| `make examples`                                           | Smoke test all examples (requires release build)                      |
| `INSTA_UPDATE=1 cargo test --test codegen_snapshot_tests` | Update codegen snapshots                                              |

## Docs-site Workflow (MkDocs Material)

When making changes under `workspaces/docs-site/`:

- **Build docs locally**: run `mkdocs build --strict` from `workspaces/docs-site` to catch broken links/anchors early.
- **Line length: no hard wrap** for docs-site `.md` files. Write prose as natural paragraphs — let the renderer handle wrapping. This applies to all markdown under `workspaces/docs-site/` and other non-code markdown files.

## Code Style

Read and follow [`workspaces/docs-site/docs/contributing/explanation/readable-maintainable-rust.md`] for the project's Rust coding conventions.

### Inline section headers

In longer functions (roughly 30+ lines or 3+ logical blocks), use `// ----` section headers to delineate logical blocks:

```rust
// ---- Context: stdlib module completions (`from std.` / `import std::`) ----
if let Some(stdlib_items) = stdlib_module_completions(&line_prefix) {
    return Ok(Some(CompletionResponse::Array(stdlib_items)));
}

// ---- Context: decorator completions (`@` at line start) ----
if let Some(decorator_items) = decorator_completions(&line_prefix) {
    return Ok(Some(CompletionResponse::Array(decorator_items)));
}
```

Guidelines:

- Keep a blank line **before** each header for visual breathing room.
- The label after `----` should describe *what* or *when*, not *how*.
- Don't overuse: if a function has only one or two simple blocks, a plain `//` comment is enough.
- These are for **intra-function** organisation. For module-level sections, use `// ============` banners.

### Formatting

- **Do not manually optimize Rust comment or rustdoc line length.** Agents do not need to worry about rustdoc line length at all; `make fmt` takes care of formatting.
- **Do not introduce staircase-wrapped prose** in `///`, `//!`, or prose `//` comments. Avoid mechanically chopping one paragraph into many short lines.
- **Keep Rust prose comments paragraph-shaped.** Break lines only when structure requires it: bullets, tables, code blocks, deliberate emphasis, or a clean sentence/ clause boundary that genuinely improves readability.
- **Prefer fewer fuller lines over many short lines** in Rust prose comments. If a rustdoc paragraph reads like a narrow column, it is probably wrong.
- Use `make fmt` to format the codebase after making changes, and before running tests.
- If you touch comment prose that is already awkwardly short-wrapped, rewrite it as a natural paragraph before running `make fmt`; do not assume rustfmt will expand it for you.

### Documentation requirements (mandatory)

Agents must treat documentation updates as part of implementation, not optional polish.

- **Public API docs are required**: Any new/changed `pub` module, type, enum variant intent, struct field intent (when not obvious), and function/method must have rustdoc that explains purpose and contract.
- **Error types need variant-level docs**: For `thiserror` enums and diagnostic types, document what each variant represents and when it is emitted.
- **Non-trivial functions and methods need docs**: New or changed functions/methods should carry rustdoc/doc comments unless they are genuinely tiny and self-evident local helpers. Prefer documenting all touched functions over debating edge cases.
- **Cross-stage and boundary helpers always need docs**: Parser/typechecker/lowering/emission/interop/conversion helpers must document purpose, invariants, and why the boundary exists, even when they are private.
- **Tiny obvious helpers are the only exception**: A very small private helper may skip rustdoc only when its name and body make the intent completely obvious and there are no invariants, fallback paths, ownership assumptions, or feature-gated behaviors to explain.
- **Behavioral boundaries must be explicit**: For pipeline boundaries (parser -> desugar -> typecheck -> lowering), docs should state what must and must not cross the boundary.
- **Docs should explain why, not narrate syntax**: Explain purpose, contracts, fallbacks, ownership/borrowing assumptions, and misuse risks. Avoid comments that merely restate the code line-by-line.
- **Rust prose comments should not be manually hard-wrapped for width**: when editing `///`, `//!`, or prose `//` comments, keep the prose natural and let formatting tools do their job. Short, choppy comment wrapping is considered a documentation defect.
- **Changed Rust source functions and methods must have rustdoc**: the mechanical gate checks changed Rust source files and fails if a function or method definition lacks a preceding rustdoc block.
- **Done criterion**: Do not mark work complete until this rustdoc audit is done for all touched files.

## Rust Anti-Patterns to Avoid

The project style guide covers broad principles. This section is a concrete quick-reference of patterns agents **must not** introduce.

| Instead of                                    | Prefer                                 | Why                                                               |
| --------------------------------------------- | -------------------------------------- | ----------------------------------------------------------------- |
| `.unwrap()` / `.expect("…")` **anywhere**     | `?`, `.context()`, or explicit `match` | Panics crash the compiler; deny lints reject these in CI          |
| `.clone()` to appease the borrow checker      | Restructure ownership or borrow        | Hides design issues and adds unnecessary allocations              |
| `&String`, `&Vec<T>`, `&Box<T>` in parameters | `&str`, `&[T]`, `&T`                   | More general — accepts owned and borrowed callers alike           |
| `x as u32` (silent truncation)                | `x.try_into()` or `From`/`Into`        | `as` silently wraps/truncates; conversions should be explicit     |
| `use foo::*` (wildcard imports)               | `use foo::{Bar, Baz}`                  | Makes origins clear; avoids surprise breakage on upstream changes |
| `.collect::<Vec<_>>()` just to re-iterate     | Chain iterators directly               | Avoids an unnecessary allocation + copy                           |
| `pub` on everything                           | `pub(crate)` or private by default     | Minimize public surface; promote visibility only when needed      |
| Blocking I/O in `async fn`                    | `tokio::fs`, `spawn_blocking`          | Blocks the executor and starves other tasks                       |
| `Result<T, String>` in public APIs            | A typed error enum (`thiserror`)       | Stringly-typed errors are hard to match and evolve                |
| `Rc<RefCell<T>>` everywhere                   | Restructure data / ownership           | Usually signals a design that fights the borrow checker           |

### Error handling — NEVER use `.unwrap()` or `.expect()`

**This is non-negotiable.** Any `.unwrap()` or `.expect()` call — in production code **or test code** — will be rejected by clippy and fail CI. Always propagate errors with `?`:

```rust
// WRONG — will not compile due to deny lint
let file = File::open(path).unwrap();

// CORRECT — propagate with ?
let file = File::open(path)
    .map_err(|e| miette!("failed to open {}: {e}", path.display()))?;
```

#### Error handling in tests

Test functions that perform fallible operations **must** return `Result` and use `?`:

```rust
// CORRECT — return Result, use ?
#[test]
fn my_test() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    fs::create_dir_all(tmp.path().join("src"))?;
    Ok(())
}
```

### Clippy is mandatory

Run `cargo clippy` and fix warnings before submitting.

## Compiler Pipeline

```text
Source → Lexer → Parser/AST → Typechecker → Lowering (AST→IR) → Emission (IR→Rust)
```

Key directories:

- `crates/incan_syntax/src/parser/` — Parser and AST definitions
- `src/frontend/typechecker/` — Type checking and semantic analysis
- `src/backend/ir/lower/` — AST to IR lowering
- `src/backend/ir/emit/` — IR to Rust code emission

## Code Locations Reference

| Feature          | Parser                                                 | Typechecker                                                         | Lowering        | Emission        |
| ---------------- | ------------------------------------------------------ | ------------------------------------------------------------------- | --------------- | --------------- |
| Field metadata   | `parser/decl.rs`                                       | `check_decl.rs`                                                     | `lower/decl.rs` | `emit/decls.rs` |
| Alias resolution | -                                                      | `check_expr/access.rs`, `calls.rs`, `match_.rs`                     | `lower/expr.rs` | -               |
| Soft keywords    | `parser/core.rs`, `parser/helpers.rs`, `parser/decl/*` | `collect/stdlib_imports.rs`                                         | -               | -               |
| Stdlib registry  | -                                                      | `incan_core::lang::stdlib` (`crates/incan_core/src/lang/stdlib.rs`) | -               | -               |
| Diagnostics      | -                                                      | `diagnostics/catalog/errors/*` in `crates/incan_syntax/src/`        | -               | -               |

## Available Skills and Agents

### Skills

Skills are reusable workflows in `.agents/skills/`. Use them by name when the task matches. This table is checked against `.agents/skills/` by `make agents-doc-sync` (part of `make pre-commit-fast`) -- if you add or remove a skill, update this table in the same change or the local gate will fail.

**Core workflow**

| Skill | Trigger | What it does |
| --- | --- | --- |
| `/start-work` | Starting work on an issue or RFC | Creates branch, gathers context from issue/RFC, checks learnings; does **not** commit (maintainer-only commits unless explicitly asked) |
| `/create-plan` | Drafting an implementation plan before coding | TDD-oriented plan with doc updates and verification commands for encero-workspace repos |
| `/test` | Writing tests for a change | Guides test selection, provides correct patterns per compiler stage |
| `/write-commit-message` | Drafting a commit message | Formats `chore\|bugfix\|feature - <issue_id(s)> <description>` per workspace convention |
| `/create-pr-description` | Drafting a PR description | Fills in the repo's PR template from the current git diff |
| `/yeet-pr` | Publishing local changes to GitHub | Commits/pushes/opens a review-ready or draft PR per repo commit-message and PR-description conventions |
| `/create-github-issue` | Filing a GitHub issue | Drafts title/body using the target repo's issue templates |
| `/closeout` | Cleaning up after a merged PR | Removes task-owned worktrees/branches, prunes refs, reports dirty or ambiguous leftovers |
| `/fleet-audit` | Checking what worktrees can be cleaned up | Fleet-wide report of protected/safe-to-close/needs-review worktrees under `tmp/`; never deletes anything itself |

**Review and repair**

| Skill | Trigger | What it does |
| --- | --- | --- |
| `/review` | Code review, PR review | Runs the full Incan-aware review checklist |
| `/fix` | Repairing review findings | Fixes actionable in-scope findings, usually after `/review` |
| `/review-and-fix` | Autonomous review + repair loop | Runs review, fixes findings, re-reviews until clean or blocked |
| `/loop` | Looping named skills until clean | Thin orchestrator that reruns a detector/repair skill pair (e.g. `/review` `/fix`) until clean or blocked |
| `/review-orchestrate` | Broad, multi-agent review | Runs specialized reviewer roles in parallel, merges into one canonical report |
| `/review-architecture` | Architecture-specific review pass | Layering, crate boundaries, single-source-of-truth, registry-driven behavior |
| `/review-code-smells` | Maintainability-specific review pass | Duplication, awkward indirection, dead code, naming, Boy Scout opportunities |
| `/review-docs-claims` | Docs-truth review pass | User-facing docs/CLI text for truthfulness, prose quality, RFC leakage |
| `/review-incan-source-quality` | Incan-source readability review pass | Checks `.incn` code reads like well-written Python, not Rust-shaped scaffolding |
| `/review-rust-prose` | Rust comment/rustdoc review pass | Prose-comment quality, rustdoc accuracy, manual line-wrap smells |
| `/review-scope` | Scope-completeness review pass | Checks the change actually matches the intended issue/RFC/branch scope |
| `/review-test-style` | Test-style review pass | Result-returning patterns, panic helpers, unwrap/expect use, obvious coverage gaps |
| `/flag-compiler-bug` | Suspecting a compiler defect mid-task | Minimizes the repro, checks for duplicates, files via `/create-github-issue` |

**RFC lifecycle**

| Skill | Trigger | What it does |
| --- | --- | --- |
| `/write-rfc` | Drafting a new RFC | Scaffolds an RFC with correct structure and conventions |
| `/review-rfc` | Checking an RFC before submission | Validates formatting, structure, content, and status-specific rules |
| `/bump-rfc` | Promoting an RFC status | Handles Draft -> Planned -> In Progress -> Implemented transitions |

**Multi-agent orchestration**

| Skill | Trigger | What it does |
| --- | --- | --- |
| `/orchestrate-parallel-work` | Delegating to parallel sub-agents | Splits a task into non-overlapping, worktree-isolated slices with orchestrator-led integration |
| `/ralph-loop` | High-thoroughness autonomous implementation | Plan/do/check/act loop across worker worktrees through to ship-ready commit/PR artifacts |

**Other**

| Skill | Trigger | What it does |
| --- | --- | --- |
| `/add-learning` | Recording a reusable insight | Appends to learnings file with correct format and topic grouping |
| `/hello-world` | Trivial example/smoke-test request | Says hello world; not part of the real workflow, used to sanity-check the skill mechanism itself |

### Agents

Subagents in `.agents/` run as isolated specialists that can be delegated to:

| Agent        | When it's used                                   | What it does                                                  |
| ------------ | ------------------------------------------------ | ------------------------------------------------------------- |
| `test-suite` | Validating changes, checking regressions, pre-PR | Analyzes diff, runs targeted tests, checks snapshots + clippy |

## Implementation Learnings

Past RFC and issue implementations produced reusable insights. These are maintained in [`.agents/learnings.md`]. **Read the relevant section before starting work on any RFC implementation or any change that touches the parser, typechecker, or lowering stages.**

- **RFC 021** — Field Metadata & Aliases: pipeline flow, typechecker-vs-lowering pitfalls, scope restrictions, reflection helpers
- **RFC 005** — Rust Interop: parser warnings, LSP/CLI wiring, `Program` struct stability
- **RFC 022** — Stdlib Namespacing & Soft Keywords: lexer-vs-parser responsibilities, per-file activation, registry-driven validation
- **Issue #116** — Parenthesized Multi-line Imports: lexer bracket tracking, shared parsing helpers, formatter idempotency
- **RFC 023** — Frontend Bounds & Extern Diagnostics: generic bounds in symbols, call-site checking, namespace-driven stdlib activation

## Release Notes Style

When updating `workspaces/docs-site/docs/release_notes/*.md`:

- Use **structured sections**: "Features and Enhancements" vs "Bugfixes"
- Use **area prefixes** for scannability: `Models:`, `Compiler:`, `Tooling:`, `Docs:`
- Link to **PRs and issues**: `(#123, #456)` for traceability
- Keep entries **concise** (one-liner + context link)
- For **patch releases**: list all fixes; for **minor releases**: curate user-facing themes

Example: `- **Models**: Field aliases and metadata for schema-safe mapping ([RFC 021], #98)`

## PR Checklist

- [ ] PR description follows `.github/pull_request_template.md`
- [ ] `cargo test` passes
- [ ] `cargo clippy` clean
- [ ] Snapshots updated if codegen changed
- [ ] New tests added for new functionality
- [ ] Docs updated (rustdoc + docs-site if user-facing)
- [ ] AGENTS.md updated with learnings (if applicable)
- [ ] Release notes updated if user-facing change
