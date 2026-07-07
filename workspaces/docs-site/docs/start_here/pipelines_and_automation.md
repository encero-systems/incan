# Pipelines & automation

This page is a routing guide for step/pipeline-style workflows (automation, reproducibility, CI-friendly runs).

## What you should do next

- Quickstart: [Getting Started](../tooling/tutorials/getting_started.md) (toolchain install and starter project)
- Pipeline tutorial: [Pipeline mini-project](../tooling/tutorials/pipeline_mini_project.md)
- CI entrypoint (projects): [CI & automation](../tooling/how-to/ci_and_automation.md)
- CI entrypoint (contributors): [CI & automation (repository)](../contributing/how-to/ci_and_automation.md)
- Tooling workflow:
    - [Testing](../tooling/how-to/testing.md)
    - [Formatting](../tooling/how-to/formatting.md)
    - [LSP](../tooling/how-to/lsp.md)
- Language foundations for “structured automation”:
    - [Error Handling](../language/explanation/error_handling.md)
    - [Imports & Modules](../language/explanation/imports_and_modules.md)
    - [File I/O](../language/how-to/file_io.md)

## What to look for

- Deterministic commands and outputs (exit codes, reproducible runs)
- Honest “what exists today” vs “future” labeling (no implied platform integrations)

## Works today (use this for pipelines)

- `incan fmt --check` for CI formatting enforcement
- `incan test` with basic filtering (`-k`) and CI-friendly exit codes
- `incan run` for deterministic single-program runs (exit code follows the program)

If you are contributing to the Incan repository itself, the CI entrypoints are `make check` / `make test` / `make smoke-test` (see: [CI & automation (repository)](../contributing/how-to/ci_and_automation.md)).

## Not yet / planned (don’t rely on it for automation)

- Package registry publishing and remote project templates
- Native Windows installer support
- Enterprise deployment policy beyond locked/offline command flags
