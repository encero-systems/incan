Put the `incan` executable on `PATH` and place its matching `toolchain.tar.zst` archive in your working directory. With `rustup` installed, install the pinned Rust compiler and then the Incan toolchain archive.

```bash
rustup toolchain install 1.98.0 --profile minimal --component rustc-dev
incan toolchain install ./toolchain.tar.zst
incan --version
```

The archive contains the Incan compiler, standard library, and native dependencies. Installation binds the toolchain to the local Rust 1.98.0 sysroot. Leave `INCAN_STDLIB` unset when using the installed standard library.

Set `INCAN_HOME` before installation to select a separate home. Use the same variable when running commands from that home, and update it if you move the home to a different directory.
