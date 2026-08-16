# Pipelines and automation

This route is for automation and data-tool builders who want typed boundaries, reproducible commands, and CI-friendly native artifacts.

<aside class="inc-bridge-note inc-incus-slot" data-incus-category="info" aria-label="Automation and pipeline mental model">
  <span class="inc-eyebrow">Automation → Incan</span>
  <strong>Keep explicit stages and repeatable commands. Move inputs, failures, and output contracts into types the compiler can check.</strong>
</aside>

## Choose your route

<div class="inc-route-grid">
  <a class="inc-route-card" href="../tooling/tutorials/pipeline_mini_project/"><span class="inc-eyebrow">First project</span><strong>Build a staged workflow</strong><span>Compose an executable project with explicit inputs, steps, failures, outputs, and tests.</span></a>
  <a class="inc-route-card" href="../tooling/tutorials/diagnose_failed_build/"><span class="inc-eyebrow">Operate</span><strong>Diagnose a failed build</strong><span>Capture structured diagnostics, explain a stable code, repair the source, and inspect the backend.</span></a>
  <a class="inc-route-card" href="../tooling/tutorials/project_to_ci_artifact/"><span class="inc-eyebrow">Preview · Delivery</span><strong>Prepare a CI artifact lane</strong><span>Use the verified local gate while hosted 0.5 toolchain installation remains a packaging preview.</span></a>
  <a class="inc-route-card" href="../tooling/how-to/ci_and_automation/"><span class="inc-eyebrow">Operate</span><strong>Use the CI contracts</strong><span>Apply formatting, testing, locked/offline, exit-code, and report surfaces in automation.</span></a>
</div>

The [typed data processor](../language/tutorials/typed_data_processor.md) is a source-verified preview of the broader typed-serde shape. Its runtime closure is not shipped in the finite 0.5 release envelope.

## Install once

Follow [Getting Started](../tooling/tutorials/getting_started.md) for the current installer choices and canonical first-project commands.

## What transfers

- Staged transformations, explicit inputs and outputs, and repeatable operational boundaries
- Deterministic commands, exit codes, locked dependencies, and inspectable reports
- Modules, file I/O, typed errors, tests, and release artifacts

## What changes

- Each fallible boundary returns a typed value rather than relying on logs or ambient shell state.
- Data contracts and transformations can live in ordinary program modules with tests.
- A native executable becomes the repeatable automation unit.

## What not to expect

- SQL, dataframe, Spark, or notebook compatibility; classify relational query and plan work with the [Incan or IncQL? chooser](incan_or_incql.md)
- A remote package registry or hosted pipeline service
- Enterprise deployment policy beyond the documented lock, offline, report, and exit-code contracts

## Continue

- [Error handling](../language/explanation/error_handling.md)
- [Imports and modules](../language/explanation/imports_and_modules.md)
- [File I/O](../language/how-to/file_io.md)
