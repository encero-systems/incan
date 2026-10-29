# How Incan works

Incan source compiles to native machine code through the pinned Rust compiler. Dev.8 does not generate a Rust project as its normal compilation path.

## Pipeline

```mermaid
--8<-- "_snippets/diagrams/compiler_pipeline.mmd"
```

<p class="inc-diagram-caption">Oven describes a unit; the checker owns its meaning; Incan lowering and the native adapter translate the checked result.</p>

The project manifest supplies package identity, dependencies and features. Checked Body IR records language facts once. Lowering consumes those facts, and the native plan reaches rustc through its Rust adapter.

Libraries publish checked contracts and native artifacts together. Standard packages use the same Loaf boundary. Oven can reuse unchanged executables and retain project preparation in a resident owner, while each program or test execution remains fresh.

Source diagnostics and codegraph records expose compiler facts to tools. See [CLI reference](../../tooling/reference/cli_reference.md) for commands and [Architecture](../../contributing/explanation/architecture.md) for contributor boundaries.
