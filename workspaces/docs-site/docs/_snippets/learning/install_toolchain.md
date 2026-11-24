Put the downloaded `incan` executable first on `PATH` and place its matching `toolchain.tar.zst` archive in your working directory. For a published release, place its matching `toolchain.tar.zst.sigstore.json` beside the archive. With `rustup` installed, install the pinned Rust compiler and then the Incan toolchain archive.

To install in a separate home, export `INCAN_HOME` with an absolute path before running these commands. Keep it set when using that installation, and update it if you move the home.

```bash
rustup toolchain install 1.98.0 --profile minimal --component rustc-dev
incan toolchain install ./toolchain.tar.zst
export PATH="${INCAN_HOME:-$HOME/.incan}/bin:$PATH"
incan --version
```

The archive contains the Incan compiler, standard library, and native dependencies. Installation binds the toolchain to the local Rust 1.98.0 sysroot. Leave `INCAN_STDLIB` unset when using the installed standard library.
