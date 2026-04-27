# Troubleshooting

This page collects common “it didn’t work” fixes when getting started with Incan from this repository.

## `incan: command not found` after `make install`

- `make install` installs `incan` into `~/.cargo/bin`.
- Ensure `~/.cargo/bin` is on your `PATH`.

Check:

```bash
ls -la ~/.cargo/bin/incan
echo "$PATH"
which incan || true
```

## Shell or editor is using an old `incan` / `incan-lsp`

This usually means your shell and editor are resolving different binaries. For local development from a clone, prefer:

```bash
cd /path/to/incan
make build
```

On local machines, `make build` builds the compiler and LSP with `cargo build --features lsp`, then links `~/.cargo/bin/incan` to `target/debug/incan` and `~/.cargo/bin/incan-lsp` to `target/debug/incan-lsp`. Keep `~/.cargo/bin` early enough in your `PATH` that both tools resolve there:

```bash
command -v incan
command -v incan-lsp
ls -l ~/.cargo/bin/incan ~/.cargo/bin/incan-lsp
incan --version
```

If `command -v incan` or `command -v incan-lsp` points somewhere unexpected, fix your `PATH` or remove the stale binary from the earlier location. If the `ls -l` target points at a different checkout, rerun `make build` from the checkout you want to use.

For VS Code/Cursor, also check the Incan settings:

```json
{
  "incan.lsp.path": "",
  "incan.compiler.path": ""
}
```

Leaving these empty makes the extension use workspace binary discovery or `PATH`. If you set `incan.lsp.path`, use a literal executable path such as `/path/to/incan/target/debug/incan-lsp`; the setting does not expand `$HOME`, `~`, or shell commands.

After changing paths or rebuilding, reload the editor window so it starts a new language-server process:

1. Run **Developer: Reload Window** from the command palette.
2. Reopen a `.incn` file.
3. Open **View → Output → Incan Language Server** and check the startup error or command-resolution message if diagnostics still look stale.

## I didn’t run `make install` (no-install fallback)

If you’re using the no-install fallback, run commands from the repository root and invoke:

```bash
./target/release/incan ...
```

If `./target/release/incan` does not exist yet, build it first:

```bash
make release
```

## Builds are slow the first time

The first `make release` (or first `incan build`) will compile Rust dependencies and can take a few minutes.

## Cargo needs internet access for dependencies

Some builds may download Rust crates via Cargo on first run. Ensure your environment can reach crates.io (or your configured
proxy/mirror).

## macOS: toolchain/linker issues

If you see errors about a missing C toolchain or linker, install Xcode Command Line Tools:

```bash
xcode-select --install
```

## Still stuck?

If you’re still stuck, please [open an issue](https://github.com/dannys-code-corner/incan/issues) and include:

- your OS and architecture
- the exact commands you ran
- the full error output
- `command -v incan`, `command -v incan-lsp`, and `ls -l ~/.cargo/bin/incan ~/.cargo/bin/incan-lsp`
- whether `incan.lsp.path` or `incan.compiler.path` is set in VS Code/Cursor
