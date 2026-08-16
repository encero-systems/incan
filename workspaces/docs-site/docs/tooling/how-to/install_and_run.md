# Install and run Incan

The latest stable toolchain is Incan 0.4. The tutorials marked as 0.5 development docs require a compiler built from the matching repository checkout. Keep those two paths distinct so an example never silently depends on a newer language surface.

## Supported hosts

The stable binary installer ships archives for macOS arm64, macOS x86_64, and Linux x86_64. Native Windows and Linux arm64 are not supported by the current binary installer; use WSL2 or a source build on those hosts.

The release manifest records the compiler archive, checksum, Rust backend policy, and extra targets needed by the toolchain. The direct installer and package-manager adapters consume the same release payload rather than publishing separate compiler builds.

## Install the stable toolchain

--8<-- "_snippets/learning/install_toolchain.md"

Rust users can instead compile the stable release from source:

```bash
cargo install --git https://github.com/encero-systems/incan.git --tag v0.4.0 --locked --features lsp --bin incan --bin incan-lsp
```

This Cargo command builds the compiler and language server; it is not the normal project build path documented for the 0.5 development line.

## Create and exercise a project

--8<-- "_snippets/learning/first_project_loop.md"

`incan new` creates `incan.toml`, `src/main.incn`, `tests/test_main.incn`, `README.md`, and `.gitignore`. The starter is deliberately small so the first run proves the complete project loop without hiding the generated files.

## Follow the 0.5 development docs

--8<-- "_snippets/learning/development_toolchain.md"

The 0.5 compiler uses the Oven-managed build path: project dependencies resolve into receipt-compatible Loaf state, generated Rust stays inspectable, and ordinary `incan build`, `run`, and `test` invoke `rustc` without a Cargo fallback. Cargo remains an internal publishing and compatibility boundary rather than the authority for normal project builds.

## Continue

- [Getting Started](../tutorials/getting_started.md) for a guided first-contact loop.
- [Choose a learning route](../../start_here/index.md) when you know the outcome you want.
- [Oven alpha](../explanation/oven_alpha.md) for the 0.5 build and package-system contract.
