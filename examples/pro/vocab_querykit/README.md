# vocab_querykit

Runnable pro-level example for query-shaped vocab surfaces.

This example is meant to be read from the Incan consumer surface first. The Rust companion crate exists to describe that surface to the compiler, but the feature is about letting a library author publish a small DSL that consumers can use after importing the library.

The consumer writes:

```incan
from pub::querykit import querykit_name

def main() -> None:
    let name = querykit_name()
    print(name)

    query:
        .amount > 100
        .customer_id
        orders |> paid_orders
        orders.filter(.status == "paid").select(.region)
```

The important parts are:

- `from pub::querykit ...` activates the vocab metadata shipped by the producer library.
- `query:` is a library-defined block keyword, not a core Incan keyword.
- `.amount`, `.customer_id`, `.status`, and `.region` are leading-dot field paths. They are valid only where the query library registered them.
- `orders |> paid_orders` uses a query-owned pipeline glyph. It does not mean every `orders` value globally supports a `|>` operator.
- `filter(.status == "paid")` and `select(.region)` prove scoped surfaces also work inside registered method-call argument positions, not only directly in a DSL block body.

At build time, the compiler does not ask the desugarer to re-read the source string. It passes typed scoped-surface artifacts through `incan_vocab`:

```text
.amount
  descriptor: query.field
  payload: leading-dot path ["amount"]
  owner: query block

orders |> paid_orders
  descriptor: query.pipe
  payload: scoped glyph "|>" with left/right operands
  owner: query block

.status inside filter(...)
  descriptor: query.method_field
  payload: leading-dot path ["status"]
  owner: query method argument
```

The example desugarer walks those typed artifacts and lowers the block to a visible `print(...)` statement. Running the consumer proves that the producer library, vocab companion, manifest metadata, parser, desugarer handoff, and runtime execution all line up.

Files worth reading in order:

- `consumer/src/main.incn` - the user-facing DSL surface.
- `producer/incan.toml` - points the producer library at its vocab companion crate.
- `producer/vocab_companion/src/lib.rs` - registers the `query:` block and scoped surfaces.
- `producer/vocab_companion/src/desugar.rs` - consumes typed scoped-surface artifacts and emits ordinary Incan AST.

This example covers:

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
