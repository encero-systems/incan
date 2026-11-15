Use an installed `0.6.0-dev.8` Oven executable and its matching toolchain archive. Installing an archive requires local Rust 1.98.0 with `rustc-dev` on a compatible host. The archive contains the compiler and its dependency closure; it does not contain the machine's Rust sysroot. Installing or packing a `.tar.zst` archive runs `tar` with `zstd`, so both must be on `PATH`; a minimal Linux image often has no `zstd`.

```bash
rustup toolchain install 1.98.0 --profile minimal --component rustc-dev
incan --version
incan toolchain install incan-0.6.0-dev.8.tar.zst
```

Set `INCAN_HOME` before installation to select a separate home. Keep it set when moving or copying that home to a different directory; all paths stored inside the packed toolchain are relative, while the machine-local Rust sysroot is rebound during installation. A toolchain built from the dev line ships its standard-library sources, so leave `INCAN_STDLIB` unset with it unless you work in a development checkout. The published `0.6.0-dev.8` archive ships no standard-library sources: every command run with it needs `INCAN_STDLIB` set to the `loaves/stdlib` directory of a checkout, and reports `no standard library` otherwise.

The archive does not install Oven's executable itself. Keep the matching executable on `PATH` as `incan`; installing an archive adds `oven` beside it. The 0.5 direct installer and package-manager channels install a different CLI; they are not a dev.8 archive installer.
