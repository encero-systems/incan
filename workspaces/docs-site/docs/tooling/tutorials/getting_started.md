# Getting started with Incan

This tutorial is the shortest public path from an installed toolchain to running, testing, and release-building a project. It does not require cloning the compiler repository.

<ol class="inc-step-rail" style="--inc-step-count: 5" aria-label="Getting started steps">
  <li><strong>Install</strong>Verify the toolchain</li>
  <li><strong>Create</strong>Scaffold a project</li>
  <li><strong>Run</strong>Execute the entry point</li>
  <li><strong>Test</strong>Check behavior</li>
  <li><strong>Build</strong>Produce a release binary</li>
</ol>

## Install and verify

Install the toolchain, then make sure the command is on `PATH`. The direct installer provisions stable Rust through `rustup` when needed and installs the `wasm32-wasip1` target used by package vocab companions:

```bash
--8<-- "_snippets/commands/direct_install.sh"
export PATH="$HOME/.local/bin:$PATH"
incan --version
```

You can also install through Homebrew, npm, or pipx; those package-manager channels use the same GitHub Release payloads as the shell installer. pipx routes through the shared installer and can provision Rust automatically. npm installs a host-specific optional platform package without running lifecycle scripts, and Homebrew installs the prebuilt command binaries from the Encero tap; both npm and Homebrew expect Rust to be managed separately.

```bash
brew tap encero-systems/tap
brew install incan
```

Native Windows and Linux arm64 are not supported by the initial toolchain installer. Use WSL2 or a source build for those hosts for now.

## Create your first project

Create a small starter project:

```bash
incan new hello --yes
cd hello
```

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

Run it:

```bash
incan run
```

<section class="inc-learning-panel inc-learning-panel--result" data-label="Result" markdown="1">

The starter prints its greeting from `src/main.incn`. At this point the manifest, source root, entry point, and Rust-backed build path are all connected.

</section>

Test it:

```bash
incan test
```

Build the release binary:

```bash
incan build --release
```

`incan build` already uses the release Cargo profile; `--release` is accepted so the first-contact command spells out the intent.

<section class="inc-learning-panel inc-learning-panel--complete inc-incus-slot" data-label="Complete" data-incus-category="success" markdown="1">

You now have a runnable project, a passing starter test, and a native release build. Continue with [Your first project](your_first_project.md) to split the starter into modules and add meaningful tests.

</section>

## What 0.4 is good for

0.4 is intended for trying Incan as an installed toolchain, creating small projects, running tests, checking diagnostics, inspecting generated artifacts, and evaluating how Incan fits into Rust-backed application tooling.

## What 0.4 is not yet good for

0.4 is not a Python compatibility runtime, a native Windows installer release, a full package registry, or a promise that generated Rust is a stable ABI. Generated Rust is inspectable current backend output; public compatibility should be based on Incan source, manifests, checked metadata, and documented CLI report schemas.

## Next steps

- [Your first project](your_first_project.md): split the starter project into modules and add real tests.
- [CLI reference](../reference/cli_reference.md): commands, flags, and machine-readable outputs.
- [Incan vs Python](../../comparisons/python.md): where Incan tries to win and where Python is still the better choice.
- [Incan vs Rust](../../comparisons/rust.md): why Incan compiles through Rust but does not replace Rust.
- [Encero stack](../../start_here/encero_stack.md): where Incan sits relative to InQL, Pallay, Omerus, Hees.ai, and Hees.io.
