# Project lifecycle reference

## Project root

A source unit's project root is the nearest directory at or above the source's parent containing `loaf.toml`. Nested projects select their nearest manifest. Without a manifest, the source is a standalone unit.

## Source layout

`[build] source-root` selects a project-relative source directory. Otherwise `src/` is selected when present; otherwise the project root is selected.

## Identity and dependencies

`[project] name` and `version` are required manifest strings. Local dependencies name other project roots through `path`. Their aliases define public import namespaces.

## Commands

`run`, `check`, `test` and `describe-unit` require explicit source paths. `build` accepts a source or project directory. `fmt` accepts one or more files or trees.

Dev.8 provides no scaffolding, version-bump, named-environment or workspace-selection command. The accepted manifest tables and CLI options are specified in [Project configuration](../../tooling/reference/project_configuration.md) and [CLI reference](../../tooling/reference/cli_reference.md).
