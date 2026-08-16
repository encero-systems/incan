# Learn Incan

Choose the shortest route to your goal. You do not need to read the documentation in navigation order.

<div class="inc-route-grid" aria-label="Learning routes">
  <a class="inc-route-card" data-learning-route="evaluate" href="what_incan_is_for/"><span class="inc-eyebrow">Evaluate</span><strong>Decide whether Incan fits</strong><span>Review the use cases, maturity boundaries, and cases where another language is still the better choice.</span></a>
  <a class="inc-route-card" data-learning-route="try" href="../tooling/tutorials/getting_started/"><span class="inc-eyebrow">Try</span><strong>Run a first project</strong><span>Install the toolchain, create a starter, run it, test it, and produce a native release build.</span></a>
  <a class="inc-route-card" data-learning-route="learn" href="../language/tutorials/book/"><span class="inc-eyebrow">Learn</span><strong>Study the fundamentals</strong><span>Follow the runnable Book through values, functions, control flow, errors, models, traits, and tests.</span></a>
  <a class="inc-route-card" data-learning-route="build" href="#build-with-the-05-release-envelope"><span class="inc-eyebrow">Build</span><strong>Make a representative project</strong><span>Choose an executable application, library, or automation route before exploring preview-only surfaces.</span></a>
  <a class="inc-route-card" data-learning-route="bridge" href="#translate-your-mental-model"><span class="inc-eyebrow">Bridge</span><strong>Translate what you know</strong><span>Start from Python, Rust, TypeScript/JavaScript, or pipeline and automation experience.</span></a>
  <a class="inc-route-card" data-learning-route="reference" href="../language/reference/language/"><span class="inc-eyebrow">Reference</span><strong>Look something up</strong><span>Use the generated language reference, standard-library index, and CLI reference as lookup surfaces.</span></a>
</div>

??? warning "Beta status — read before adopting"
    --8<-- "_snippets/callouts/beta_expectations.md"

    [Review the release notes](../release_notes/index.md).

If the first-contact loop fails, go directly to [Troubleshooting](../tooling/how-to/troubleshooting.md). To evaluate the longer-term direction, read the [1.0 domain-native demo target](domain_native_demo.md) and [1.0 public contracts](public_contracts.md).

## Choose the right problem shape

Not every data-shaped problem belongs in Incan. Use the [Incan or IncQL? chooser](incan_or_incql.md) before translating SQL, dataframe, query-planning, or lineage work into application code.

## Build with the 0.5 release envelope

The executable projects below were exercised with the prepared 0.5 release envelope. Complete [Getting Started](../tooling/tutorials/getting_started.md), then use its [development-toolchain step](../tooling/tutorials/getting_started.md#continue-with-the-development-tutorials) before starting one of them.

<div class="inc-route-grid" aria-label="Executable project tutorials">
  <a class="inc-route-card" data-learning-project="first-project" data-project-status="executable" href="../tooling/tutorials/your_first_project/"><span class="inc-eyebrow">Application</span><strong>Build a real first project</strong><span>Split a command-line project into modules, replace the placeholder test, run it, and verify the result.</span></a>
  <a class="inc-route-card" data-learning-project="pipeline-step" data-project-status="executable" href="../tooling/tutorials/pipeline_mini_project/"><span class="inc-eyebrow">Automation</span><strong>Build a typed workflow step</strong><span>Run and test a deterministic step with explicit success and failure types.</span></a>
  <a class="inc-route-card" data-learning-project="diagnose-build" data-project-status="executable" href="../tooling/tutorials/diagnose_failed_build/"><span class="inc-eyebrow">Diagnosis</span><strong>Understand a failed build</strong><span>Capture diagnostic JSON, ask the compiler to explain it, repair the source, and inspect the backend.</span></a>
  <a class="inc-route-card" data-learning-project="guided-project" data-project-status="executable" href="../language/tutorials/guided_project/"><span class="inc-eyebrow">Guided spine</span><strong>Carry one workflow through the fundamentals</strong><span>Apply functions, explicit failures, modules, tests, and delivery to one cumulative project.</span></a>
</div>

The [library package tutorial](../tooling/tutorials/build_and_consume_library.md) is a source-verified preview: its producer and consumer check with the 0.5 compiler, while native library output still needs a matching Loaf beyond the finite release envelope.

## Explore source-verified previews

These pages are checked against the 0.5 compiler, but their provider closure is intentionally outside the finite release envelope. Each page states its exact completion boundary; none presents a missing Loaf as a runnable artifact.

<div class="inc-route-grid" aria-label="Preview project tutorials">
  <a class="inc-route-card" data-learning-project="typed-data-processor" data-project-status="preview" href="../language/tutorials/typed_data_processor/"><span class="inc-eyebrow">Preview · Typed serde</span><strong>Design a typed data processor</strong><span>Check a JSON-in/JSON-out project whose typed-serde runtime closure is not shipped in 0.5.</span></a>
  <a class="inc-route-card" data-learning-project="first-api" data-project-status="preview" href="../language/tutorials/build_your_first_api/"><span class="inc-eyebrow">Preview · Web</span><strong>Trace a typed API</strong><span>Check route and response contracts without claiming an unavailable native web runtime.</span></a>
  <a class="inc-route-card" data-learning-project="async-worker" data-project-status="preview" href="../language/tutorials/async_worker_pipeline/"><span class="inc-eyebrow">Preview · Async</span><strong>Model an async worker pipeline</strong><span>Check task, join, and deadline contracts without claiming an unavailable async Loaf.</span></a>
  <a class="inc-route-card" data-learning-project="rust-crate" data-project-status="preview" href="../language/tutorials/add_a_rust_crate/"><span class="inc-eyebrow">Preview · Rust crate</span><strong>Shape a crate boundary</strong><span>Verify the import and lock contract while arbitrary crate closures remain outside 0.5.</span></a>
  <a class="inc-route-card" data-learning-project="ci-artifact" data-project-status="preview" href="../tooling/tutorials/project_to_ci_artifact/"><span class="inc-eyebrow">Preview · Delivery</span><strong>Prepare a CI artifact lane</strong><span>Use the verified local commands while hosted 0.5 toolchain installation is still a packaging preview.</span></a>
</div>

## Translate your mental model

<div class="inc-route-grid" aria-label="Background bridges">
  <a class="inc-route-card" href="coming_from_python/"><span class="inc-eyebrow">Python</span><strong>Application and tooling code</strong><span>Keep readable application shapes while making types, errors, builds, and deployment more explicit.</span></a>
  <a class="inc-route-card" href="coming_from_rust/"><span class="inc-eyebrow">Rust</span><strong>Evaluate the higher-level boundary</strong><span>Keep native builds, results, traits, and crates without treating Incan as a low-level Rust replacement.</span></a>
  <a class="inc-route-card" href="coming_from_typescript_javascript/"><span class="inc-eyebrow">TypeScript / JavaScript</span><strong>Move a CLI or service to a native toolchain</strong><span>Translate modules and application types without expecting a JavaScript runtime or npm package compatibility.</span></a>
  <a class="inc-route-card" href="pipelines_and_automation/"><span class="inc-eyebrow">Automation</span><strong>Build a reproducible typed workflow</strong><span>Start with a processor or pipeline and carry it into explicit failures, tests, locks, and CI reports.</span></a>
</div>

## More routes

- [New to typed programming](beginner.md)
- [See how Incan fits the Encero stack](encero_stack.md)
- [Contribute to the compiler and documentation](../contributing/index.md)

These Learn pages route readers by intent. Canonical language, tooling, reference, and contributor material remains in its owning section.
