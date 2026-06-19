# Coming from TypeScript or JavaScript

This page is a routing guide for TypeScript and JavaScript developers evaluating Incan for application code, command-line tools, services, and typed domain packages.

## Install first

If you already use Node-based tooling, install the npm adapter. It installs command shims and resolves the same verified Incan toolchain archive used by the direct installer:

```bash
npm install -g @incan/toolchain
incan --version
command -v incan-lsp
```

The direct installer is the same release path without npm in the middle, which is useful for shell scripts, CI images, and environments where you want explicit control over the toolchain manifest:

```bash
curl -fsSL https://github.com/dannys-code-corner/incan/releases/latest/download/install.sh | sh
export PATH="$HOME/.local/bin:$PATH"
incan --version
command -v incan-lsp
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
