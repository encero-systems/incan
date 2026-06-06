# Install and run Incan

This page documents the public 0.4 install path. Use the SDK installer when you want to try Incan as a user. Use the source-build path only when you are contributing to the compiler or testing an unreleased branch.

## Supported hosts

The 0.4 SDK installer ships archives for macOS arm64, macOS x86_64, and Linux x86_64. Native Windows and Linux arm64 are not part of the initial SDK installer; use WSL2 or a source build for those hosts for now. Generated Rust projects still use the local Rust toolchain, so install Rust with `rustup` before running projects that build binaries.

The SDK manifest also records the Rust backend policy for the release, including the `wasm32-wasip1` target used by packages that ship vocabulary companions.

## Install the SDK

The canonical 0.4 artifact source is the GitHub Release. The release publishes `install.sh`, `manifest.json`, checksums, and platform SDK archives; Homebrew, npm, and pip are thin adapters over that same manifest rather than separate compiler builds.

The direct installer path is:

```bash
curl -fsSL https://github.com/dannys-code-corner/incan/releases/latest/download/install.sh | sh
```

For a dry run that resolves the manifest and target without writing files:

```bash
curl -fsSL https://github.com/dannys-code-corner/incan/releases/latest/download/install.sh | sh -s -- --dry-run
```

The installer reads the release manifest, selects the archive for your host target, verifies the archive checksum, installs into `INCAN_HOME` (default `~/.incan`), and links `incan` plus `incan-lsp` into `INCAN_BIN_DIR` (default `~/.local/bin`). Make sure the bin directory is on `PATH`.

```bash
export PATH="$HOME/.local/bin:$PATH"
incan --version
incan-lsp --version
```

Package-manager installs use the same SDK archive contract:

```bash
brew install https://github.com/dannys-code-corner/incan/releases/latest/download/incan.rb
npm install -g incan-sdk
pipx install incan-sdk
```

Use Homebrew when you want native macOS or Linux command management. Use npm when you want the SDK command shims available through Node-based tooling, editors, or CI images. Use `pipx` for Python-oriented environments; plain `pip install --user incan-sdk` also works, but `pipx` keeps the command package isolated from project environments.

The npm and pip packages install the SDK into a package-local cache on first install or first command use, then delegate to the real `incan` and `incan-lsp` binaries from the verified SDK archive. Set `INCAN_SDK_MANIFEST` to pin a manifest, or use the direct `install.sh --manifest <URL|PATH>` path when you need fully explicit release control.

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
