# Hees.ai v0.5 installed-SDK validation

This validation contract defines how the constrained Hees.ai proof lane for issue [#552](https://github.com/encero-systems/incan/issues/552) proves that it can run from an installed Incan SDK instead of a repo-local compiler checkout.

Status: Incan-side validation contract. Closing #552 still requires a recorded run from the Hees.ai revision that carries the governed-workbench proof; this reference defines the commands and evidence that run must provide.

## North star

A maintainer with the staged v0.5 SDK installed should be able to enter the Hees.ai project, run one documented command sequence, and capture evidence that the proof path uses the installed `incan` command. The SDK does not need to be publicly released first: an installed artifact produced by the release pipeline is valid when its version and artifact identity are recorded. Public proof docs must not require `/Users/.../incan/target/debug/incan`, a sibling Incan checkout, or generated Rust paths from a compiler repo.

The validation should prove Incan surfaces, not Hees.ai product polish:

- stable installed command discovery;
- project locking, checking, testing, building, and reporting through public CLI commands;
- workbench wrapper behavior that honors the installed SDK path;
- trace/report/decision artifacts that demonstrate governed behavior;
- explicit blocker routing for missing compiler, stdlib, runtime, package, or tooling surfaces.

## Command levels

Use levels so the proof can keep moving without pretending that every environment has local models, ample disk, or network.

| Level | Purpose | Commands | Required evidence |
| --- | --- | --- | --- |
| SDK discovery | Prove the caller is not using a repo-local compiler binary. | `INCAN_BIN="$(command -v incan)"`; `"$INCAN_BIN" --version`; `INCAN_BIN="$INCAN_BIN" bin/hees-workbench --print-incan-bin`. | The printed path resolves to the installed staged SDK command or explicitly supplied installed SDK binary, not `../incan/target/debug/incan`. Record the v0.5 version and release-artifact identity. An earlier stable SDK may test wrapper discovery, but it does not satisfy v0.5 release evidence. |
| Static project check | Prove the project can be checked from the installed SDK without running the workbench. | `"$INCAN_BIN" check src/hees_workbench.incn --format json`; optionally `"$INCAN_BIN" check src/main.incn --format json`. | JSON diagnostics are empty or contain linked blockers with issue numbers. The command should not require a sibling compiler checkout. |
| Lock and contract tests | Prove package resolution and ordinary test surfaces. | `"$INCAN_BIN" lock`; `"$INCAN_BIN" test`. | Fresh lock state, test result, compiler version, and any dependency-resolution blockers. This level may create target output and should be skipped during disk-pressure incidents unless the caller accepts that cost. |
| Workbench build-only | Prove the wrapper can build the selected workbench target with the installed SDK. | `INCAN_BIN="$INCAN_BIN" HEES_WORKBENCH_BUILD_ONLY=1 bin/hees-workbench digital_wellness`. | Wrapper-resolved compiler path, exit status, build/report artifact paths, and disk impact. This level may rebuild Cargo output. |
| Live governed proof | Prove the flagship #549 behavior when local model/runtime prerequisites are present. | `INCAN_BIN="$INCAN_BIN" bin/hees-workbench digital_wellness`. | Stable progress lines plus `target/hees-workbench/latest_trace.json`, `latest_report.md`, `latest_candidates.json`, `latest_decisions.json`, and policy artifact paths. If Ollama or model prerequisites are absent, record that as an environment blocker rather than an Incan failure. |

## Public documentation rules

Public Hees.ai proof docs should show `incan` or an explicit `INCAN_BIN="$(command -v incan)"` setup. They must not instruct users to run `/Users/danny/Development/encero/incan/target/debug/incan` or any other compiler-repo `target/debug/incan` path.

Repo-local compiler paths are acceptable only in private spike notes or failure forensics, and only when the note also explains the installed-SDK blocker that forced the local path.

Wrappers should resolve compilers in this order:

1. honor an explicit `INCAN_BIN`;
2. use `command -v incan` when available;
3. fail with a message that asks the operator to install Incan or set `INCAN_BIN`.

They should not silently prefer a sibling checkout's `target/debug/incan` in the default public proof path. A local-debug escape hatch may exist, but it must be opt-in and should not be the first resolution path.

## Blocker categories

Every failed validation step should be classified before opening or updating issues.

| Category | Route |
| --- | --- |
| Installed SDK discovery or release layout failure | #552 or the installer/release-manifest owner. |
| Project lock, package, or dependency resolution failure | Link the exact compiler/tooling issue; do not hide it as a Hees-only quirk. |
| Missing stdlib/runtime surface such as environment, process, HTTP, web lifecycle, time, or byte streams | Link the owning stdlib/runtime issue, such as #557, #341, #440, #526, #579, or #544. |
| Missing backend-neutral source identity, semantic facts, HIR, or type facts | Link #634, #648, #649, #650, #282, or #224. |
| Product scenario, domain copy, UI polish, corpus content, or local model availability | Track in Hees.ai, not in the Incan milestone. |

## Handoff and evidence ownership

This reference deliberately does not describe one mutable Hees.ai checkout as the current implementation. The Hees.ai revision selected for the proof owns its wrapper and public-documentation changes. Issue #552 must link that exact revision and record the command output, SDK version, installed binary path, and produced artifacts.

If the selected consumer revision lacks `bin/hees-workbench --print-incan-bin` or the installed-command resolution order above, fix that consumer boundary before recording release evidence. Do not substitute edits in a dirty checkout or a compiler-repository binary and call the proof complete.

## Done criteria for #552

#552 is done when:

- the Hees.ai proof docs expose an installed-SDK command sequence;
- the workbench wrapper path prints and uses the installed SDK command by default;
- any remaining local-only assumptions are recorded as linked blockers;
- at least the SDK discovery and static project check levels have been run with the installed v0.5 artifact selected for release, with its exact version and artifact identity recorded;
- any skipped build/test/live levels state the reason, such as disk pressure, missing model service, or an open compiler/runtime blocker;
- IncQL and Pallay validation remain out of scope.
