Use an installed `0.6.0-dev.8` Oven executable and its matching toolchain archive. Installing an archive requires local Rust 1.98.0 with `rustc-dev` and a compatible host. The archive contains the compiler and its dependency closure; it does not contain the machine's Rust sysroot.

```bash
incan --version
incan toolchain install incan-0.6.0-dev.8.tar.zst
```

Set `INCAN_HOME` before installation to select a separate home. Set `INCAN_STDLIB` to the matching standard-library source directory containing the package directories with `loaf.toml`. The selected toolchain must contain their compiled artifacts for the profile you use.

The archive does not install Oven's executable itself. Keep the matching `incan` on `PATH`. The 0.5 direct installer and package-manager channels install a different CLI; they are not a dev.8 archive installer.
