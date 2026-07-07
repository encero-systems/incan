# Incan Toolchain npm Package

Incan is a statically typed, Pythonic programming language for writing clear high-level application code that compiles to native Rust. Learn more at [incan.io](https://incan.io).

This package provides `incan` and `incan-lsp` command shims for the Incan toolchain. npm installs a matching optional platform package, such as `@incan/toolchain-linux-x64`, that contains the prebuilt toolchain payload for the current host. The default install path does not run an npm lifecycle script.

```bash
npm install -g @incan/toolchain
incan --version
```

The command shims resolve the installed platform package and run its bundled commands directly. Supported npm hosts are Linux x64, macOS x64, and macOS arm64.

`install-incan` remains available for explicit installer flows that need a custom manifest, cache location, or archive override. The script-free `incan` and `incan-lsp` shims do not provision Rust during `npm install`; make sure `rustup`, `cargo`, `rustc`, and the `wasm32-wasip1` target are available before building projects.
