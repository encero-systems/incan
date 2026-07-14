---
name: create-pr-description
description: Generate a PR description following the repository's pull request template. Use when the user asks to create, draft, or generate a PR description for a pull request. The skill automatically locates the PR template in the target repository and fills it in based on the git diff.
---

# PR Description Generator

## Workflow

1. **Identify the repository and branch** - Determine which repository and branch the PR is for. If not specified, ask the user. The workspace may contain multiple repositories (e.g., `incan/`, `IncQL/`).
2. **Locate the PR template** - Find `.github/pull_request_template.md` in the repository root. If not found, use the standard template from the repository's `.github/` folder.
3. **Gather context** - Run `git diff main...HEAD` (or the appropriate base branch) to see all changes. Also check `git log main..HEAD --oneline` for commit messages.
4. **Check for associated issue** - Extract issue number from branch name (e.g., `bugfix/184-...` → issue #184) or check commit messages for `Fixes #NNN`, `Closes #NNN`, etc.
5. **RFC lifecycle preflight** - If the branch implements an RFC or touches `workspaces/docs-site/docs/RFCs/*`, inspect the governing RFC before drafting the PR body.
   - If the RFC is `In Progress` and the branch implements the full RFC scope, use `bump-rfc` first so the RFC is `Implemented`, moved into `closed/implemented/`, and reflected in the PR diff before the PR is presented for review.
   - If the branch implements only part of the RFC, keep the RFC active and state the remaining scope clearly in the PR body.
   - Do not draft a review-ready PR description that claims full RFC completion while the RFC file still says `In Progress`.
6. **Analyze the changes** - Categorize the changes by:
   - Type (bug fix, new feature, refactor, docs, CI/tooling, RFC)
   - Area(s) affected (Incan Language, Compiler, Tooling, Editor, Runtime, Docs)
   - User-facing behavior changes
   - Internal architectural changes
   - Risks or breaking changes
7. **Fill in the template** - Use the analysis to populate each section of the PR template.
8. **Add issue reference** - If an issue number was found, append `Closes #<issue number>` at the end of the PR description.
9. **Output the PR description** - Return the complete PR description in markdown format, ready to be used in a pull request.

## PR Template Location

The skill automatically locates the PR template at:
- `.github/pull_request_template.md` in the repository root

If the template is not found, the skill uses the standard template structure.

## Template Sections

### Summary
- What does this PR change, and why?
- One tight paragraph summarizing the change.

### Type of change
- Bug fix, New feature, Refactor/maintenance, Documentation, CI/tooling, or RFC

### Area(s)
- Incan Language (syntax/semantics)
- Compiler (frontend/backend/codegen)
- Tooling (CLI/formatter/test runner)
- Editor integration (LSP/VS Code extension)
- Runtime / Core crates (stdlib/core/derive)
- Documentation

### Key details
- **User-facing behavior**: what changes for users?
- **Internals**: what changed architecturally?
- **Risks**: what could break?

### Testing / verification
- [ ] `make test` / `cargo test`
- [ ] `make examples` (if relevant)
- [ ] `incan fmt --check .` (if relevant)
- Manual verification notes

### Docs impact
- [ ] No docs changes needed
- [ ] Docs updated
- [ ] Docs follow Divio intent (tutorial/how-to/reference/explanation)

## Output Format

Return the PR description in markdown format, ready to be used in a pull request. Include all sections of the template, with checkboxes for items that need user verification. If an issue number was found, append `Closes #<issue number>` at the end of the PR description.

For RFC-driven PRs, include the final RFC lifecycle state in the verification or docs section. If full RFC scope is implemented, the PR description should reference the RFC under `workspaces/docs-site/docs/RFCs/closed/implemented/`; if it is not, describe the remaining RFC checklist items instead of using a closing keyword that would close the issue.

## Examples

### Example 1: Bug Fix in incan

**User prompt:** "Create a PR description for bugfix/184 in the incan repo"

**Output:**
```markdown
## Summary

This PR fixes RFC 042 trait supertrait assignability and generic upcast compatibility checks in the typechecker, and improves project root resolution logic for multi-file Incan projects.

**Bug fixes:**
- Trait-typed values are now correctly assignable to supertrait return types (e.g., `def to_root(x: Mid) -> Root: return x` where `Mid with Root`)
- Generic trait upcasts work correctly with compatible type arguments (e.g., `BoundedDataSet[T]` → `DataSet[T]`)
- Transitive supertrait generics substitute properly through the hierarchy
- Concrete adopters satisfy transitive supertrait annotations

**Infrastructure improvements:**
- Enhanced project root resolution in `build.rs` and `common.rs` to handle cases where the manifest is not found, defaulting to inferred project root
- Added logic to resolve imports from the project source root when in non-source directories (e.g., `tests/`, `examples/`)
- Improved source root detection with manifest-based configuration support

## Type of change

- [x] Bug fix
- [ ] New feature
- [ ] Refactor / maintenance
- [ ] Documentation
- [ ] CI / tooling
- [ ] RFC (adds/updates `docs/RFCs/*`)

## Area(s)

- [x] Incan Language (syntax/semantics)
- [x] Compiler (frontend/backend/codegen)
- [ ] Tooling (CLI/formatter/test runner)
- [ ] Editor integration (LSP/VS Code extension)
- [ ] Runtime / Core crates (stdlib/core/derive)
- [ ] Documentation

## Key details

- **User-facing behavior**: Trait-typed values can now be returned as supertrait types without explicit casting. Generic trait hierarchies with compatible type arguments are correctly recognized as assignable.
- **Internals**: Added transitive supertrait closure computation, updated type compatibility checks for trait-to-trait upcasts (including generic arguments), and improved module resolution to fallback to project source root for imports in non-source directories.
- **Risks**: The typechecker changes affect trait assignability checks; existing code that relied on incorrect rejection of valid upcasts may now compile. The module resolution changes improve import handling but could affect edge cases with unusual project structures.

## Testing / verification

- [x] `make test` / `cargo test`
- [x] `make examples` (if relevant)
- [x] `incan fmt --check .` (if relevant)
- [ ] Manual verification described below

**Tests added:**
- `test_trait_supertrait_assignability_codegen` - validates RFC 042 upcast codegen
- `test_supertrait_cycle_is_diagnosed` - validates cycle detection
- `test_supertrait_transitive_closure` - validates transitive supertrait resolution
- `test_types_compatible_trait_to_supertrait_named` - validates named trait upcasts
- `test_types_compatible_trait_to_supertrait_generic` - validates generic trait upcasts
- `test_types_compatible_trait_to_supertrait_transitive_generic` - validates transitive generic upcasts
- `test_check_with_imports_concrete_and_supertrait_upcasts` - validates cross-module upcasts

## Docs impact

- [x] No docs changes needed
- [ ] Docs updated
- [ ] Docs follow Divio intent (tutorial/how-to/reference/explanation) where applicable

The RFC 042 specification is the normative source for trait supertrait assignability semantics.

## Checklist

- [ ] I kept public docs user-focused and moved internals to contributing docs when appropriate
- [ ] I avoided duplicating canonical install/run instructions in multiple places
- [ ] I added/updated tests where it materially reduces regressions

Closes #184
```

### Example 2: New Feature in incan (without issue)

**User prompt:** "Create a PR description for feature/180 in the incan repo"

**Output:**
```markdown
## Summary

This PR implements RFC 035: First-class named function references in Incan. Named `def` functions can now be passed as values, stored in variables, and placed in collections without wrapping in a closure. `Callable[Params, R]` is accepted as syntactic sugar for `(Params) -> R` and desugared at parse time.

## Type of change

- [ ] Bug fix
- [x] New feature
- [ ] Refactor / maintenance
- [ ] Documentation
- [ ] CI / tooling
- [ ] RFC (adds/updates `docs/RFCs/*`)

## Area(s)

- [x] Incan Language (syntax/semantics)
- [x] Compiler (frontend/backend/codegen)
- [ ] Tooling (CLI/formatter/test runner)
- [ ] Editor integration (LSP/VS Code extension)
- [ ] Runtime / Core crates (stdlib/core/derive)
- [ ] Documentation

## Key details

- **User-facing behavior**: Named functions can now be used as first-class values. The `Callable[Params, R]` syntax provides a convenient way to express function types.
- **Internals**: Parser now recognizes `Callable` as a type sugar and desugars it to the canonical function type. Typechecker validates function references and their signatures. Lowering emits Rust closures for function references.
- **Risks**: This is a new language feature; existing code is not affected. The `Callable` sugar may conflict with future type aliases named `Callable`.

## Testing / verification

- [ ] `make test` / `cargo test`
- [ ] `make examples` (if relevant)
- [ ] `incan fmt --check .` (if relevant)
- [ ] Manual verification described below

**Tests added:**
- Typechecker tests for function reference type checking
- Codegen snapshot tests for function reference emission
- Integration tests for function reference usage

## Docs impact

- [ ] No docs changes needed
- [x] Docs updated
- [ ] Docs follow Divio intent (tutorial/how-to/reference/explanation) where applicable

**Docs updated:**
- Language reference for function references
- Tutorial for first-class functions
- RFC 035 implementation notes

## Checklist

- [ ] I kept public docs user-focused and moved internals to contributing docs when appropriate
- [ ] I avoided duplicating canonical install/run instructions in multiple places
- [ ] I added/updated tests where it materially reduces regressions
Closes #2```

### Example 3: PR in IncQL repo

**User prompt:** "Create a PR description for feature/new-dataset-api in the IncQL repo"

**Output:**
```markdown
## Summary

This PR adds a new dataset API for IncQL that provides a unified interface for both bounded and unbounded data streams. The API is built around the `DataSet[T]` trait hierarchy with `BoundedDataSet[T]` and `UnboundedDataSet[T]` intermediate traits.

## Type of change

- [ ] Bug fix
- [x] New feature
- [ ] Refactor / maintenance
- [ ] Documentation
- [ ] CI / tooling
- [ ] RFC (adds/updates `docs/RFCs/*`)

## Area(s)

- [x] Incan Language (syntax/semantics)
- [ ] Compiler (frontend/backend/codegen)
- [x] Tooling (CLI/formatter/test runner)
- [ ] Editor integration (LSP/VS Code extension)
- [ ] Runtime / Core crates (stdlib/core/derive)
- [ ] Documentation

## Key details

- **User-facing behavior**: Authors can now write pipelines that work with both batch and streaming data using the same API. The type system enforces streaming constraints at compile time.
- **Internals**: Added `DataSet[T]`, `BoundedDataSet[T]`, `UnboundedDataSet[T]` traits and `DataFrame[T]`, `LazyFrame[T]`, `DataStream[T]` concrete types to the IncQL library package.
- **Risks**: This is a new API; existing code is not affected. The trait hierarchy may need adjustment based on user feedback.

## Testing / verification

- [ ] `make test` / `cargo test`
- [ ] `make examples` (if relevant)
- [ ] `incan fmt --check .` (if relevant)
- [ ] Manual verification described below

**Tests added:**
- Trait hierarchy tests
- Type assignability tests
- Operation API tests

## Docs impact

- [ ] No docs changes needed
- [x] Docs updated
- [x] Docs follow Divio intent (tutorial/how-to/reference/explanation) where applicable

**Docs updated:**
- `docs/language/reference/dataset_types.md` - Reference documentation
- `docs/language/explanation/dataset_types.md` - Explanatory documentation
- `docs/rfcs/001_incql_dataset.md` - RFC 001

## Checklist

- [ ] I kept public docs user-focused and moved internals to contributing docs when appropriate
- [ ] I avoided duplicating canonical install/run instructions in multiple places
- [ ] I added/updated tests where it materially reduces regressions
```
