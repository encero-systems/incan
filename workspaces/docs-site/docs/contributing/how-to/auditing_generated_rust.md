# Auditing generated Rust

Generated Rust is a derived artifact, but it is still a review surface for correctness, maintainability, and performance regressions. Use the generated Rust audit runner when a change affects lowering, emission, stdlib copying, generated test harnesses, or fixture packages under `target/incan/`.

The audit report is a review skeleton. It records the artifact class, path, availability, strictness status, and structured placeholders for clone, allocation, and eager-collection notes. It does not assign a subjective score.

## Run the default report

After generating representative fixtures, run:

```bash
scripts/generated_rust_audit.py
```

The default artifact list points at representative `target/incan/` fixture outputs. If those outputs have not been generated in the current worktree, the report keeps going and marks them as `missing` with `not_evaluated` strictness.

To emit JSON for automation or later aggregation:

```bash
scripts/generated_rust_audit.py --format json
```

To make missing artifacts fail a CI-style check:

```bash
scripts/generated_rust_audit.py --fail-on-missing
```

To test the audit helper itself without relying on previously generated artifacts, run:

```bash
make generated-rust-audit-gate
```

That target uses committed Rust fixtures and validates the JSON/Markdown report behavior, marker counting, missing-artifact handling, explicit artifact paths, and `--fail-on-missing`.

## Audit explicit artifacts

Pass generated Rust files or directories with `SURFACE_CLASS=PATH` specs:

```bash
scripts/generated_rust_audit.py \
  --artifact program-main=target/incan/my_fixture/src/main.rs \
  --artifact stdlib-copy=target/incan/my_fixture/src/__incan_std \
  --artifact test-harness=target/incan/tests/my_harness
```

Use surface classes that describe the generated artifact's role, not its quality. Common classes:

| Surface class   | Use for                                      |
| --------------- | -------------------------------------------- |
| `program-main`  | Generated package entry point                |
| `stdlib-copy`   | Copied or synthesized `__incan_std` modules  |
| `test-harness`  | Generated test runner or preheat harnesses   |
| `surface-fixture` | Fixture package directories for one feature |

Directories are scanned recursively for `.rs` files. Non-existent paths and directories without Rust files are reported explicitly instead of being silently skipped.

When an artifact path is inside the repository, the report renders it as a repository-relative path even if the command received an absolute path.

## Read the report

Each artifact row has these fields:

| Field               | Meaning                                                      |
| ------------------- | ------------------------------------------------------------ |
| Surface class       | Reviewer-provided classification for the artifact role       |
| Artifact path       | File or directory that was requested                         |
| Check status        | `present`, `missing`, `no-rust-files`, or `unsupported-path` |
| Strictness status   | `available_for_review` when Rust files exist; otherwise `not_evaluated` |
| Clone notes         | Marker count plus a manual-review placeholder                |
| Allocation notes    | Marker count plus a manual-review placeholder                |
| Eager collection notes | Marker count plus a manual-review placeholder             |

Marker counts are literal occurrence scans for review prompts. They are not findings by themselves. The reviewer decides whether each marker is expected, avoidable, or needs a follow-up change.

## Suggested contributor loop

1. Build or run the representative fixture that exercises the compiler surface under review.
2. Run `scripts/generated_rust_audit.py` with explicit `--artifact` entries for the generated files or directories.
3. Save Markdown or JSON output with `--output` if the review needs an artifact.
4. Fill in the clone, allocation, and eager-collection notes during manual review.
5. Attach the report or summarize its objective statuses in the implementation handoff.

## Review a backend change

Review a compiler or backend change against the [Rust-source backend deprecation](../explanation/rust_source_backend_deprecation.md) policy and the [backend behavior inventory](../reference/backend_behavior_inventory.md) with these questions:

1. Is this behavior documented, tested, and intentionally supported?
2. Does the patch change source behavior or only generated artifact shape?
3. If it changes source behavior, is the behavior recorded before backend emission?
4. If it changes generated Rust, is the generated Rust a consumer of semantic facts or the source of the decision? Does the change add semantic authority to old Rust-source lowering/emission?
5. Which evidence lane proves it outside the local file path? Does the test cover the boundary that can observe the behavior: direct, import, facade/reexport, package consumer, test batch, vocab, generated project, or downstream lane?
6. If it is a compatibility fix or touches old backend compatibility, where is the [migration note](../reference/rust_source_backend_deprecation.md#migration-note), and does it state a retirement condition?
7. Which stable ID, semantic fact, HIR, Body IR, ABI metadata, runtime-service fact, or diagnostic should own the behavior next?
8. Is there a concrete existing anchor in the inventory's [repo anchors](../reference/backend_behavior_inventory.md#representative-repo-anchors), or does the change need a new focused fixture?
9. Does it preserve the current Rust-source backend without making replacement harder?

Use more than one [evidence lane](../reference/backend_behavior_inventory.md#evidence-lanes) when a behavior can cross boundaries.

The inventory's repo anchors are starting points for reviewers. They are not exhaustive, and new backend work should add tighter tests when a behavior lacks a direct anchor.

Before adding a new broad regression lane, check whether the repository already has a compact [guardrail](../reference/rust_source_backend_deprecation.md#guardrails) for the boundary.

## Update the stdlib coverage inventory

The [generated Rust stdlib coverage inventory](../reference/generated_rust_stdlib_coverage.md) has one row per `std.*` module. A change that alters a module's coverage updates its row, and a new stdlib module gets a row in its component's section.

1. List the stdlib modules (every `.incn` file under a component's `src/`, apart from its `lib.incn` provider entrypoint):

    ```sh
    rg --files loaves/stdlib | rg '/src/.*\.incn$' | rg -v '/lib\.incn$'
    ```

2. Find the tests and fixtures that reach a module:

    ```sh
    rg -n 'std_|from std\.|import std' loaves/toolchain/incan-cli/tests loaves/compiler/incan_emit/tests loaves/compiler/incan_test_support/fixtures loaves/stdlib/core/rust/tests
    ```

3. Pick the row's coverage label from the inventory's label table, and name the tests or fixtures that prove it in the evidence column.
