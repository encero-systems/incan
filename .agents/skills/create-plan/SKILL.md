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
- **Pattern intake**: active area, 2-3 local precedent files to read, source-of-truth boundary/registry, and required verification path.
- **Capability intake** for Incan-language or stdlib work: current language features, local examples/tests proving available constructs, and any claimed limitation with evidence.
- **TDD** (red → green → refine) when behavior is testable.
- **Concrete file paths** as markdown links to real paths in that repo.
- **Documentation** subsection when the change is user-visible (see below).
- **Code docs / rustdocs** subsection when the change introduces or reshapes public/shared APIs, compiler snapshot bridges, or subtle boundary helpers.
- **Gate** subsection with **exact commands** from this skill’s [Verification](#verification) section, adapted to the repo.
- **Success criteria** checklist.
- Optional **mermaid** only when a small diagram clarifies a pipeline or data flow.
- Optional **Parallelization opportunities** subsection only when the user explicitly wants delegation and the work splits into clean, non-overlapping ownership slices. Keep it short: slice, owned scope, verification, blocker.

Do **not** edit the plan file after the user asks to **execute** the plan unless they explicitly request plan updates.

If the task is clearly parallelizable after planning, hand off execution to `orchestrate-parallel-work` rather than embedding full worker-orchestration rules in the plan itself.

## Pattern intake

Before selecting files to edit, identify the local pattern that should govern the work:

- Active area: parser, typechecker, lowering, emission, stdlib, CLI/tooling, docs, tests, or a named combination.
- Precedents: 2-3 nearby files that already implement the same behavior shape. Prefer same-stage and same-domain examples.
- Source of truth: the RFC, docs contract, diagnostics catalog, stdlib registry, ownership policy, CLI reference, or other boundary that owns the behavior.
- Verification path: the narrow red test plus any broader proof needed to cover downstream stages or build modes.

Plans should name these items explicitly. If there is no close precedent, say so and explain which boundary document or implementation shape is being used instead.

## Capability intake for Incan work

Do not plan around assumptions about what Incan cannot do. Incan is moving fast, and local repo examples/tests are more reliable than memory.

For language, stdlib, examples, or `.incn` implementation work, inspect current capability before choosing a design:

- search local `.incn` stdlib/examples/tests for the construct you think may be unavailable;
- check parser/typechecker/codegen tests for recently added syntax or lowering support;
- prefer a small source-level probe or focused fixture over guessing;
- only claim “Incan cannot express this” after recording the checked evidence and the specific failing construct;
- if a fallback to Rust interop or backend support is needed, state the primitive gap narrowly rather than treating the whole feature as impossible.

Plans for Incan work should include the checked capability evidence or explicitly say `capability evidence: not yet checked` and keep the design provisional.

## TDD (default when tests exist)

1. **Red**: Add a failing test that encodes the contract **before** production changes.
   - Prefer the **narrowest** command + filter the repo already uses (`cargo test <filter>`, `make test`, `incan test`, etc.).
   - Use a **behavioral assertion** when a golden/snapshot alone would not fail first (e.g. substring check on generated output).
2. **Green**: Minimal change in the correct layer (parser vs typecheck vs lower vs emit vs library `.incn`—per repo docs).
3. **Refine**: Update snapshots/goldens with the repo’s documented env flags or workflows (`INSTA_UPDATE`, etc.).

**Pitfall**: Typecheck-only green is not enough for codegen pipelines; plan tests that exercise **lowering/emission** or end-to-end output when relevant.

**Rust interop pitfall**: `rust-metadata` is optional in Incan. If the task touches `import rust::...`, `rusttype`, or Rust-boundary method/call lowering, the plan should explicitly cover both:
- the metadata-enhanced path (focused unit/integration coverage when that feature matters), and
- a default-build path (for example a real example/program build) so the fix does not only work with metadata enabled.

## Documentation

When users or release notes should see the change:

- **Release notes**: add a bullet in the repo’s current release notes file (path differs by project; find it under `docs/release_notes/` or `workspaces/docs-site/docs/release_notes/` or as documented in `CONTRIBUTING.md`). Match existing style (area prefix, one line, link `#issue`).

- **Tutorials / reference**: smallest update under that repo’s `docs/` tree; for MkDocs sites, run **`mkdocs build --strict`** from the configured docs root when prose or nav changes.

If there is **no** user-visible delta, state **`docs: none`** in the plan.

## Code docs / rustdocs

When the implementation adds or changes public items, shared interop vocabulary, compiler snapshot helpers, or other subtle cross-stage boundaries:

- Plan the required **rustdoc/doc comment updates** alongside the code change instead of treating them as optional cleanup.
- Be explicit about which files need API/boundary docs refreshed.
- If nothing public or boundary-shaped changed, state **`rustdocs: none`** in the plan.

## Verification

Every plan must end with a **Gate** table. Pick commands from the target repo; do not invent targets.

### Incan (`incan/`)

From the Incan repo root:

| Step | Command | Notes |
|------|---------|--------|
| Format | `make fmt` | Writes sources; `make fmt-check` for read-only. Nightly rustfmt required (see Makefile). |
| Full gate | `make pre-commit` | Full local gate: full checks + `smoke-test-fast`. |
| Smoke | `make smoke-test` | Runs tests again plus `smoke-test-core` (release build, canaries, example builds, scripts). |

**Typical one-liner after implementation:** `make fmt && make pre-commit`.

Optional: `make smoke-test` when you explicitly want the full smoke suite after `pre-commit`.

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
