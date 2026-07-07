# Coming from Rust (evaluator)

This page routes Rust-first evaluators who want to understand where Incan keeps Rust-shaped semantics and where it trades surface syntax for application-code ergonomics.

## Install first

If you already use Cargo and want a source-built compiler, install the release source directly from Git with the LSP feature enabled so both `incan` and `incan-lsp` are installed:

```bash
cargo install --git https://github.com/encero-systems/incan.git --tag v0.4.0 --locked --features lsp --bin incan --bin incan-lsp
incan --version
incan-lsp --version
```

If you want the faster binary toolchain path instead, use the release installer. This path can also bootstrap the stable Rust backend through `rustup` on a fresh machine:

```bash
--8<-- "_snippets/commands/direct_install.sh"
export PATH="$HOME/.local/bin:$PATH"
incan --version
incan-lsp --version
```

After installation, create a project and run the normal first-contact loop:

```bash
incan new hello --yes
cd hello
incan run
incan test
incan build --release
```

## What you should do next

- Quickstart: [Getting Started](../tooling/tutorials/getting_started.md) (toolchain install, starter project, and source-build fallback for contributors)
- Explanation:
    - [Why Incan?](../language/explanation/why_incan.md)
    - [Why not just Rust?](../language/explanation/why_not_just_rust.md)
    - [Rust-shaped confidence](../language/explanation/rust_shaped_confidence.md)
    - [How Incan works](../language/explanation/how_incan_works.md)
- Interop: [Rust Interop](../language/how-to/rust_interop.md)
- Error handling: [Fallible and infallible paths](../language/tutorials/fallible_and_infallible_paths.md)
- Projects today: [Projects today](../tooling/explanation/projects_today.md)
- Reference surfaces:
    - [Language reference (generated)](../language/reference/language.md)
    - [CLI reference](../tooling/reference/cli_reference.md)
- Stability: [Stability policy](../stability.md) + [Release notes](../release_notes/index.md)
- Evolution surfaces:
    - [Contributing start here](../contributing/index.md)
    - [Incan Contributor Book (Advanced)](../contributing/tutorials/book/index.md)
    - [RFC index](../RFCs/index.md)
    - [Roadmap](../roadmap.md)

## What to look for

- Clear boundaries: what exists today vs roadmap (especially for WASM/frontend)
- “Stable vs experimental” labeling without forcing you to read RFCs first
- Rust-shaped `Result` composition: Incan keeps `map`, `map_err`, `and_then`, `or_else`, `inspect`, and `inspect_err` rather than adding Python-style aliases, with callable arguments documented as `Callable[...]`
