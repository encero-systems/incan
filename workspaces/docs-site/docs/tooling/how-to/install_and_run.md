# Install and run Incan

This page documents the public 0.4 install path. Use the toolchain installer when you want to try Incan as a user. Use the source-build path only when you are contributing to the compiler or testing an unreleased branch.

## Supported hosts

The 0.4 toolchain installer ships archives for macOS arm64, macOS x86_64, and Linux x86_64. Native Windows and Linux arm64 are not part of the initial toolchain installer; use WSL2 or a source build for those hosts for now. Generated Rust projects use Cargo under the hood, so the direct installer provisions stable Rust through `rustup` when needed and ensures the `wasm32-wasip1` target is installed for packages that ship vocabulary companions.

The toolchain manifest records the Rust backend policy for the release, including the Rust channel and extra targets the installer must make available.

## Install the toolchain

The canonical 0.4 artifact source is the GitHub Release. The release publishes `install.sh`, `manifest.json`, checksums, and platform toolchain archives; Homebrew, npm, and pip are thin adapters over that same manifest rather than separate compiler builds.

Use the direct installer when you want the release manifest, checksum verification, and command links without a package-manager adapter:

```bash
--8<-- "_snippets/commands/direct_install.sh"
```

For a dry run that resolves the manifest and target without writing files:

```bash
--8<-- "_snippets/commands/direct_install_dry_run.sh"
```

The installer reads the release manifest, selects the archive for your host target, provisions the Rust backend unless you pass `--skip-rust`, verifies the archive checksum, installs into `INCAN_HOME` (default `~/.incan`), and links `incan` plus `incan-lsp` into `INCAN_BIN_DIR` (default `~/.local/bin`). Make sure the bin directory is on `PATH`.

```bash
export PATH="$HOME/.local/bin:$PATH"
incan --version
incan-lsp --version
```

Package-manager installs use the same release archive payloads while fitting into the command manager you already use:

```bash
brew tap encero-systems/tap
brew install incan
npm install -g @incan/toolchain
pipx install incan
```

Use Homebrew when you want native macOS or Linux command management and already have Rust managed separately. The generated release formula is published through the Encero tap, while the release asset remains the source used to update that formula. Use npm when you want the toolchain command shims available through Node-based tooling, editors, or CI images without an install-time lifecycle script. Use `pipx` for Python-oriented environments; plain `pip install --user incan` also works, but `pipx` keeps the command package isolated from project environments.

The npm package installs `@incan/toolchain` plus a host-specific optional platform package and then runs the packaged `incan` and `incan-lsp` binaries directly. It does not run `postinstall`, download archives during npm install, or provision Rust automatically. Make sure `rustup`, `cargo`, `rustc`, and `wasm32-wasip1` are available before building projects from an npm-installed toolchain.

The direct installer and pipx package route through the shared installer, so they can provision Rust unless you pass `--skip-rust` through `install-incan` or set `INCAN_SKIP_RUST_INSTALL=1`. Set `INCAN_TOOLCHAIN_MANIFEST` to pin a manifest, or use the direct `install.sh --manifest <URL|PATH>` path when you need fully explicit release control.

Rust users can also install from Git through Cargo, which compiles the release source instead of downloading a prebuilt toolchain archive:

```bash
cargo install --git https://github.com/encero-systems/incan.git --tag v0.4.0 --locked --features lsp --bin incan --bin incan-lsp
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
git clone https://github.com/encero-systems/incan.git
cd incan
make install
incan run examples/simple/hello.incn
```

The source-build path links the compiler from the checkout and is useful for development. It is not the public first-contact path for evaluating a toolchain release.
