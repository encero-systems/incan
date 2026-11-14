# Install and run Incan

Install a release for your host with its installer, or install a toolchain archive with its matching Oven executable. A development line is installed by its tag; a tag whose release files are not published reports the file it could not download.

## Install a release

--8<-- "_snippets/learning/install_toolchain.md"

Installation first checks Rust 1.98.0 and `rustc-dev`, then verifies the driver's identity against the local sysroot before replacing an existing toolchain. A missing prerequisite reports the exact `rustup` command to run. A failed installation preserves the existing toolchain. Use a separate `INCAN_HOME` to test a release without replacing your normal installation.

## Release files

A release is one directory of files. The installer reads these from it, with the host's target triple as `TARGET`:

| File | Contents |
| --- | --- |
| `SHA256SUMS` | One `SHA256  NAME` line for each file below. |
| `incan-TARGET` | The Oven executable, which answers to `incan` and `oven`. |
| `incan-lsp-TARGET` | The language server. |
| `toolchain-TARGET.tar.zst` | The toolchain archive. |

## Install an archive without the installer

Run a release's Oven executable on its archive:

```bash
chmod +x incan-aarch64-apple-darwin
./incan-aarch64-apple-darwin toolchain install toolchain-aarch64-apple-darwin.tar.zst
```

This copies the executable to `INCAN_HOME/bin/incan` and links `INCAN_HOME/bin/oven` to it. Put `INCAN_HOME/bin` on `PATH`. The language server is not part of the archive; copy `incan-lsp-TARGET` to `INCAN_HOME/bin/incan-lsp` yourself.

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
