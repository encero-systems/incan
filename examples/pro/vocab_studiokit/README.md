# vocab_studiokit

Runnable pro-level example for workflow-shaped vocab surfaces.

This example shows:

- A block keyword with header arguments: `workflow "daily":`.
- Scoped workflow glyphs assembled from existing tokens: `>>`, `//`, `:=`, and `===`.
- An Incan entry in `producer/src/vocab.incn` receiving structured glyph operands through `std.vocab` and returning an ordinary print statement.
- Query-like and step-like descriptor sketches alongside the exercised workflow block. They preserve the original sketch: a query references a consumer-supplied `__studiokit_query_result`, and a step emits `pass`. The workflow example summarizes syntax; it does not execute a workflow engine. Signature-head input transport is still unsupported by the current public bridge.

Run it from the repository root:

```bash
incan run examples/pro/vocab_studiokit/consumer/src/main.incn
```
