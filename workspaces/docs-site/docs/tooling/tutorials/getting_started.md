# Getting started with Incan

This tutorial is the shortest public path from an installed toolchain to running, testing, and release-building a project. It does not require cloning the compiler repository.

<aside class="inc-tutorial-meta" aria-label="Tutorial details">
  <dl>
    <div><dt>Reader</dt><dd>First-time Incan evaluator</dd></div>
    <div><dt>Prerequisites</dt><dd>A supported host and a terminal</dd></div>
    <div><dt>Time</dt><dd>10–15 minutes, excluding a cold Rust build</dd></div>
    <div><dt>Verified</dt><dd>Incan <code>&gt;=0.4.0,&lt;0.5.0</code> stable release</dd></div>
    <div><dt>Status</dt><dd>Stable-release executable</dd></div>
    <div><dt>Outcome</dt><dd>A runnable, tested native starter project</dd></div>
    <div><dt>Artifacts</dt><dd><code>hello/</code>, starter tests, and a release binary</dd></div>
  </dl>
</aside>

<ol class="inc-step-rail" style="--inc-step-count: 5" aria-label="Getting started steps">
  <li><strong>Install</strong>Verify the toolchain</li>
  <li><strong>Create</strong>Scaffold a project</li>
  <li><strong>Run</strong>Execute the entry point</li>
  <li><strong>Test</strong>Check behavior</li>
  <li><strong>Build</strong>Produce a release binary</li>
</ol>

## Install and verify

--8<-- "_snippets/learning/install_toolchain.md"

## Create your first project

Create a small starter project and complete the canonical first-contact loop:

--8<-- "_snippets/learning/first_project_loop.md"

This creates:

```text
hello/
├── src/
│   └── main.incn          # Entry point and a small greeting function
├── tests/
│   └── test_main.incn     # Starter test for the greeting function
├── README.md
├── .gitignore
└── incan.toml             # Project manifest with a main script and requires-incan constraint
```

<section class="inc-learning-panel inc-learning-panel--result" data-label="Result" markdown="1">

The starter prints its greeting from `src/main.incn`. At this point the manifest, source root, entry point, and Rust-backed build path are all connected.

</section>

`incan build` already uses the release Cargo profile; `--release` is accepted so the first-contact command spells out the intent.

<section class="inc-learning-panel inc-learning-panel--complete inc-incus-slot" data-label="Complete" data-incus-category="success" markdown="1">

You now have a runnable project, a passing starter test, and a native release build. Continue with [Your first project](your_first_project.md) to split the starter into modules and add meaningful tests.

</section>

## What this release line is good for

The canonical installer currently resolves the stable Incan 0.4 release line, which is the line verified by this first-contact tutorial. It is intended for trying Incan as an installed toolchain, creating small projects, running tests, checking diagnostics, inspecting generated artifacts, and evaluating how Incan fits into Rust-backed application tooling.

Development tutorials identify their own 0.5 compiler range in the tutorial details. Use the docs version selector and that **Verified** field together; do not assume that the stable installer supplies an unreleased development compiler.

## Continue with the development tutorials

The representative project tutorials on the development site use 0.5 language and toolchain features. Switch compiler lines explicitly before following them:

--8<-- "_snippets/learning/development_toolchain.md"

Return to the [0.4 documentation](https://incan.io/v0.4/) when you want material that matches the installed stable release. The version selector and each tutorial's **Verified** field are part of the instructions, not decorative metadata.

## What this release line is not yet good for

Incan is not a Python compatibility runtime, a native Windows installer release, a full package registry, or a promise that generated Rust is a stable ABI. Generated Rust is inspectable current backend output; public compatibility should be based on Incan source, manifests, checked metadata, and documented CLI report schemas.

## Next steps

- [Your first project](your_first_project.md): after switching to the 0.5 development compiler, split the starter into modules and add real tests.
- [CLI reference](../reference/cli_reference.md): commands, flags, and machine-readable outputs.
- [Incan vs Python](../../comparisons/python.md): where Incan tries to win and where Python is still the better choice.
- [Incan vs Rust](../../comparisons/rust.md): why Incan compiles through Rust but does not replace Rust.
- [Encero stack](../../start_here/encero_stack.md): where Incan sits relative to IncQL, Pallay, Omerus, Hees.ai, and Hees.io.
