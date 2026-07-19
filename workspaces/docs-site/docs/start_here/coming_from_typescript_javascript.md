# Coming from TypeScript or JavaScript

This page is a routing guide for TypeScript and JavaScript developers evaluating Incan for application code, command-line tools, services, and typed domain packages.

<aside class="inc-bridge-note inc-incus-slot" data-incus-category="javascript" aria-label="TypeScript and JavaScript to Incan mental model">
  <span class="inc-eyebrow">TypeScript / JavaScript → Incan</span>
  <strong>Keep explicit application structure. Move runtime-only assumptions into types, results, and compiler-owned project facts.</strong>
</aside>

## Install first

If you already use Node-based tooling, install the npm adapter. It installs command shims plus a host-specific optional platform package for the same prebuilt Incan toolchain payloads used by the release installers, without running an npm lifecycle script:

```bash
npm install -g @incan/toolchain
incan --version
incan-lsp --version
```

The npm path exposes `incan` and `incan-lsp` immediately, but it does not run rustup during installation. Make sure Rust and `wasm32-wasip1` are available before building projects, or use the direct installer when you want Rust provisioning and explicit control over the toolchain manifest:

```bash
--8<-- "_snippets/commands/direct_install.sh"
export PATH="$HOME/.local/bin:$PATH"
incan --version
incan-lsp --version
```

After installation, create a project and run the normal first-contact loop:

```bash
incan new hello --yes
cd hello
incan run
incan test
incan build --release
```

## What you should do next

- Install the toolchain and create a starter project: [Getting Started](../tooling/tutorials/getting_started.md)
- Compare the runtime and deployment tradeoffs: [Incan vs JS/TS](../comparisons/javascript_typescript.md)
- If anything fails: [Troubleshooting](../tooling/how-to/troubleshooting.md)
- Set up your editor: [Editor setup](../tooling/how-to/editor_setup.md)
- Learn the basics: [The Incan Book (Basics)](../language/tutorials/book/index.md)
- Look up commands and JSON outputs: [CLI reference](../tooling/reference/cli_reference.md)
- Inspect compiler-owned project facts: [Codegraph inspection](../tooling/reference/codegraph_inspection.md)

## Mental model translations

- **Types are not erased at the authoring boundary**: Incan uses static types for source checking and then compiles through Rust, so the typed API surface is intended to support both humans and tooling before runtime.
- **Errors are values by default**: `Result`, `Option`, and `?` make fallible paths explicit instead of relying on JavaScript-style exceptions for normal control flow.
- **Packages can expose tooling facts**: diagnostics, build reports, generated Rust inspection, and codegraph export are public CLI surfaces rather than ad hoc logs.
- **Native output is the current deployment target**: Incan is not a JS runtime or a TypeScript transpiler; it is a native toolchain for new application code that should stay readable while compiling through the Rust ecosystem.

## Explanation

- [Why Incan?](../language/explanation/why_incan.md)
- [How Incan works](../language/explanation/how_incan_works.md)
- [Error handling](../language/explanation/error_handling.md)
- [Rust-shaped confidence](../language/explanation/rust_shaped_confidence.md)
