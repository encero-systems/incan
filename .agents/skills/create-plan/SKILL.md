---
name: create-plan
description: Drafts implementation plans with TDD, documentation updates, and repository verification commands before coding. Use when the user asks for an implementation plan, /create-plan, or structured pre-implementation design for work in encero workspaces (e.g. Incan, InQL).
---

# Create implementation plan

## When to use

Apply when scoping **implementation** (not RFC-only drafting): bugs, features, compiler/library behavior, tests, or user-facing docs tied to that work.

**Identify the repository root** from the user’s path or context (`incan/`, `InQL/`, etc.). If unclear, ask. Follow that repo’s **`AGENTS.md`** and **`CONTRIBUTING.md`** (repository root) for authoritative commands and boundaries.

## Plan output shape

Produce a **single markdown plan** (paste into Plan mode or a `.plan.md` file):

- **Goal** or **root cause** (1–3 sentences).
- **TDD** (red → green → refine) when behavior is testable.
- **Concrete file paths** as markdown links to real paths in that repo.
- **Documentation** subsection when the change is user-visible (see below).
- **Gate** subsection with **exact commands** from this skill’s [Verification](#verification) section, adapted to the repo.
- **Success criteria** checklist.
- Optional **mermaid** only when a small diagram clarifies a pipeline or data flow.

Do **not** edit the plan file after the user asks to **execute** the plan unless they explicitly request plan updates.

## TDD (default when tests exist)

1. **Red**: Add a failing test that encodes the contract **before** production changes.
   - Prefer the **narrowest** command + filter the repo already uses (`cargo test <filter>`, `make test`, `incan test`, etc.).
   - Use a **behavioral assertion** when a golden/snapshot alone would not fail first (e.g. substring check on generated output).
2. **Green**: Minimal change in the correct layer (parser vs typecheck vs lower vs emit vs library `.incn`—per repo docs).
3. **Refine**: Update snapshots/goldens with the repo’s documented env flags or workflows (`INSTA_UPDATE`, etc.).

**Pitfall**: Typecheck-only green is not enough for codegen pipelines; plan tests that exercise **lowering/emission** or end-to-end output when relevant.

## Documentation

When users or release notes should see the change:

- **Release notes**: add a bullet in the repo’s current release notes file (path differs by project; find it under `docs/release_notes/` or `workspaces/docs-site/docs/release_notes/` or as documented in `CONTRIBUTING.md`). Match existing style (area prefix, one line, link `#issue`).

- **Tutorials / reference**: smallest update under that repo’s `docs/` tree; for MkDocs sites, run **`mkdocs build --strict`** from the configured docs root when prose or nav changes.

If there is **no** user-visible delta, state **`docs: none`** in the plan.

## Verification

Every plan must end with a **Gate** table. Pick commands from the target repo; do not invent targets.

### Incan (`incan/`)

From the Incan repo root:

| Step | Command | Notes |
|------|---------|--------|
| Format | `make fmt` | Writes sources; `make fmt-check` for read-only. Nightly rustfmt required (see Makefile). |
| Full test + clippy | `make pre-commit-full` | fmt-check, full `cargo test` (via `TEST_CMD`), clippy `-D warnings`. |
| Smoke | `make smoke-test` | Runs tests again plus `smoke-test-core` (release build, canaries, example builds, scripts). |

**Typical one-liner after implementation:** `make fmt && make pre-commit-full && make smoke-test`.

Optional: `make verify` = `pre-commit-full` + `smoke-test-fast` when the full test suite was already run repeatedly.

**Project rules:** no `.unwrap()` / `.expect()` in Incan (see `AGENTS.md`).

### InQL (`InQL/`)

From the InQL package root:

| Step | Command |
|------|---------|
| CI-equivalent gate | `make ci` (or `make fmt-check`, `make build`, `make test` as listed in `AGENTS.md`) |

Release notes and RFC alignment follow `InQL/AGENTS.md` and `CONTRIBUTING.md`.

### Other repos

Mirror whatever **AGENTS.md** / **CONTRIBUTING.md** / **Makefile** list as the maintainer’s gate; copy command names literally into the plan.

## Template

Skeleton: [reference.md](reference.md).
