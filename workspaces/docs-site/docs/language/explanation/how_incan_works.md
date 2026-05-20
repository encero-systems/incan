# How Incan works

This page is a user-level mental model of what happens when you run Incan.

## The short version

Incan compiles `.incn` source code to Rust, then builds an executable.

## Pipeline (conceptual)

```mermaid
--8<-- "_snippets/diagrams/compiler_pipeline.mmd"
```

## What this means in practice

- Incan programs are compiled (not interpreted).
- The “happy path” is: edit code → run `incan` → get an executable or an error.
- When things fail, you typically care about which stage failed (parse, typecheck, codegen, build).

## Where to go next

- Build/run commands: [Install, build, and run](../../tooling/how-to/install_and_run.md)
- CLI surface: [CLI reference](../../tooling/reference/cli_reference.md)
- Deeper internals (contributors): [Compiler architecture](../../contributing/explanation/architecture.md)
