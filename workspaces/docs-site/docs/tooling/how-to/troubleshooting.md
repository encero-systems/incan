# Troubleshooting

This page collects common setup and first-run fixes for the installed SDK path. Contributor-only source-build notes are called out separately.

## `incan: command not found` after SDK install

The SDK installer links `incan` and `incan-lsp` into `INCAN_BIN_DIR`, defaulting to `~/.local/bin`. Make sure that directory is on your `PATH`:

```bash
export PATH="$HOME/.local/bin:$PATH"
command -v incan
incan --version
```

If you installed with a custom bin directory, use that path instead:

```bash
curl -fsSL https://github.com/dannys-code-corner/incan/releases/latest/download/install.sh | INCAN_BIN_DIR="$HOME/bin" sh
export PATH="$HOME/bin:$PATH"
```

## Shell or editor is using an old `incan` / `incan-lsp`

This usually means your shell and editor resolve different binaries. Check both commands:

```bash
command -v incan
command -v incan-lsp
incan --version
incan tools doctor
```

For VS Code/Cursor, also check the Incan settings:

```json
{
  "incan.lsp.path": "",
  "incan.compiler.path": ""
}
```

Leaving these empty makes the extension use workspace binary discovery or `PATH`. If you set `incan.lsp.path`, use a literal executable path such as `/Users/me/.local/bin/incan-lsp`; the setting does not expand `$HOME`, `~`, or shell commands. The extension warns when either path setting contains shell syntax, points at a missing file, or points at a non-executable file.

After changing paths or reinstalling, reload the editor window so it starts a new language-server process:

1. Run **Incan: Doctor** from the command palette and check the **Incan** output channel.
2. Run **Developer: Reload Window** from the command palette.
3. Reopen a `.incn` file.
4. Re-run **Incan: Doctor** if diagnostics still look stale.

## Contributor source builds

If you are working from a compiler checkout, use repository `make` targets instead of the SDK installer:

```bash
cd /path/to/incan
make build
```

On local machines, `make build` builds the compiler and LSP with `cargo build --features lsp`, then links `~/.cargo/bin/incan` to `target/debug/incan` and `~/.cargo/bin/incan-lsp` to `target/debug/incan-lsp`. Keep `~/.cargo/bin` early enough in your `PATH` that both tools resolve there.

If you intentionally want the release binary from a checkout:

```bash
make release
./target/release/incan --version
```

## Builds are slow the first time

The first `incan build`, `incan test`, or generated project run may compile Rust dependencies and can take a few minutes. Later runs should reuse Cargo artifacts unless dependency inputs, profile, target directory, or lock data changed.

## Cargo needs internet access for dependencies

Some builds may download Rust crates via Cargo on first run. Ensure your environment can reach crates.io or your configured proxy/mirror.

For restricted or offline environments, run the supported preflight before the build:

```bash
incan tools doctor
```

The doctor report includes offline-readiness diagnostics for local dependency inputs. Treat it as advisory: it can flag likely problems before Cargo runs, but it cannot guarantee that a later `incan build --frozen` or `incan test --frozen` will succeed.

Run once while online to populate Cargo's cache and generate `incan.lock`, then use:

```bash
incan build --frozen
incan test --frozen
```

Offline policy prevents fetching; it does not remove crate dependencies, so crates that are not already available to Cargo locally can still make an offline build fail. Use `--offline` when you want Cargo to fail instead of using the network without also requiring a lockfile. Use `--locked` when the lockfile must exist and match current dependency inputs.

## macOS: toolchain/linker issues

If you see errors about a missing C toolchain or linker, install Xcode Command Line Tools:

```bash
xcode-select --install
```

## Still stuck?

If you’re still stuck, please [open an issue](https://github.com/dannys-code-corner/incan/issues) and include your OS, architecture, exact commands, and full error output.
