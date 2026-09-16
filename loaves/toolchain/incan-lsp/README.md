# `incan-lsp`

Ring: **toolchain**

The Incan language server: the `incan-lsp` binary editors launch over stdio, and the `incan_lsp` library behind it — diagnostics, hover, go-to-definition and semantic tokens (including RFC 081 embedded-fragment ownership) computed by re-running the frontend on the open documents.

## Layout

- `loaves/toolchain/incan-lsp/src/main.rs` — the binary: `--version`/`--help` before the server starts, a multi-thread Tokio runtime with the compiler's deep worker stack, `tower-lsp` over stdin/stdout.
- `loaves/toolchain/incan-lsp/src/lib.rs` — `IncanLanguageServer` and the public modules.
- `loaves/toolchain/incan-lsp/src/backend.rs` — the protocol implementation and the Rust-inspect workspace it prepares through the driver's session, project and Cargo-policy services.
- `loaves/toolchain/incan-lsp/src/diagnostics.rs` — compiler diagnostics to LSP diagnostics.
- `loaves/toolchain/incan-lsp/src/semantic_tokens.rs` — token classification, including which bytes a DSL owns.
- `loaves/toolchain/incan-lsp/src/call_site_type_args.rs` — completion and hover helpers for call-site generic arguments.
- `loaves/toolchain/incan-lsp/tests/rfc081_embedded_conformance.rs` — the six embedded-fragment submodes carried from parse to emission, through both formatter modes and semantic highlighting.

## Depends on

The compiler ring (`incan_frontend`, `incan_driver`, `incan_provider`), the kernel, `oven_model`, and its own `tokio` (narrowed feature set) and `tower-lsp`. Never `incan-cli`: the server builds without the command line, and CI checks that with `cargo check -p incan-lsp --no-default-features --lib --bin incan-lsp`.

## Build and run

```bash
cargo build -p incan-lsp
incan-lsp --version
```

`make build` builds it beside the CLI and links `~/.cargo/bin/incan-lsp`; `make install-lsp` installs it through Cargo.
