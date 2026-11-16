# Install and run Incan

Install Incan 0.6 from the release bundle for your host, then create and exercise a project. The 0.5 installer, Homebrew, npm and pipx channels install Incan 0.5.1; [Move a project from 0.5 to 0.6](move_from_0_5.md) lists what changed.

## Install a toolchain archive

--8<-- "_snippets/learning/install_toolchain.md"

Installation first checks Rust 1.98.0 and `rustc-dev`, then verifies the driver's identity against the local sysroot before replacing an existing toolchain. A missing prerequisite reports the exact `rustup` command to run. A failed installation preserves the existing toolchain. Use a separate `INCAN_HOME` to test an archive without replacing your normal installation.

## Create and exercise a project

Run `oven new hello` to write a starter project with `loaf.toml`, `src/main.incn` and `tests/test_main.incn`, or create the files as shown in [Getting started](../tutorials/getting_started.md).

--8<-- "_snippets/learning/first_project_loop.md"

## Use a development checkout

--8<-- "_snippets/learning/development_toolchain.md"

## Pack an installed toolchain

```bash
incan toolchain pack incan-toolchain.tar.zst
```

Packing includes the installed driver, Rust ABI caches, native dependencies, standard Loaves and registry closure. It rewrites paths for relocation and excludes the local sysroot. The destination extension selects tar, gzip or zstd compression.

To relocate an installed home, copy the complete directory and set `INCAN_HOME` to the copy. Do not carry checkout-only `INCAN_STDLIB` or `INCAN_SOURCE_ROOT` values into the installed invocation.

## Continue

- [Project configuration](../reference/project_configuration.md)
- [Oven](../explanation/oven_alpha.md)
