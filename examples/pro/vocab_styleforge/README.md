# vocab_styleforge

Conformance example for a CSS-shaped descriptor-gated embedded fragment (RFC 081, `#1023`).

Read the Incan consumer first. Its Incan vocabulary entry registers the `Style` and `SelectorDeclarationValue` submodes; it does not claim CSS compatibility.

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
`incan check` proves its parser/typechecker contract. Running or building the consumer is refused with `unsupported source construct: embedded DSL fragment expression`. The consumer is listed in `examples/check-only.txt`; the examples runner checks it.

Files worth reading in order:

- `consumer/src/main.incn` - the user-facing DSL surface.
- `producer/loaf.toml` - declares `[vocab] entry`.
- `producer/src/vocab.incn` - registers the `style:` and `spacing:` fragment descriptors through `std.vocab`.
