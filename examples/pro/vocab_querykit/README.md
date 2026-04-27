# vocab_querykit

Runnable pro-level example for query-shaped vocab surfaces.

This example shows:

- `library_vocab()` exporting a DSL block keyword.
- Leading-dot field paths inside a registered `query:` block.
- Leading-dot field paths inside registered query method arguments, such as `filter(...)` and `select(...)`.
- A query-owned pipeline glyph assembled from punctuation and operator tokens: `orders |> paid_orders`.
- A Rust desugarer receiving scoped-surface artifacts through `incan_vocab` instead of reparsing source text.

Run it from the repository root:

```bash
./target/debug/incan build --lib examples/pro/vocab_querykit/producer/src/lib.incn
./target/debug/incan run examples/pro/vocab_querykit/consumer/src/main.incn
```
