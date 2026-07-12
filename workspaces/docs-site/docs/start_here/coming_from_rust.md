# Coming from Rust (evaluator)

This page routes Rust-first evaluators who want to understand where Incan keeps Rust-shaped semantics and where it trades surface syntax for application-code ergonomics.

<aside class="inc-bridge-note inc-bridge-note--rust inc-incus-slot" data-incus-category="rust" aria-label="Rust to Incan mental model">
  <span class="inc-eyebrow">Rust → Incan</span>
  <strong>Keep explicit fallibility, traits, and native compilation. Spend less surface syntax on application structure.</strong>
</aside>

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

<div class="inc-route-grid">
  <a class="inc-route-card" href="../../tooling/tutorials/getting_started/"><span class="inc-eyebrow">Quickstart</span><strong>Install and evaluate</strong><span>Use the binary toolchain or source-build path, then run the first project loop.</span></a>
  <a class="inc-route-card" href="../../language/explanation/rust_shaped_confidence/"><span class="inc-eyebrow">Semantics</span><strong>Rust-shaped confidence</strong><span>See which safety and explicitness properties Incan carries into a smaller application surface.</span></a>
  <a class="inc-route-card" href="../../language/how-to/rust_interop/"><span class="inc-eyebrow">Interop</span><strong>Cross the Rust boundary</strong><span>Import Rust crates and author explicit interop where native ecosystem reach matters.</span></a>
  <a class="inc-route-card" href="../../contributing/tutorials/book/"><span class="inc-eyebrow">Internals</span><strong>Contributor Book</strong><span>Follow the compiler pipeline, layering rules, tests, formatter, LSP, and docs loop.</span></a>
</div>

For the wider evaluation, read [Why Incan?](../language/explanation/why_incan.md), [Why not just Rust?](../language/explanation/why_not_just_rust.md), [How Incan works](../language/explanation/how_incan_works.md), [fallible paths](../language/tutorials/fallible_and_infallible_paths.md), [projects today](../tooling/explanation/projects_today.md), the [stability policy](../stability.md), [release notes](../release_notes/index.md), [RFC index](../RFCs/index.md), and [roadmap](../roadmap.md).

## What to look for

- Clear boundaries: what exists today vs roadmap (especially for WASM/frontend)
- “Stable vs experimental” labeling without forcing you to read RFCs first
- Rust-shaped `Result` composition: Incan keeps `map`, `map_err`, `and_then`, `or_else`, `inspect`, and `inspect_err` rather than adding Python-style aliases, with callable arguments documented as `Callable[...]`
