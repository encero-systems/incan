# Embedded language fragments

A library you import can add block keywords whose bodies are not Incan. A markup library gives you `html:`, a styling library `css:`, a script-shaped library `pattern:`. Inside those blocks you write something that looks like another language:

```incan
from pub::webkit import render

def page(title: str) -> None:
    html:
        <h1>{title}</h1>
```

This page explains what that actually is, because the honest answer is narrower than it looks and the difference matters when you hit its edges.

## It is not the language it resembles

`<h1>{title}</h1>` is not HTML. Incan has a fixed catalogue of six **submodes** — small grammars owned by the compiler — and a library chooses which one its block claims. `Markup` happens to accept tags, attributes, text, entity references and comments, which covers a useful subset of HTML-shaped syntax and stops there. Namespaces, doctypes and unquoted attribute values are not accepted, and never will be as part of that submode.

The same applies to every other submode. Here is the whole catalogue, with what each one refuses:

| Submode | Accepts | Does not accept |
| --- | --- | --- |
| `Markup` | Open, close and self-closing tags; attributes written `name`, `name="literal"` or `name={expr}`; text runs; `&name;` entity references; `<!-- ... -->` comments; `{expr}` holes | Namespaces, doctypes, processing instructions, unquoted attribute values |
| `Style` | Comma-separated selector lists, captured as flat token runs rather than parsed any further; a `{ property: value; ... }` declaration block; `--custom-property` declarations; `/* ... */` comments | Nested rules, at-rules such as `@media`, any structure within a selector |
| `RawText` | Verbatim text interleaved with `{expr}` holes | Anything else — there is no structure to parse, by design |
| `RegexTemplate` | Exactly one of: a `/pattern/flags` regex literal, or a `` `...${expr}...` `` template string | Both forms in one fragment; regex flags outside ASCII letters |
| `SelectorDeclarationValue` | Exactly one value: a dimension like `16px`, a color like `#1166ff`, a `var(--name)` reference, an identifier, a string, a number, or an `{expr}` hole | More than one value; arithmetic between values |
| `TypePosition` | `Name`, qualified `a.b.Name`, generic `Name<Arg, ...>`, nullable `T?`, array `T[]`, union `A \| B` | Bounds, variance, wildcards, function types |

Read the "does not accept" column as final for that submode, not as a roadmap. Nothing in it is scheduled.

That boundary is deliberate rather than unfinished. Accepting a real external grammar would mean tracking a specification the compiler does not own, across versions it cannot pin, and would turn every gap into a compatibility bug. A fixed small grammar can be specified exactly and can say no clearly.

**A library that describes its blocks as "HTML support" or "CSS support" is overclaiming.** What it has is a submode, and the accepted constructs are listed in the [vocab authoring guide](../../contributing/how-to/authoring_vocab_crates.md#the-submode-catalog).

## Unrecognized syntax is an error, never a reinterpretation

Anything a submode does not accept is a parse error at the point it appears. It is not silently treated as text, not passed through to a runtime to reject later, and not reinterpreted as ordinary Incan.

This is the property that makes the feature usable. A fragment that compiles has been understood by the compiler, and a fragment that has not been understood does not compile.

## Holes are real Incan

`{title}` is an **expression hole**: an escape back into ordinary Incan, typechecked exactly as it would be anywhere else.

```incan
def page(title: str) -> None:
    html:
        <h1>{title}</h1>
        <p>{subtitle}</p>
```

If `subtitle` is not in scope, that is an ordinary unresolved-name error on that line — not something discovered later by the library's own machinery. A hole whose type does not fit its use is an ordinary type error. The fragment is foreign; what you interpolate into it is not.

## Blocks belong to the library that declared them

`html:` only means anything in a file that imports the library declaring it. Outside such a file, `html` is an ordinary identifier and `<h1>` is not valid syntax anywhere.

Two libraries cannot both claim the same block in the same place. If they try, the compiler rejects the combination as ambiguous rather than picking one, so which library you imported first never changes what your code means.

## What the compiler does not decide

The compiler parses a fragment, typechecks its holes, and hands the library a typed artifact. It assigns **no runtime meaning** to the fragment's own content — tag names, selectors, regex patterns and type shapes mean whatever the owning library decides they mean.

A library that has not yet supplied that step still gives you a fragment that parses and typechecks; only building it to a binary refuses, with a message naming exactly what is missing. That refusal is deliberate: guessing at what a DSL's syntax should do at runtime is precisely the kind of silent behavior this design exists to avoid.

## Formatting

`incan fmt` has exactly two behaviours inside a fragment, and the library picks which one applies to its blocks.

By default it reformats the fragment from the structure it parsed: elements, rules, declarations and values are laid out consistently, and the whitespace you happened to write between them is not preserved. Expression holes are formatted by the same code that formats that expression anywhere else.

A library can instead declare a block layout-sensitive, and then `incan fmt` reproduces the fragment's original text exactly. That is the right choice when the whitespace is content — indentation-significant templates, for instance. It is the library's declaration, not a per-file setting, so if you need it and your library has not declared it, that is a request to make to the library.

There is no third behaviour. A fragment is either reformatted from its structure or preserved verbatim; the compiler never falls back to leaving a fragment half-handled.

## What editors do inside a fragment

The language server draws the same ownership line the compiler does.

Inside an expression hole you get ordinary Incan tooling: hover, completions, signature help, go-to-definition and type errors all work exactly as they do outside a fragment, because a hole is ordinary Incan.

Everywhere else in the fragment — tag names, selectors, declaration properties, regex patterns, type shapes — you get an ownership hover naming the submode and the library whose descriptor claimed the block, and nothing else. In particular, a name there does not resolve against Incan scope, even when a variable in the same file happens to be spelled the same way. Answering a tag name with an unrelated local's type would be worse than answering nothing.

Diagnostics follow the same split. A construct the submode rejects is reported where you wrote it, in the submode's own terms; a mistake inside a hole is reported as the ordinary Incan error it is.

## See also

- [Authoring vocab crates](../../contributing/how-to/authoring_vocab_crates.md) — the library-author side, including the full submode catalogue
- [RFC 081](../../RFCs/closed/implemented/081_language_shaped_dsl_embeddings.md) — the specification
