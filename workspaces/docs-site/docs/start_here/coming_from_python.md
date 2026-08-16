# Coming from Python (apps)

This route is for Python developers evaluating Incan for application code, services, typed domain packages, and deployment-oriented tools.

<aside class="inc-bridge-note inc-incus-slot" data-incus-category="python" aria-label="Python to Incan mental model">
  <span class="inc-eyebrow">Python → Incan</span>
  <strong>Keep the readable application-code shape. Move more failure, type, and deployment facts into contracts the compiler can check.</strong>
</aside>

## Choose your route

<div class="inc-route-grid">
  <a class="inc-route-card" href="../tooling/tutorials/getting_started/"><span class="inc-eyebrow">Start</span><strong>Run the first project</strong><span>Use the canonical installer and complete the run, test, and release-build loop.</span></a>
  <a class="inc-route-card" href="../tooling/tutorials/your_first_project/"><span class="inc-eyebrow">First project</span><strong>Build a small native command</strong><span>Keep the readable function-and-module shape while adding explicit exports, tests, and a locked native build.</span></a>
  <a class="inc-route-card" href="../tooling/tutorials/pipeline_mini_project/"><span class="inc-eyebrow">Workflow</span><strong>Compose a typed processing step</strong><span>Model an input, return a typed result, split the project into modules, and test both paths.</span></a>
  <a class="inc-route-card" href="../comparisons/python/"><span class="inc-eyebrow">Evaluate</span><strong>Compare Python and Incan</strong><span>Check ecosystem, runtime, deployment, type, and error-model tradeoffs before adopting.</span></a>
</div>

## Preview broader 0.5 source surfaces

The [typed data processor](../language/tutorials/typed_data_processor.md) and [typed API](../language/tutorials/build_your_first_api.md) show the intended serde and web authoring surfaces. Their source is checked against the 0.5 development compiler, but the finite 0.5 release envelope does not yet ship the typed-serde or web Loafs required to execute them. Use them to evaluate language shape, not as the first successful runtime exercise.

## Install once

Follow [Getting Started](../tooling/tutorials/getting_started.md) for the current installer choices and canonical `new` → `run` → `test` → `build` loop. If you use Python tooling every day, the `pipx` option keeps the `incan` command isolated from project environments; it does not make Incan a Python package runtime.

## What transfers

- Readable functions, named arguments, modules, collections, comprehensions, and model-shaped application code
- Familiar function, module, and result composition for application work
- A small amount of syntax around an explicit, testable application core

## What changes

- `Result`, `Option`, and `?` make ordinary fallibility part of the function contract instead of an exception-only convention.
- Models and derives move validation and serialization behavior into compiler-visible types.
- The toolchain produces native binaries through Rust rather than executing source in CPython.
- A manifest and lockfile own project and dependency facts that Python tools often distribute across several files.

## What not to expect

- CPython compatibility, Python package compatibility, or the ability to import arbitrary PyPI packages
- Notebook or dataframe compatibility merely because the source syntax is familiar
- Dynamic monkey-patching as an application extension model

## Continue

- [The Incan Book](../language/tutorials/book/index.md) for a linear fundamentals route
- [Error handling](../language/explanation/error_handling.md) for the `Result` and `Option` model
- [Models and classes](../language/explanation/models_and_classes/index.md) for data and behavior types
- [Editor setup](../tooling/how-to/editor_setup.md) and [Troubleshooting](../tooling/how-to/troubleshooting.md) for the working loop
