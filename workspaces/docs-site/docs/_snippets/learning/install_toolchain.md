Install Rust 1.98.0 with `rustc-dev` through `rustup`, then run the release's installer. The installer needs `curl`, `tar`, and `shasum` or `sha256sum`.

```bash
rustup toolchain install 1.98.0 --profile minimal --component rustc-dev
curl -fsSL https://github.com/encero-systems/incan/releases/latest/download/install-incan.sh | bash
export PATH="$HOME/.incan/bin:$PATH"
incan --version
```

The installer downloads three files for the host from the release: the Oven executable, the language server and the toolchain archive. It checks each against the release's `SHA256SUMS`, then runs the downloaded executable's `incan toolchain install` on the archive. It checks checksums only; it does not verify a release signature.

`incan`, `oven` and `incan-lsp` land in `INCAN_HOME/bin`, which is `~/.incan/bin` by default. The installer prints the `PATH` line to add when that directory is not on `PATH`, and the `INCAN_HOME` line when the home is not `~/.incan`.

The installer does not install or change Rust. A missing Rust 1.98.0 or `rustc-dev` stops the installation and names the `rustup` command above. The archive contains the compiler and its dependency closure; it does not contain the machine's Rust sysroot.

| Option | Effect |
| --- | --- |
| `--version TAG` | Installs the release with that tag, such as a development line, instead of the latest release. |
| `--release URL_OR_DIRECTORY` | Reads the release files from a mirror or from disk. `INCAN_RELEASE` sets the same value. |
| `--incan-home PATH` | Installs into that home. `INCAN_HOME` sets the same value. |
| `--target TRIPLE` | Installs that host target's files instead of the detected one. |
| `--dry-run` | Prints what would be read and where it would go, and writes nothing. |

Pass options after `bash -s --`, as in `curl -fsSL URL | bash -s -- --version 0.6.0-dev.9`. Releases are built for `aarch64-apple-darwin` and `x86_64-unknown-linux-gnu`.

A home that already holds an `incan` is upgraded as a whole: the earlier `incan` and `incan-lsp` are put back when the new toolchain is refused.

Keep `INCAN_HOME` set when using a home other than `~/.incan`, and when moving or copying a home to a different directory; all paths stored inside the packed toolchain are relative, while the machine-local Rust sysroot is rebound during installation. Set `INCAN_STDLIB` only for development checkouts; an installed archive uses its shipped standard-library sources.

The 0.5 installer, Homebrew, npm and pipx channels install the 0.5 CLI; they do not install this toolchain.
