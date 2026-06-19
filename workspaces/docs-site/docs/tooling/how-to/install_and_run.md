# Install and run Incan

This page documents the public 0.4 install path. Use the toolchain installer when you want to try Incan as a user. Use the source-build path only when you are contributing to the compiler or testing an unreleased branch.

## Supported hosts

The 0.4 toolchain installer ships archives for macOS arm64, macOS x86_64, and Linux x86_64. Native Windows and Linux arm64 are not part of the initial toolchain installer; use WSL2 or a source build for those hosts for now. Generated Rust projects still use the local Rust toolchain, so install Rust with `rustup` before running projects that build binaries.

The toolchain manifest also records the Rust backend policy for the release, including the `wasm32-wasip1` target used by packages that ship vocabulary companions.

## Install the toolchain

The canonical 0.4 artifact source is the GitHub Release. The release publishes `install.sh`, `manifest.json`, checksums, and platform toolchain archives; Homebrew, npm, and pip are thin adapters over that same manifest rather than separate compiler builds.

Use the direct installer when you want the release manifest, checksum verification, and command links without a package-manager adapter:

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
command -v incan-lsp
```

Package-manager installs use the same toolchain archive contract while fitting into the command manager you already use:

```bash
brew install https://github.com/dannys-code-corner/incan/releases/latest/download/incan.rb
npm install -g @incan/toolchain
pipx install incan
```

Use Homebrew when you want native macOS or Linux command management. Use npm when you want the toolchain command shims available through Node-based tooling, editors, or CI images. Use `pipx` for Python-oriented environments; plain `pip install --user incan` also works, but `pipx` keeps the command package isolated from project environments.

The npm and pip packages install the toolchain into a package-local cache on first install or first command use, then delegate to the real `incan` and `incan-lsp` binaries from the verified toolchain archive. Set `INCAN_TOOLCHAIN_MANIFEST` to pin a manifest, or use the direct `install.sh --manifest <URL|PATH>` path when you need fully explicit release control.

Rust users can also install from Git through Cargo, which compiles the release source instead of downloading a prebuilt toolchain archive:

```bash
cargo install --git https://github.com/dannys-code-corner/incan.git --tag v0.4.0 --locked --features lsp --bin incan --bin incan-lsp
```

Cargo installation compiles the compiler from source instead of downloading a prebuilt toolchain archive. The `lsp` feature enables the `incan-lsp` binary alongside the default `incan` CLI, so this is the right Cargo command when you want the same two commands exposed by the release installer. Prefer the direct installer, Homebrew, npm, or pipx when you want checksum-verified prebuilt release artifacts.

## Create a starter project

After installation, the shortest first run creates a starter project and exercises run, test, and release-build paths:

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

The source-build path links the compiler from the checkout and is useful for development. It is not the public first-contact path for evaluating a toolchain release.
