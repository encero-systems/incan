# RFC 081: Language-shaped DSL embeddings

- **Status:** Draft
- **Created:** 2026-04-27
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 027 (`incan-vocab` block registration and desugaring)
    - RFC 040 (scoped DSL surface forms)
    - RFC 045 (scoped DSL symbol surfaces)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC captures the language-embedding design track for DSLs that need to look like established languages or language fragments: CSS, HTML, XML, Ruby, JavaScript, TypeScript, Java, Kotlin, Groovy, and similar surfaces. The goal is not to make those syntaxes part of ordinary Incan. The goal is to define when an explicit DSL block may opt into descriptor-gated token forms, lexical submodes, and language-shaped ambiguity rules without leaking them into core Incan parsing.

## Motivation

RFC 040 defines the base scoped-surface layer: scoped operator-like glyphs, binding-like glyphs, leading-dot expression forms, descriptor metadata, diagnostics, formatting, and desugaring handoff. That layer is sufficient for query-like blocks, workflow/application DSLs, and other purpose-built surfaces that mostly reuse ordinary Incan tokens.

Language-shaped DSLs need more than that. CSS needs selector tokens, declaration values, dimensions, colors, and custom properties. HTML and XML need markup submodes, attributes, raw text, entity-like references, comments, and expression holes. Ruby needs sigil identifiers, symbols, block parameter bars, regex and percent literal modes. JavaScript and TypeScript need optional access, strict equality, template literals, regex literals, comments, type-position syntax, and JSX/TSX-like markup if enabled. JVM-family surfaces add annotations, generic type syntax, lambdas, nullable/member-access forms, string interpolation, optional punctuation, closures, and regex operators.

Putting all of that in RFC 040 would collapse two compiler-layer concerns into one RFC. RFC 081 exists because language-shaped DSLs need lexical modes and token forms that build on RFC 040's base scoped-surface contract without redefining it.

## Goals

- Define how explicit DSL blocks may opt into language-shaped lexical modes and token forms.
- Keep language-shaped syntax descriptor-gated and position-scoped.
- Preserve core Incan tokenization and parsing outside eligible DSL positions.
- Define how embedded language fragments expose typed syntax artifacts to desugarers, formatters, diagnostics, and LSP tooling.
- Support narrow product-specific template/style fragments without requiring a full implementation of every target language.
- Provide a compatibility target for CSS, HTML, XML, Ruby, JavaScript, TypeScript, Java, Kotlin, Groovy, and similar language-shaped DSLs.

## Non-Goals

- Making CSS, HTML, XML, Ruby, JavaScript, TypeScript, Java, Kotlin, Groovy, or any other external language valid ordinary Incan syntax.
- Defining a universal parser generator for arbitrary languages.
- Guaranteeing source-compatible implementations of every external language grammar.
- Replacing RFC 040 scoped operator-like glyphs, binding-like glyphs, or leading-dot expression forms.
- Allowing libraries to mutate global lexical behavior through imports alone.
- Implementing this feature as part of the RFC 040 delivery slice.

## Guide-level explanation

A DSL author should be able to register a block whose body is parsed in a scoped lexical mode:

```incan
css:
    .card:hover > #title {
        --accent-color: #1166ff;
        color: var(--accent-color);
    }
```

Inside the `style` block, selector tokens, custom-property names, dimensions, colors, and declaration values may be meaningful to the DSL. Outside that block, `#1166ff`, `.card:hover`, and `--accent-color` are not ordinary Incan expression syntax.

Markup-shaped DSLs need a different mode:

```incan
html:
    <section class="card">
        <h1>{title}</h1>
        <img src={image_url} alt="Preview" />
    </section>
```

Here the DSL owns tags, attributes, text nodes, comments, entity-like references, and expression holes. The compiler should not pretend that `<section>` is just a chain of less-than and greater-than operators.

Some language-shaped DSLs mix expression and declaration syntax:

```incan
script:
    const name = user?.profile?.name ?? "Guest";
    const view = (items) => items.map((item) => `${item.id}:${item.name}`);
```

For these surfaces, a descriptor must say which lexical forms are enabled, which positions admit them, and what typed artifact the DSL receives.

## Reference-level explanation

A language-shaped DSL descriptor must name an owning block kind and one or more eligible positions within that block. The descriptor must not apply to ordinary Incan code outside those positions.

A descriptor may declare lexical submodes for markup, style rules, raw text, comments, regex literals, template strings, interpolation holes, type positions, selector positions, declaration values, and similar language-shaped regions.

A descriptor may declare token forms that are not ordinary Incan tokens, including custom-property names, dimensions, color literals, entity references, sigil identifiers, symbol literals, at-keywords, annotations, template-literal segments, regex literals, and namespace-qualified names.

A descriptor must define how each accepted token form or submode contributes to a typed syntax artifact. Later compiler phases must not rediscover the meaning of accepted surfaces by matching raw source text.

A descriptor must define the boundaries of expression holes when an embedded surface allows Incan expressions inside foreign-looking syntax. Expression holes must re-enter ordinary Incan parsing using an explicit delimiter or another unambiguous descriptor-owned boundary.

When a token spelling is valid both in ordinary Incan and in an embedded language-shaped DSL, the ordinary meaning must remain authoritative outside eligible DSL positions. Inside eligible positions, the innermost eligible descriptor owns the language-shaped interpretation.

If two same-depth descriptors claim the same token form or lexical submode in the same eligible position, the compiler must reject the combination as ambiguous unless this RFC or a successor RFC defines an explicit conflict-resolution rule.

## Design details

### Syntax

This RFC does not reserve global syntax. It reserves descriptor space for explicit DSL blocks that choose a language-shaped body or subposition.

### Semantics

Accepted embedded fragments are DSL-owned syntax artifacts. Their runtime meaning is supplied by the owning DSL's desugarer or lowering hook, not by core Incan evaluation.

### Interaction with existing features

RFC 040 remains the base model for scoped ownership, positive eligibility, misuse diagnostics, and descriptor identity. RFC 045 remains the home for scoped identifier-level meaning. This RFC extends those ideas to token forms and lexical submodes that cannot be modeled honestly as ordinary operator-like glyphs.

### Compatibility / migration

This RFC is additive. Code that does not import and use a DSL with language-shaped descriptors is unaffected.

## Alternatives considered

1. Put full language-shaped embedding into RFC 040. Rejected because it would make a useful scoped-surface RFC carry the cost of every language-shaped lexical submode before it can ship.

2. Require external files for all CSS, HTML, XML, or script-like content. Rejected as the only model because small embedded fragments are useful in application and template DSLs, and forcing every fragment into a sidecar file weakens locality.

3. Treat foreign-looking syntax as strings. Rejected as the only model because strings hide structure from diagnostics, formatting, LSP, desugaring, and policy.

## Drawbacks

- Descriptor-gated lexical modes increase parser, formatter, and LSP complexity.
- Embedded language fragments can make source files visually dense if overused.
- Partial language-shaped implementations may create user confusion if they look like a full external language but intentionally support only a subset.
- Tooling must make ownership visible enough that readers can tell where ordinary Incan ends and the DSL-owned surface begins.

## Layers affected

- **Parser / AST** — must support descriptor-gated token forms, lexical submodes, expression-hole re-entry, and typed embedded-fragment artifacts.
- **Typechecker / symbol resolution** — must keep embedded DSL ownership separate from ordinary Incan expression typing while still typechecking explicit Incan expression holes.
- **Lowering / IR emission** — must pass embedded artifacts to the owning DSL rather than lowering them as ordinary Incan syntax.
- **Formatter** — must format language-shaped fragments from structured artifacts or preserve source layout where the DSL declares itself layout-sensitive.
- **LSP / tooling** — must expose ownership, highlighting, hover, diagnostics, and completions across ordinary Incan and embedded submodes.
- **Docs / examples** — must clearly distinguish narrow product-specific fragments from full language-compatible embeddings.

## Unresolved questions

- Which embedded surfaces should be standardized first: CSS-like style fragments, HTML/XML-like markup fragments, or script/type-expression fragments?
- How much of a target language grammar may a DSL claim before it must identify itself as a partial subset?
- Should descriptor declarations name external language compatibility levels, or should every embedded surface be described only in Incan-owned terms?
- How should formatter fallback behave when a DSL provides tokenization but not a full formatter?
- What is the smallest template/style fragment needed before full language-shaped embedding work is planned?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
