# vocab_studiokit

Runnable pro-level example for workflow-shaped vocab surfaces.

This example shows:

- A block keyword with header arguments: `workflow "daily":`.
- Scoped workflow glyphs assembled from existing tokens: `>>`, `//`, `:=`, and `===`.
- A Rust desugarer receiving the scoped glyph payload and lowering the block to ordinary statements.
- A larger companion registration shape that also sketches query-like and step-like declarations for library authors.

Run it from the repository root:

```bash
./target/debug/incan build --lib examples/pro/vocab_studiokit/producer/src/lib.incn
./target/debug/incan run examples/pro/vocab_studiokit/consumer/src/main.incn
```
