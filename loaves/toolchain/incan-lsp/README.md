# `incan-lsp`

Ring: **toolchain**

The Incan language server: the `incan-lsp` binary editors launch over stdio, and the `incan_lsp` library behind it — diagnostics, hover, go-to-definition and semantic tokens (including RFC 081 embedded-fragment ownership) computed by re-running the frontend on the open documents.

## Layout

- `loaves/toolchain/incan-lsp/src/main.rs` — the binary: `--version`/`--help` before the server starts, a multi-thread Tokio runtime with the compiler's deep worker stack, `tower-lsp` over stdin/stdout.
- `loaves/toolchain/incan-lsp/src/lib.rs` — `IncanLanguageServer` and the public modules.
- `loaves/toolchain/incan-lsp/src/backend.rs` — the protocol implementation and compilation sessions prepared from Oven's unit descriptions.
- `loaves/toolchain/incan-lsp/src/diagnostics.rs` — compiler diagnostics to LSP diagnostics.
- `loaves/toolchain/incan-lsp/src/semantic_tokens.rs` — token classification, including which bytes a DSL owns.
- `loaves/toolchain/incan-lsp/src/call_site_type_args.rs` — completion and hover helpers for call-site generic arguments.
- Embedded-fragment behavior is covered by contract cases; the former standalone conformance test was removed.

## Depends on

The dependency contract is declared in `loaves/toolchain/incan-lsp/loaf.toml`. The server uses `incan_frontend`, `incan_driver`, the kernel contracts, `tokio` and `tower-lsp`. Oven owns command parsing and build orchestration.

## Build and run

```bash
incan build loaves/toolchain/incan-lsp --profile release
incan-lsp --version
```

The executable is named `incan-lsp` in the Rust facet's release `bin` directory under the Loaf's `target` tree. Configure the editor to launch that binary over stdio.
