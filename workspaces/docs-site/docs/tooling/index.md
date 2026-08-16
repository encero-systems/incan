# Tooling

This section owns the installed Incan experience: projects, commands, editor support, diagnostics, build evidence, and CI.

## Choose a route

<div class="inc-route-grid">
  <a class="inc-route-card" href="tutorials/getting_started/"><span class="inc-eyebrow">First contact</span><strong>Install and run Incan</strong><span>Use the canonical installer and complete the new, run, test, and release-build loop.</span></a>
  <a class="inc-route-card" href="tutorials/your_first_project/"><span class="inc-eyebrow">Project</span><strong>Move beyond the starter</strong><span>Split a command-line application into modules and replace placeholder tests with meaningful behavior.</span></a>
  <a class="inc-route-card" href="tutorials/project_to_ci_artifact/"><span class="inc-eyebrow">Preview · Delivery</span><strong>Prepare a CI artifact lane</strong><span>Verify the local gate and report contract while hosted 0.5 toolchain installation remains a packaging preview.</span></a>
  <a class="inc-route-card" href="tutorials/diagnose_failed_build/"><span class="inc-eyebrow">Diagnosis</span><strong>Explain a failed build</strong><span>Capture stable diagnostic JSON, ask the compiler to explain it, repair the source, and inspect backend output.</span></a>
</div>

## Canonical first-contact flow

[Getting Started](tutorials/getting_started.md) is the single source for installation and the `new` → `run` → `test` → `build` loop. Audience bridge pages translate concepts but do not maintain competing setup commands.

## Work by task

<div class="inc-route-grid">
  <a class="inc-route-card" href="how-to/editor_setup/"><span class="inc-eyebrow">Write</span><strong>Configure editor feedback</strong><span>Set up the LSP and keep formatting feedback close to the source.</span></a>
  <a class="inc-route-card" href="how-to/testing/"><span class="inc-eyebrow">Verify</span><strong>Run and filter tests</strong><span>Use project tests, filters, lock enforcement, and CI-friendly exit behavior.</span></a>
  <a class="inc-route-card" href="how-to/ci_and_automation/"><span class="inc-eyebrow">Automate</span><strong>Use reproducible CI commands</strong><span>Apply format, check, test, lock, offline, and report contracts without scraping prose.</span></a>
  <a class="inc-route-card" href="explanation/oven_alpha/"><span class="inc-eyebrow">Build system</span><strong>Understand Oven Alpha</strong><span>See how 0.5 selects receipt-compatible Loafs, executes direct-rustc plans, and accounts for storage.</span></a>
  <a class="inc-route-card" href="reference/cli_reference/"><span class="inc-eyebrow">Reference</span><strong>Look up a command contract</strong><span>Find exact flags, environment variables, diagnostics, reports, and inspection schemas.</span></a>
</div>

## Deeper project paths

- [Build and consume an Incan library](tutorials/build_and_consume_library.md)
- [Build a pipeline mini-project](tutorials/pipeline_mini_project.md)
- [Manage dependencies](how-to/dependencies.md)
- [Understand Oven Alpha](explanation/oven_alpha.md)
- [Understand projects today](explanation/projects_today.md)
- [Inspect compiler-backed codegraph data](reference/codegraph_inspection.md)
- [Use agent and tooling documentation surfaces](reference/agent_docs_surfaces.md)

Repository `make` targets remain the [contributor path](../contributing/how-to/ci_and_automation.md) for working on the Incan compiler itself.
