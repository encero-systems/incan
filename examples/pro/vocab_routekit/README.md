# vocab_routekit

Runnable pro-level example for route-shaped vocab surfaces.

This example shows:

- A block keyword with header arguments: `route "/health":`.
- A nested block-context clause: `middleware:`.
- A scoped operator-like glyph inside the route block: `get + post`.
- A scoped mapping glyph made from punctuation syntax: `get + post -> handler.index`.
- A Rust desugarer receiving the scoped glyph payload and lowering the block to ordinary statements.

Run it from the repository root:

```bash
./target/debug/incan build --lib examples/pro/vocab_routekit/producer/src/lib.incn
./target/debug/incan run examples/pro/vocab_routekit/consumer/src/main.incn
```
