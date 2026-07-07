# Hees.ai v0.5 dependency inventory

This inventory seeds issue [#651](https://github.com/encero-systems/incan/issues/651). It keeps the Hees.ai proof lane constrained: Hees.ai may validate Incan compiler, stdlib, runtime, and tooling direction, but it must not quietly turn the 0.5 milestone into broad product scope.

Status: seed inventory. It maps known proof-lane needs from [#549](https://github.com/encero-systems/incan/issues/549) and [#552](https://github.com/encero-systems/incan/issues/552) to provider lanes and guardrails.

## Categories

| Category | Meaning | Action |
| --- | --- | --- |
| Existing 0.4 tooling surface | Already part of the install/check/build/test/inspect/report path. | Consume it directly through installed `incan` commands where possible. |
| 0.5 backend foundation dependency | Needs stable IDs, semantic facts, `IncanType`, HIR, Body IR, ABI hooks, or backend migration scaffolding. | Link to #634, #648, #649, #650, #282, #224, or a child issue. |
| 0.5 stdlib/runtime dependency | Needs source-authored stdlib, runtime helpers, process/env/time/web/archive/http/io behavior, or stdlib packaging work. | Link to the owning stdlib/runtime issue; do not hide it in Hees.ai. |
| Temporary compatibility shim | Needed to keep the proof path running before the general Incan surface exists. | Record owner, removal condition, and why it is not general language scope. |
| Hees.ai-only product behavior | Product policy, content, UI, domain copy, scenario data, or business workflow not needed by Incan itself. | Keep it out of Incan release scope; track it in Hees.ai. |

## Proof-lane requirements

| Requirement | Provider lane | Current mapping | Guardrail |
| --- | --- | --- | --- |
| Run from an installed SDK path, not a compiler checkout. | Existing 0.4 tooling plus #552. | `incan` install/run/check/test/build/report commands, SDK layout validation. | Do not depend on `/target/debug/incan` or repo-local source paths in public proof docs. |
| Produce inspectable diagnostics, build reports, generated artifacts, or codegraph facts. | Existing 0.4 tooling. | `incan check --format json`, `incan build --report json`, `incan inspect rust`, `incan inspect codegraph`. | Treat these as consumed surfaces; do not add product-only output schemas to Incan. |
| Explain why the governed workflow is not a generic chatbot demo. | Hees.ai product behavior plus existing docs surfaces. | #549 scenario/script/artifact docs. | Product narrative belongs to Hees.ai; Incan docs should only explain compiler/runtime/tooling validation. |
| Stable source identity for trace/report/decision artifacts. | 0.5 backend foundation. | #634 umbrella, #648 stable IDs/facts, #650 HIR v0. | Do not fake stable identity by scraping generated Rust or line text alone. |
| Type/runtime obligations visible enough for later backend work. | `IncanType`, ABI v0, runtime-service facts. | #649 semantic type model and ABI v0 hooks. | Record unknowns explicitly; do not assume hosted `std` in future-facing metadata. |
| Runtime and stdlib services for proof execution. | 0.5 stdlib/runtime lane. | Candidate providers include #557 `std.environ`, #341 `std.process`, #440 std.web lifecycle, #579 fallible reader chunk streams, #526 package-level timezones, and #544 compiled stdlib modules. | Link exact needs before implementation; do not import broad stdlib scope into Hees.ai by default. |
| Dependency and package boundary sanity. | Tooling/package lane. | Existing dependency handling and future incan.pub/package work where needed. | Use explicit blocker issues when SDK/package behavior is missing. |
| Policy/invariant enforcement artifacts. | Hees.ai product behavior unless generalized. | #549 should define the scenario and expected trace/report/decision outputs. | Only promote a behavior to Incan scope if another Incan user needs the same general compiler/std/tooling capability. |

## Existing validation anchors

Use these anchors before creating new proof-only machinery:

| Surface | Existing anchors |
| --- | --- |
| SDK install and package-manager shims | `tests/sdk_installer_tests.rs`, `workspaces/release/install-incan-sdk.sh`, `workspaces/release/sdk/manifest.schema.v1.json`, `workspaces/release/npm/**`, `workspaces/release/pip/**`. |
| Starter and zero-clone flow | `tests/integration_tests.rs`, `src/cli/commands/init.rs`, `workspaces/docs-site/docs/language/reference/project_lifecycle.md`, `workspaces/docs-site/docs/tooling/how-to/install_and_run.md`. |
| JSON diagnostics, build reports, and codegraph inspection | `tests/cli_integration.rs`, `src/cli/commands/diagnostics.rs`, `src/cli/commands/build_report.rs`, `src/cli/commands/codegraph.rs`, `workspaces/docs-site/docs/tooling/reference/cli_reference.md`, `workspaces/docs-site/docs/tooling/reference/codegraph_inspection.md`. |
| Checked public API facts | `workspaces/docs-site/docs/tooling/reference/checked_api_metadata.md`, `tests/fixtures/boundary_parity/README.md`. |
| Generated Rust inspection and artifact contracts | `tests/generated_rust_artifact_tests.rs`, `tests/generated_rust_audit_tests.rs`, `tests/generated_rust_native_consumer_tests.rs`, `workspaces/docs-site/docs/contributing/how-to/auditing_generated_rust.md`. |

## Initial dependency map

| Hees.ai need | Classification | Provider issue or source | Notes |
| --- | --- | --- | --- |
| Installed-command validation path. | Existing 0.4 tooling / Hees.ai validation. | [#552](https://github.com/encero-systems/incan/issues/552). | The proof should prefer installed `incan` commands and report local-only assumptions as blockers. |
| Governed workbench demo scenario. | Hees.ai-only product behavior with Incan validation hooks. | [#549](https://github.com/encero-systems/incan/issues/549). | Keep scenario copy, policy content, and product UX outside Incan compiler scope. |
| Stable compiler-owned identities for evidence. | 0.5 backend foundation. | [#648](https://github.com/encero-systems/incan/issues/648), [#650](https://github.com/encero-systems/incan/issues/650). | Needed if proof artifacts cite source declarations/calls beyond text spans. |
| Backend-neutral type/runtime facts. | 0.5 backend foundation. | [#649](https://github.com/encero-systems/incan/issues/649). | Needed if proof artifacts explain type or runtime obligations in Incan terms. |
| Current-behavior parity checklist. | 0.5 phase-0 foundation. | [#646](https://github.com/encero-systems/incan/issues/646). | Hees.ai should consume supported behavior, not accidental generated-Rust quirks. |
| Old-backend compatibility policy. | 0.5 phase-0 foundation. | [#647](https://github.com/encero-systems/incan/issues/647). | Temporary shims must identify their middle-end destination. |
| Stdlib source packaging boundary. | 0.5 stdlib/runtime. | [#544](https://github.com/encero-systems/incan/issues/544). | Relevant if Hees.ai exposes stdlib materialization or SDK layout issues. |
| Runtime environment access. | 0.5 stdlib/runtime. | [#557](https://github.com/encero-systems/incan/issues/557). | Only include if the proof path reads process environment through Incan. |
| Process execution. | 0.5 stdlib/runtime. | [#341](https://github.com/encero-systems/incan/issues/341). | Only include if the proof path needs a general `std.process` API. |
| Web app lifecycle/task supervision. | 0.5 stdlib/runtime. | [#440](https://github.com/encero-systems/incan/issues/440). | Candidate dependency for a hosted workbench, not required until the proof path says so. |

## Shim rules

Temporary shims are allowed only when they make the proof path runnable while a general provider issue is still open.

Every shim needs:

- owning Hees.ai issue or Incan provider issue;
- explicit reason the general surface is not ready;
- command or artifact proving the shim is exercised;
- removal condition;
- confirmation that the shim does not change Incan language semantics.

## Out-of-scope examples

These belong outside Incan 0.5 unless separately approved:

- Product UI polish for Hees.ai.
- Domain-specific policy copy.
- Product analytics dashboards.
- Broad InQL/Pallay SDK validation beyond the constrained Hees.ai proof path.
- New language features whose only evidence is a product-specific convenience.

## Next inventory actions

1. Freeze the #549 scenario transcript/artifact list before implementing dependencies.
2. Run the #552 installed-SDK path and record exact blockers.
3. For every blocker, decide whether it is existing 0.4 tooling, 0.5 backend foundation, 0.5 stdlib/runtime, temporary shim, or Hees.ai-only.
4. Reject any product need that cannot explain how it validates Incan itself.
