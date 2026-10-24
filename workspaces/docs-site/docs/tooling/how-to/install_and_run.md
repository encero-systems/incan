# Install and run Incan

Use the `0.6.0-dev.8` Oven executable with a matching toolchain archive for your host. These instructions require the archive and executable to be available; they do not imply that the development release has been published.

## Install a toolchain archive

--8<-- "_snippets/learning/install_toolchain.md"

Installation verifies the driver's identity against the local sysroot before replacing an existing toolchain. A failed installation preserves the existing toolchain. Use a separate `INCAN_HOME` to test an archive without replacing your normal installation.

## Create and exercise a project

Create `loaf.toml`, `src/main.incn` and `tests/test_main.incn` as shown in [Getting started](../tutorials/getting_started.md).

--8<-- "_snippets/learning/first_project_loop.md"

## Use a development checkout

--8<-- "_snippets/learning/development_toolchain.md"

## Pack an installed toolchain

```bash
incan toolchain pack incan-0.6.0-dev.8.tar.zst
```

Packing includes the installed driver, Rust ABI caches, native dependencies, standard Loaves and registry closure. It rewrites paths for relocation and excludes the local sysroot. The destination extension selects tar, gzip or zstd compression.

## Continue

- [Project configuration](../reference/project_configuration.md)
- [Oven](../explanation/oven_alpha.md)
