# vocab_querykit

Runnable pro-level example for query-shaped vocab surfaces.

Read the Incan consumer first. `producer/src/vocab.incn` registers the surface and defines its native desugarer through `std.vocab`.

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

At build time, the compiler passes structured, independently spanned expressions through `std.vocab`:

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

The Incan desugarer walks those artifacts and returns an ordinary `print(...)` statement summarizing fields and pipes. It does not execute database queries. Shared syntax traversal lives in the ordinary Incan `vocab_tools` example library.

Files worth reading in order:

- `consumer/src/main.incn` - the user-facing DSL surface.
- `producer/loaf.toml` - declares `[vocab] entry`.
- `producer/src/vocab.incn` - registers scoped surfaces and returns structured Incan output.

This example covers:

- `library_vocab()` exporting a DSL block keyword.
- Leading-dot field paths inside a registered `query:` block.
- Leading-dot field paths inside registered query method arguments, such as `filter(...)` and `select(...)`.
- A query-owned pipeline glyph assembled from punctuation and operator tokens: `orders |> paid_orders`.
- An Incan desugarer receiving scoped-surface artifacts through `std.vocab`.

Run it from the repository root:

```bash
incan run examples/pro/vocab_querykit/consumer/src/main.incn
```
