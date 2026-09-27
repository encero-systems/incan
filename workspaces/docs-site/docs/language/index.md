# Language

This section owns Incan source: syntax, semantics, patterns, and the mental models behind `.incn` programs. Start from what you are trying to learn or build; use the navigation taxonomy only when you already know the page type you need.

## Choose a route

<div class="inc-route-grid">
  <a class="inc-route-card" href="tutorials/book/"><span class="inc-eyebrow">Fundamentals</span><strong>Read the Incan Book</strong><span>Learn values, functions, control flow, errors, models, traits, and tests through small runnable chapters.</span></a>
  <a class="inc-route-card" href="tutorials/guided_project/"><span class="inc-eyebrow">Cumulative</span><strong>Carry one project through the language</strong><span>Apply functions, results, modules, tests, locking, and native builds to one typed workflow step.</span></a>
  <a class="inc-route-card" href="../tooling/tutorials/build_and_consume_library/"><span class="inc-eyebrow">Package</span><strong>Build a library boundary</strong><span>Choose an explicit public surface, build its checked artifact, and run a locked-workspace consumer.</span></a>
  <a class="inc-route-card" href="tutorials/checked_c_binding/"><span class="inc-eyebrow">Native boundary</span><strong>Build a checked C binding</strong><span>Use the experimental checked-C surface when a governed native ABI is the right boundary.</span></a>
  <a class="inc-route-card" href="../start_here/"><span class="inc-eyebrow">Background bridge</span><strong>Translate what you already know</strong><span>Choose the Python, Rust, TypeScript/JavaScript, or automation route from the Learn hub.</span></a>
</div>

## Recommended learning sequence

1. Complete [Getting Started](../tooling/tutorials/getting_started.md) so the compiler, project, tests, and release build all work.
2. Use the [Book](tutorials/book/index.md) for syntax and fundamentals.
3. Follow the [guided project spine](tutorials/guided_project.md) or choose a representative [project tutorial](../start_here/index.md#build-with-the-05-release-envelope).
4. Keep [how-to guides](how-to/error_handling_recipes.md) and the [generated language reference](reference/language.md) as lookup surfaces.

## Choose the kind of answer

<div class="inc-route-grid">
  <a class="inc-route-card" href="tutorials/guided_project/"><span class="inc-eyebrow">Tutorial</span><strong>Learn by completing an outcome</strong><span>Use a staged, release-envelope executable project when you want the reasoning and sequence together.</span></a>
  <a class="inc-route-card" href="how-to/error_handling_recipes/"><span class="inc-eyebrow">How-to</span><strong>Solve a focused task</strong><span>Use recipes for errors, files, JSON, async work, modules, tests, collections, and Rust interop.</span></a>
  <a class="inc-route-card" href="reference/language/"><span class="inc-eyebrow">Reference</span><strong>Look up the contract</strong><span>Use generated syntax, type, built-in, derive, and standard-library surfaces when you know what to find: exact syntax and signatures, behavior stated without a walkthrough, and what holds across versions.</span></a>
  <a class="inc-route-card" href="explanation/how_incan_works/"><span class="inc-eyebrow">Explanation</span><strong>Understand why it works this way</strong><span>Read the mental models behind compilation, errors, modules, models, traits, and Rust-shaped confidence.</span></a>
</div>

## Common lookup points

- [Feature inventory](reference/feature_inventory.md)
- [Error handling](explanation/error_handling.md)
- [Models and classes](explanation/models_and_classes/index.md)
- [Imports and modules](how-to/imports_and_modules.md)
- [Async programming](how-to/async_programming.md)
- [Rust interop](how-to/rust_interop.md)
- [Checked C bindings](tutorials/checked_c_binding.md)
- [Web framework](tutorials/web_framework.md)
- [Standard library index](reference/stdlib/index.md)

For installation, projects, commands, editor support, reports, and CI, continue to [Tooling](../tooling/index.md). RFCs remain [design records for contributors](../RFCs/index.md), not a prerequisite for learning the language.

The [typed data processor](tutorials/typed_data_processor.md), [typed API](tutorials/build_your_first_api.md), [async worker](tutorials/async_worker_pipeline.md), and [external Rust crate](tutorials/add_a_rust_crate.md) are executable 0.5 projects. Standard-library work reuses the full toolchain Loaf; project-specific dependencies enter through an explicit Oven bake rather than a hidden Cargo fallback.
