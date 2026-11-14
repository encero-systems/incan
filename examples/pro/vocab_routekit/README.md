# vocab_routekit

Runnable pro-level example for route-shaped vocab surfaces.

This example shows:

- A block keyword with header arguments: `route "/health":`.
- A nested block-context clause: `middleware:`.
- A scoped operator-like glyph inside the route block: `get + post`.
- A scoped mapping glyph made from punctuation syntax: `get + post -> handler.index`.
- An Incan entry in `producer/src/vocab.incn` receiving structured glyph operands through `std.vocab` and returning an ordinary print statement. The example reports verb and handler syntax; it does not start an HTTP server.

Run it from the repository root:

```bash
incan run examples/pro/vocab_routekit/consumer/src/main.incn
```
