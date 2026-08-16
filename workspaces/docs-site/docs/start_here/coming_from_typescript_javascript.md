# Coming from TypeScript or JavaScript

This route is for TypeScript and JavaScript developers evaluating Incan for command-line tools, services, and typed application packages.

<aside class="inc-bridge-note inc-incus-slot" data-incus-category="javascript" aria-label="TypeScript and JavaScript to Incan mental model">
  <span class="inc-eyebrow">TypeScript / JavaScript → Incan</span>
  <strong>Keep explicit application structure. Move runtime-only assumptions into types, results, and compiler-owned project facts.</strong>
</aside>

## Choose your route

<div class="inc-route-grid">
  <a class="inc-route-card" href="../tooling/tutorials/getting_started/"><span class="inc-eyebrow">Start</span><strong>Run the first project</strong><span>Install the toolchain once, then run, test, and release-build a native starter.</span></a>
  <a class="inc-route-card" href="../tooling/tutorials/your_first_project/"><span class="inc-eyebrow">First project</span><strong>Build a native command</strong><span>Use modules, explicit exports, typed functions, tests, and a native build without a JavaScript runtime.</span></a>
  <a class="inc-route-card" href="../tooling/tutorials/build_and_consume_library/"><span class="inc-eyebrow">Package preview</span><strong>Design a local typed boundary</strong><span>Source-check a deliberately small library API and its locked-workspace consumer.</span></a>
  <a class="inc-route-card" href="../comparisons/javascript_typescript/"><span class="inc-eyebrow">Evaluate</span><strong>Compare JS/TS and Incan</strong><span>Check runtime, deployment, type, error, package, and ecosystem tradeoffs before adopting.</span></a>
</div>

## Preview service and typed-serde surfaces

The [typed API](../language/tutorials/build_your_first_api.md) and [typed data processor](../language/tutorials/typed_data_processor.md) are source-verified previews of the intended web and serde authoring experience. Their runtime Loafs are not shipped in the finite 0.5 release envelope, so they supplement rather than replace the executable routes above.

## Install once

Follow [Getting Started](../tooling/tutorials/getting_started.md) for the current installer choices and canonical first-project commands. If you already use Node tooling, the npm option installs command shims plus a host-specific release package; it does not make Incan a JavaScript runtime or TypeScript transpiler.

## What transfers

- Modules, async functions, collection transforms, JSON-shaped models, and service or CLI architecture
- Static application types as documentation and editor feedback
- Familiar package-manager ergonomics for installing the compiler command

## What changes

- Types participate in source checking and native compilation rather than being erased before execution.
- `Result`, `Option`, and `?` make ordinary failure paths explicit instead of relying on exceptions or rejected promises.
- Manifests, lockfiles, build reports, and generated-code inspection are compiler-owned project surfaces.
- The deployment artifact is native rather than a JavaScript bundle plus runtime.

## What not to expect

- A JavaScript runtime, TypeScript transpiler, npm package compatibility, or browser DOM APIs
- Structural typing to behave exactly like TypeScript
- Existing Node services to run unchanged

## Continue

- [How Incan works](../language/explanation/how_incan_works.md)
- [Error handling](../language/explanation/error_handling.md)
- [Rust-shaped confidence](../language/explanation/rust_shaped_confidence.md)
- [CLI reference](../tooling/reference/cli_reference.md)
