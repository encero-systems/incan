# Install and run Incan

This page documents the public 0.4 install path. Use the SDK installer when you want to try Incan as a user. Use the source-build path only when you are contributing to the compiler or testing an unreleased branch.

## Supported hosts

The 0.4 SDK installer targets macOS and Linux first. Native Windows is not part of the initial SDK installer; use WSL2 for now. Generated Rust projects still use the local Rust toolchain, so install Rust with `rustup` before running projects that build binaries.

The SDK manifest also records the Rust backend policy for the release, including the `wasm32-wasip1` target used by packages that ship vocabulary companions.

## Install the SDK

The hosted installer path is:

```bash
curl -fsSL https://incan.pub/install.sh | sh
```

For a dry run that resolves the manifest and target without writing files:

```bash
curl -fsSL https://incan.pub/install.sh | sh -s -- --dry-run
```

The installer reads the release manifest, selects the archive for your host target, verifies the archive checksum, installs into `INCAN_HOME` (default `~/.incan`), and links `incan` plus `incan-lsp` into `INCAN_BIN_DIR` (default `~/.local/bin`). Make sure the bin directory is on `PATH`.

```bash
export PATH="$HOME/.local/bin:$PATH"
incan --version
incan-lsp --version
```

## Create a starter project

After installation, the shortest first run is:

```bash
incan new hello --yes
cd hello
incan run
incan test
incan build --release
```

`incan new` creates an `incan.toml`, `src/main.incn`, `tests/test_main.incn`, `README.md`, and `.gitignore`. The generated project is intentionally small: one function, one entrypoint, and one test that checks the generated behavior.

## Source-build fallback for contributors

If you are working on Incan itself, build from the repository instead:

```bash
git clone https://github.com/dannys-code-corner/incan.git
cd incan
make install
incan run examples/simple/hello.incn
```

The source-build path links the compiler from the checkout and is useful for development. It is not the public first-contact path for evaluating an SDK release.
