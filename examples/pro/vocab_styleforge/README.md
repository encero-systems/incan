# vocab_styleforge

Conformance example for a CSS-shaped descriptor-gated embedded fragment (RFC 081, `#1023`).

This example is meant to be read from the Incan consumer surface first. The Rust companion crate exists to
describe the accepted `Style` and `SelectorDeclarationValue` submode grammars to the compiler; it does not claim
CSS compatibility of any kind.

The consumer writes:

```incan
def card_theme() -> None:
    style:
        .card:hover, #title {
            --accent-color: #1166ff;
            color: var(--accent-color);
            padding: 16px;
        }

def gutter() -> None:
    spacing:
        2rem
```

The important parts are:

- `from pub::styleforge ...` activates the vocab metadata shipped by the producer library.
- `style:` and `spacing:` are library-defined block keywords, not core Incan keywords.
- Inside `style:`, `.card:hover, #title` is accepted as a selector list, `--accent-color: #1166ff;` as a custom
  property declaration with a color literal, `var(--accent-color)` as a custom-property reference, and
  `padding: 16px;` as a dimension declaration -- exactly the constructs the `Style` submode enumerates, nothing
  more.
- `spacing:` claims the narrower `SelectorDeclarationValue` submode: a single bare declaration-value fragment
  (`2rem`) outside a full style block.
- Outside these two block bodies, `#1166ff`, `.card:hover`, and `--accent-color` are not ordinary Incan expression
  syntax -- they only mean something inside the descriptor's claimed position.

## What this proves, and what it does not

This example proves the parser-to-typechecker-to-lowering contract: the fragment parses through its own dedicated
submode grammar, produces a typed `EmbeddedFragmentExpr` artifact with real source spans, and that artifact reaches
Body IR successfully. It does **not** register a desugarer or lowering hook, so it has no runtime meaning yet --
RFC 081 explicitly assigns that responsibility to the owning DSL's own desugarer or lowering hook (`#1023`'s
scope is the mechanism, not a concrete CSS-in-Rust runtime). Building this consumer with `incan build` will
therefore stop at Rust emission with a clear, explicit refusal (`cannot emit Rust code for a descriptor-gated
embedded fragment: no owning DSL lowering hook is registered for it yet`) rather than silently emitting nothing or
guessing at semantics. `incan check` against the consumer proves the parser/typechecker contract without needing a
lowering hook.

Files worth reading in order:

- `consumer/src/main.incn` - the user-facing DSL surface.
- `producer/incan.toml` - points the producer library at its vocab companion crate.
- `producer/vocab_companion/src/lib.rs` - registers the `style:`/`spacing:` blocks and their embedded-fragment
  descriptors.
