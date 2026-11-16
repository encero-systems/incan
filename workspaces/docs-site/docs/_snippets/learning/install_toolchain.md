Install from the release bundle for your host. A bundle holds the `incan` command, the `incan-lsp` language server and the toolchain archive, `toolchain.tar.zst`; the [release page](https://github.com/encero-systems/incan/releases) names the bundle for each host. Installing needs Rust 1.98.0 with `rustc-dev` on a compatible host, and [ORAS](https://oras.land) 1.3 pulls the bundle. The archive contains the compiler, its dependency closure and the standard library; it does not contain the machine's Rust sysroot.

```bash
rustup toolchain install 1.98.0 --profile minimal --component rustc-dev
oras pull ghcr.io/encero-systems/loaves.pub/toolchain/incan:<bundle for your host> --output incan-bundle
chmod +x incan-bundle/incan incan-bundle/incan-lsp
incan-bundle/incan toolchain install incan-bundle/toolchain.tar.zst
cp incan-bundle/incan-lsp "$HOME/.incan/bin/"
export PATH="$HOME/.incan/bin:$PATH"
incan --version
```

Installation copies `incan` to `INCAN_HOME/bin` and adds `oven` beside it. `INCAN_HOME` is `~/.incan` unless you set it; set it before installation to select a separate home, and copy `incan-lsp` into that home's `bin` instead. Keep it set when moving or copying that home to a different directory: all paths stored inside the packed toolchain are relative, while the machine-local Rust sysroot is rebound during installation. Leave `INCAN_STDLIB` unset unless you work in a development checkout.
