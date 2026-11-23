# Install and run Incan

Use an Incan 0.6 executable with a toolchain archive from the same build for your host.

## Install a toolchain archive

--8<-- "_snippets/learning/install_toolchain.md"

Installation first checks Rust 1.98.0 and `rustc-dev`, then verifies the driver's identity against the local sysroot before replacing an existing toolchain. A missing prerequisite reports the exact `rustup` command to run. A failed installation preserves the existing toolchain. Use a separate `INCAN_HOME` to test an archive without replacing your normal installation.

## Create and exercise a project

Create `loaf.toml`, `src/main.incn` and `tests/test_main.incn` as shown in [Getting started](../tutorials/getting_started.md).

--8<-- "_snippets/learning/first_project_loop.md"

## Use a development checkout

--8<-- "_snippets/learning/development_toolchain.md"

## Pack an installed toolchain

```bash
incan toolchain pack toolchain.tar.zst
```

Packing includes the installed driver, Rust ABI caches, native dependencies, standard Loaves and registry closure. It rewrites paths for relocation and excludes the local sysroot. The destination extension selects tar, gzip or zstd compression.

To relocate an installed home, copy the complete directory and set `INCAN_HOME` to the copy. Do not carry checkout-only `INCAN_STDLIB` or `INCAN_SOURCE_ROOT` values into the installed invocation.

## Continue

- [Project configuration](../reference/project_configuration.md)
- [Oven](../explanation/oven_alpha.md)
