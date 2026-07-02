# Incan Toolchain npm Installer

Incan is a statically typed, Pythonic programming language for writing clear high-level application code that compiles to native Rust. Learn more at [incan.io](https://incan.io).

This package is a thin installer and command shim for the Incan toolchain. It installs verified toolchain archives from the shared Incan release manifest instead of building the compiler from npm packaging.

```bash
npm install -g @incan/toolchain
incan --version
```

The command shims install the toolchain into a package-local cache on first use and default to the release manifest that matches this package version. Set `INCAN_NPM_TOOLCHAIN_HOME`, `INCAN_NPM_BIN_DIR`, or `INCAN_TOOLCHAIN_MANIFEST` when you need a custom cache location or manifest.
