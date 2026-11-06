# Projects in dev.8

An Incan project combines authored source and a `loaf.toml` identity. Oven discovers the source's project, resolves active local dependencies and features, and describes one compilation unit. The compiler receives that description rather than independently interpreting the project manifest.

## One native compilation path

The checker owns semantic facts and Body IR. Incan lowering reads those facts and constructs a native plan; the driver adapts the plan to the pinned Rust compiler's MIR. Normal builds produce machine code without generating a Rust project.

Standard packages follow the same library boundary as other Loaves. They publish compiled native artifacts and checked sidecars. Imports retain their `std.*` spelling, while the toolchain supplies the packages for the selected profile.

## Preparation and reuse

Oven prepares path libraries before their consumers. Unchanged executables can be retained on disk. Resident project owners retain preparation between run, build and test requests. Input changes invalidate reuse; test results themselves are never cached.

`OVEN_TRACE=-` exposes `reused` or `compiled (...)` decisions. Disabling owners with `OVEN_OWNER=off` does not disable executable reuse.

## Project-owned and installed artifacts

Executables belong under the project's `target/incan/<profile>/bin/`. Active path dependency artifacts belong under the root consumer's `target/incan/loaves/<package>/<profile>/`. Standard-package artifacts belong in the selected toolchain home.

This separates source ownership from reusable native outputs. Editing an artifact is not an edit to its source. Source and manifests remain the inputs to subsequent compilation.

For a task-oriented walkthrough, see [Getting started](../tutorials/getting_started.md). For accepted manifest fields and CLI behavior, see [Project configuration](../reference/project_configuration.md) and [CLI reference](../reference/cli_reference.md).
