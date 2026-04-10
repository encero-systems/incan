# Agent Instructions for Incan Development

Incan is a Python-like language that compiles to Rust. The compiler itself is written in Rust and generates native Rust code via an IR-based pipeline. This document contains guidance for AI agents working on the codebase.

> **CRITICAL — NO `.unwrap()` / `.expect()` ANYWHERE.** This is the single most important rule.
> Multiple modules enforce `#![deny(clippy::unwrap_used)]` and `#![deny(clippy::expect_used)]`.
> This applies to **all** code — production, tests, examples. No exceptions, no shortcuts.
> Use `?` with `Result`-returning test functions, or propagate errors explicitly.
> See [Error handling in tests](#error-handling-in-tests) for the correct pattern.

> **CRITICAL — THE USER DECIDES WHAT IS RELEVANT.** Scope, PR boundaries, and which files “belong” on a branch are **the maintainer’s call**, not the agent’s. Never label work as “unrelated PR noise,” “cleanup,” or “hygiene” as a reason to remove or revert it. Always check with the user when in doubt.
>
> **FORBIDDEN without explicit user approval that quotes the exact paths or commands:** anything that overwrites or deletes uncommitted work — including `git checkout -- <path>`, `git restore <path>`, `git clean`, `git reset --hard`, `stash drop`, or equivalent. If you believe files should be split, reverted, or left out of a PR, **state that and ask**; do not run destructive git operations on your own initiative.
>
> **Commits and pushes:** The maintainer commits code unless they **explicitly** ask you to run `git commit` or `git push`. Implement and test in the working tree; offer a suggested commit message as text. The `/start-work` skill states the same rule.

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

Skills, learnings, and agent notes live under **this repository’s** `.agents/` directory (committed here).

## General Workflow

1. **Branch from main**: Create a feature branch using the naming convention `<type>/<issue>-<slug>`, where type is `feature`, `chore`, or `bugfix`. Examples: `feature/165-implement-rfc-031-library-system-phase-1`, `chore/88-vocab-drift-guardrails`, `bugfix/42-fix-parser-crash`. Use the `/start-work` skill to automate this.
2. **Follow RFCs**: RFCs in `workspaces/docs-site/docs/RFCs/` are the spec — implement exactly what they say.
3. **Run tests**: `make test` must pass before considering work complete. Run targeted tests during development; run the full suite when you finish.
4. **Update snapshots**: `INSTA_UPDATE=1 cargo test --test codegen_snapshot_tests` to update changed snapshots.
5. **Boy Scout Rule**: Leave every file you touch in better shape than you found it — fix stale TODOs, missing doc comments, unused imports, misleading names.
6. **Documentation gate (mandatory)**: Before finalizing any change, audit every touched Rust module and ensure rustdocs are present and accurate for all new/changed public APIs and non-obvious internal helpers.

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

- Wrap rustdoc prose naturally — don't worry about line length.
- Use `make fmt` to format the codebase after making changes, and before running tests.

### Documentation requirements (mandatory)

Agents must treat documentation updates as part of implementation, not optional polish.

- **Public API docs are required**: Any new/changed `pub` module, type, enum variant intent, struct field intent (when not obvious), and function/method must have rustdoc that explains purpose and contract.
- **Error types need variant-level docs**: For `thiserror` enums and diagnostic types, document what each variant represents and when it is emitted.
- **Non-trivial functions and methods need docs**: New or changed functions/methods should carry rustdoc/doc comments unless they are genuinely tiny and self-evident local helpers. Prefer documenting all touched functions over debating edge cases.
- **Cross-stage and boundary helpers always need docs**: Parser/typechecker/lowering/emission/interop/conversion helpers must document purpose, invariants, and why the boundary exists, even when they are private.
- **Tiny obvious helpers are the only exception**: A very small private helper may skip rustdoc only when its name and body make the intent completely obvious and there are no invariants, fallback paths, ownership assumptions, or feature-gated behaviors to explain.
- **Behavioral boundaries must be explicit**: For pipeline boundaries (parser -> desugar -> typecheck -> lowering), docs should state what must and must not cross the boundary.
- **Docs should explain why, not narrate syntax**: Explain purpose, contracts, fallbacks, ownership/borrowing assumptions, and misuse risks. Avoid comments that merely restate the code line-by-line.
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

Skills are reusable workflows in `.agents/skills/`. Use them by name when the task matches:

| Skill           | Trigger                           | What it does                                                                                                                            |
| --------------- | --------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| `/start-work`   | Starting work on an issue or RFC  | Creates branch, gathers context from issue/RFC, checks learnings; does **not** commit (maintainer-only commits unless explicitly asked) |
| `/test`         | Writing tests for a change        | Guides test selection, provides correct patterns per compiler stage                                                                     |
| `/review`       | Code review, PR review            | Runs the full Incan-aware review checklist                                                                                              |
| `/write-rfc`    | Drafting a new RFC                | Scaffolds an RFC with correct structure and conventions                                                                                 |
| `/review-rfc`   | Checking an RFC before submission | Validates formatting, structure, content, and status-specific rules                                                                     |
| `/bump-rfc`     | Promoting an RFC status           | Handles Draft -> Planned -> In Progress -> Done transitions                                                                             |
| `/add-learning` | Recording a reusable insight      | Appends to learnings file with correct format and topic grouping                                                                        |

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
