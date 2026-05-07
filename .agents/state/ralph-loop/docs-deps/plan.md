# Plan — docs-deps

Status: planned

1. Confirm the current docs dependency pins and tracked docs-site artifacts.
2. Update only the vulnerable docs dependency pins in `workspaces/docs-site/requirements-docs.txt`.
3. Install or update docs requirements from the docs-site workspace.
4. Run `mkdocs build --strict` from `workspaces/docs-site`.
5. Review the diff for scope, generated artifact drift, and compatibility fallout.
6. Fix any verification or review findings, then hand off with commands and final status.
