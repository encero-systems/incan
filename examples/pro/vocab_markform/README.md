# vocab_markform

Conformance example for an HTML/XML-shaped descriptor-gated embedded fragment (RFC 081, `#1023`).

This example is meant to be read from the Incan consumer surface first. The Rust companion crate exists to
describe the accepted `Markup` submode grammar to the compiler; it does not claim HTML or XML compatibility of any
kind.

The consumer writes:

```incan
def render_card(title: str, image_url: str) -> None:
    markup:
        <section class="card">
            <h1>{title}</h1>
            <!-- rendered by markform -->
            Terms &amp; conditions apply
            <img src={image_url} alt="Preview" />
        </section>
```

The important parts are:

- `from pub::markform ...` activates the vocab metadata shipped by the producer library.
- `markup:` is a library-defined block keyword, not a core Incan keyword.
- `<section class="card">...</section>` is an ordinary open/close element with a quoted attribute value.
- `{title}` is an expression hole: it re-enters ordinary Incan parsing and typechecks `title` as the real `str`
  parameter it is, not as opaque DSL text.
- `<!-- rendered by markform -->` is a comment, preserved verbatim.
- `&amp;` is an entity reference.
- `<img src={image_url} alt="Preview" />` is a self-closing element with both an expression-hole attribute value
  (`src`) and a literal attribute value (`alt`).
- Outside a `markup:` block, none of this is ordinary Incan syntax -- `<section>` parses (or fails to parse) as
  plain Incan expressions, never silently as markup.

## What this proves, and what it does not

This example proves the parser-to-typechecker-to-lowering contract: the fragment parses through the `Markup`
submode's dedicated grammar, its expression holes typecheck as real Incan expressions (`title` and `image_url`
resolve to `str`, exactly as they would anywhere else in the function), and the resulting typed
`EmbeddedFragmentExpr` artifact reaches Body IR successfully with its holes already lowered. It does **not**
register a desugarer or lowering hook, so it has no runtime meaning yet -- RFC 081 explicitly assigns that
responsibility to the owning DSL's own desugarer or lowering hook (`#1023`'s scope is the mechanism, not a
concrete HTML-rendering runtime). Building this consumer with `incan build` will therefore stop at Rust emission
with a clear, explicit refusal (`cannot emit Rust code for a descriptor-gated embedded fragment: no owning DSL
lowering hook is registered for it yet`) rather than silently emitting nothing or guessing at semantics. `incan
check` against the consumer proves the parser/typechecker contract without needing a lowering hook.

Files worth reading in order:

- `consumer/src/main.incn` - the user-facing DSL surface.
- `producer/incan.toml` - points the producer library at its vocab companion crate.
- `producer/vocab_companion/src/lib.rs` - registers the `markup:` block and its embedded-fragment descriptor.
