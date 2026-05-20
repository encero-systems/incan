# Generated Rust Quality

Generated Rust is a first-class Incan output. It is not merely a compiler scratch artifact, even when a future Rust-hosted caller layer gives Rust applications a more curated API. A contributor should be able to inspect emitted Rust, understand the important shape, diagnose backend mistakes, and reason about performance without reverse-engineering a pile of accidental scaffolding.

This page defines the contributor quality contract for generated Rust. It complements [Duckborrowing](duckborrowing.md), which owns backend ownership planning, and [Readable, maintainable Rust](readable-maintainable-rust.md), which covers hand-written Rust in the repository.

## Product Surface

Generated Rust has more than one audience:

- **Rust compiler and Cargo**: emitted projects must build deterministically with the selected dependencies, edition, features, and profile.
- **Incan contributors**: snapshots and `--emit-rust` output must make backend changes reviewable.
- **Rust hosts**: package-facing generated crates should be usable through normal Cargo mechanics, even when not every generated symbol is a stable host API.
- **Future caller tooling**: RFC 097-style caller adapters should build on good generated Rust instead of hiding poor output behind wrappers.

Not every generated helper is a stable public API. Internal names may be compiler-owned and may change. That does not lower the bar for readability, debuggability, or performance: internal output still needs to be coherent enough to review and debug.

## Surface Classes

Classify each generated Rust finding by surface before deciding severity:

- **Public/package-facing output**: generated library crate exports, public models, classes, enums, newtypes, traits, functions, constants, module paths, Cargo metadata, and `.incnlib` metadata that downstream projects consume.
- **Debuggable artifact output**: `incan --emit-rust`, generated projects under `target/incan/`, codegen snapshots, and generated test harnesses that contributors inspect while debugging.
- **Compiler-private output**: helper bindings, private modules, desugarer plumbing, test harness glue, temporary locals, and implementation details that are not promised as stable Rust APIs.

Public/package-facing findings usually need the strictest treatment. Debuggable artifacts need enough structure to review and diagnose. Compiler-private output can be more mechanical, but it still should not hide repeated performance costs or block source-level diagnosis.

## Quality Contract

Generated Rust should satisfy these expectations unless a feature-specific design explicitly says otherwise.

### Correct Rust

- Generated code must compile without requiring users to write Rust escape hatches for ordinary Incan code.
- The Rust shape must preserve checked Incan semantics, including ownership, mutability, type conversions, visibility, imports, and error propagation.
- Backend fixes should prefer semantic or IR-level policy over local emitter patches when the behavior is not truly local.
- Generated Cargo manifests should be structured, deterministic, and reviewable.

### Readable Rust

- Generated names should be stable, legal Rust identifiers and should preserve the source concept where that is useful for debugging.
- Compiler-private helpers should use clear, recognizable prefixes such as `__incan_*`.
- Module layout should follow source modules and generated package boundaries rather than flattening unrelated concepts into one file.
- Repeated generated patterns should eventually move into runtime/helper APIs when that makes the emitted code smaller and clearer.
- Formatting should remain Rust-reader-friendly through `prettyplease` or another structured Rust formatter, not ad hoc string assembly.

### Performant Rust

- Avoidable `.clone()`, `.to_string()`, `.collect()`, heap allocation, dynamic dispatch, and eager intermediate containers are quality issues, not just style issues.
- Clones and string materialization must be explainable from Incan semantics, a checked target type, a `ValueUseSite`, or an intentional runtime boundary.
- Last-use moves should win over defensive cloning when the IR proves the value can be consumed.
- Lookups, membership tests, iterator adapters, and stdlib helpers should prefer borrowed or lazy shapes when they do not change source semantics.
- Broad emitter rewrites are acceptable when an audit finds a repeated quality or performance problem, but the change needs explicit scope, tests, and a performance argument.
- Hot generated paths should get benchmark or profiling coverage when the performance claim is not obvious from the emitted shape.

### Hostable Rust

- Package-facing generated crates should expose coherent Rust items for public Incan declarations where the current package model does so.
- Ordinary Rust crates should be able to depend on generated library artifacts through Cargo path dependencies and call currently exported public generated Rust items directly.
- Rust-hosted caller APIs may provide a curated stable surface later, but they should not be treated as permission for low-quality generated implementation code.
- Public/package-facing output should avoid leaking compiler-internal layout details when a stable helper or adapter can carry the boundary more clearly.
- Generated rustdoc should be considered for declarations that are intended to be inspected from Rust.

### Debuggable Rust

- A contributor should be able to run `incan --emit-rust path/to/file.incn` and connect the result back to the source feature.
- Snapshot names should identify the language feature, regression, stdlib module, or interop boundary being protected.
- Diagnostics should report Incan source concepts whenever possible; generated Rust names are fallback evidence, not the primary user-facing explanation.
- When generated Rust must contain unusual scaffolding, tests or docs should make the reason discoverable.

### Testable Rust

- Codegen snapshots are the default review gate for emitted shape changes.
- Build or run tests are required when Rust validity, borrow checking, dependency wiring, async runtime setup, or generated Cargo behavior is the risk.
- Ownership changes should include planner/lowering coverage where the decision is made and generated-Rust coverage where the shape is observed.
- Public/package-facing generated output should have representative fixtures, not only minimized internal snippets.
- Native Rust consumer coverage should exercise Cargo against the generated library crate itself rather than only inspecting generated text.

## Audit Workflow

Use this loop when changing generated Rust:

1. Identify the source-level semantic change or quality problem.
2. Classify the affected surface as public/package-facing, debuggable artifact, or compiler-private output.
3. Decide whether the fix belongs in typechecking, lowering, ownership planning, emission, runtime helpers, or project generation.
4. Inspect current output with `incan --emit-rust` or the relevant generated project under `target/incan/`.
5. Label each clone, allocation, eager collection, or helper call as required by semantics, required by current API shape, suspicious, or intentionally optimized.
6. Add or update a focused codegen snapshot when the emitted shape should be reviewable.
7. Add a build/run/integration test when Rust compilation behavior matters.
8. For performance-sensitive changes, explain why the new shape avoids work or add a benchmark/profiling follow-up.
9. Run the narrow test first, then the relevant repository gate before closeout.

Useful commands:

```bash
incan --emit-rust path/to/file.incn
cargo test --test codegen_snapshot_tests
INSTA_UPDATE=1 cargo test --test codegen_snapshot_tests
make pre-commit
```

## When To Broaden Scope

Do not keep generated Rust cleanup arbitrarily small when the evidence shows a broader design problem. Broaden the work, or file a clearly scoped follow-up, when any of these are true:

- The same awkward pattern appears across multiple emitters or snapshots.
- A local fix would bypass duckborrowing, typechecker facts, or IR ownership policy.
- Generated public/package-facing Rust would become harder for Rust hosts to call or inspect.
- The emitted shape adds avoidable work to a likely-hot path.
- The issue blocks RFC 097-style Rust-hosted caller work or future generated crate stability.

Broadening still needs discipline: state the affected compiler layers, list the generated surfaces that should change, add representative tests, and make the performance story explicit.

## v0.3 Baseline

For v0.3, the baseline is:

- Generated Rust quality is documented as an explicit contributor contract.
- Representative generated Rust snapshots cover ordinary language features, stdlib compiled modules, Rust interop, ownership-sensitive regressions, and generated package/project behavior.
- A native Rust consumer characterization covers direct Cargo use of generated library output for currently supported public models and functions.
- Performance review is part of generated Rust review, especially around clone/allocation-heavy output.
- Larger helper APIs, inspection tooling, or caller-boundary architecture are tracked as follow-up work rather than silently hidden inside one emitter patch.

Current audit gaps to track from the v0.3 baseline:

- Direct `IrCodegen` snapshots are broad, but final package-facing artifact sets are thinner: `Cargo.toml`, nested `src/**`, `.incnlib`, Rust ABI metadata, and consumer `pub::` projects need a small golden baseline.
- Real stdlib generated output is covered unevenly. Important public modules should be classified as snapshot, compile-only, import-user-facing, or missing.
- Rust interop coverage is strong in direct snapshots, but weaker across generated library artifacts and downstream consumers.
- Rust-hosted callability coverage is currently limited to generated package artifacts and direct native Rust consumption of supported public items; RFC 097 caller adapters remain follow-up work.
- Iterator and comprehension output currently contain known clone/allocation-heavy shapes. Some are required by current semantics or API shape, but borrowed iterator and callback designs should be tracked as performance work.

For v0.4, the natural next step is tooling maturity: generated Rust inspection commands, quality gates, or snapshot grouping that make drift easier to review.

For v0.5 and later, the focus shifts toward architectural stability: stable helper crates, caller adapters, generated crate contracts, and compatibility policy on the path to 1.0.
