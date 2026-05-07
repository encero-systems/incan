# Scope — docs-deps

Status: planned

## End State

The docs site dependency pins remediate issue #475 docs alerts by using:

- `pymdown-extensions` at least `10.16.1`
- `Pygments` at least `2.20.0`

The docs site still renders under strict MkDocs validation.

## In Scope

- Update `workspaces/docs-site/requirements-docs.txt`.
- Preserve the existing Pygments compatibility note if it remains useful.
- Run docs dependency installation/update as needed.
- Run `mkdocs build --strict` from `workspaces/docs-site`.
- Maintain Ralph loop state under `.agents/state/review-report.md` and `.agents/state/ralph-loop/docs-deps/`.

## Non-Goals

- No release notes changes.
- No docs prose rewrites.
- No navigation or theme changes unless compatibility requires it.
- No commit, push, or pull request creation.
