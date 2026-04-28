# RFC 040: Scoped DSL Surface Forms

- **Status:** Implemented
- **Created:** 2026-03-08
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 027 (`incan-vocab` block registration and desugaring)
    - RFC 028 (global operator overloading)
    - RFC 045 (scoped DSL symbol surfaces — companion RFC for identifier-level scoping)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/174
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** v0.3

## Summary

Introduce **scoped DSL surface forms** for explicit DSL blocks and their registered subgrammars: a registered DSL may give a local syntax shape positive meaning inside eligible positions of an owning block, while preserving negative misuse diagnostics outside that block in the activating file or module. Operator-like glyphs such as `>>`, `<<`, `|>`, `<|`, `->`, `<-`, `//`, `===`, or `+` can support chaining, routing, linking, comparison, matching, or composition within a DSL block without implying that the operand types globally implement the corresponding RFC 028 operator traits. Binding-like glyphs such as `:=` are reserved for block-local alias/binding forms and are specified as a separate family from operators. Expression-form surfaces such as leading-dot paths (`.column`, `.order.amount`) provide implicit-receiver syntax that is valid only inside eligible DSL positions, without becoming a general Incan expression form.

This RFC does **not** define a closed whitelist of legal DSL surfaces. It defines the descriptor contract, scoping rules, parsing-eligibility rules, semantic artifact requirements, diagnostic rules, and conflict policy that make scoped surfaces safe to use. The surface set is open-ended as long as a DSL registers the shape explicitly and it coexists cleanly with the core grammar and tooling.

RFC 040 defines the base scoped-surface layer for product DSLs. At minimum, it must be enough for query-like blocks and method arguments, site/application composition blocks, assistant/workflow surfaces, and app-builder template/style entry points that fit descriptor-gated surface forms. It owns descriptor contracts for scoped glyphs and expression-form surfaces. Full language-shaped embeddings such as CSS, HTML, XML, Ruby, JavaScript, TypeScript, Java, Kotlin, and Groovy belong to RFC 081 (language-shaped DSL embeddings), because they require lexical modes and token forms beyond RFC 040's base surface model. Narrow template or style surfaces may still be specified against RFC 040 when they can be modeled as constrained DSL surfaces.

## Motivation

### Global operator overloading is not the right model for every DSL

RFC 028 defines ordinary global operator overloading. That is the right tool when a type truly supports an operator everywhere:

- a matrix type can globally support `@`
- a custom numeric type can globally support `+`
- a pipeline object can globally support `>>`

But some DSLs need the same glyphs with a meaning that exists **only inside an explicit block and specific DSL positions within it**.

Example:

```incan
pipeline user_sync:
    extract >> normalize >> validate >> store
```

The intent here is not necessarily that `Step` globally implements `Shr`. The real meaning may be "inside a `pipeline` block, register directed links between adjacent steps." Outside that block, `extract >> normalize` should be an error with a targeted message.

If the types globally implement RFC 028 operator traits, the compiler can no longer honestly say "this is only valid inside a pipeline block." That is why block-local surface meaning needs its own mechanism.

### RFC 027 already provides the right substrate, but not the full glyph model

RFC 027 gives Incan an explicit DSL block model:

- libraries register block keywords and placement rules
- block structure remains explicit and available to later compilation phases
- DSL blocks can contribute library-owned surface meaning without mutating ordinary language semantics

That is already the correct architectural home for DSL-owned glyphs. What is missing is a way for a block to say "inside these positions, this glyph has a local meaning."

### DSLs need concise chaining and concise naming

The immediate motivating cases are operator-like glyphs:

- `>>` / `<<` for directional linking
- `|>` / `<|` for pipe-style flow or reverse application inside a block
- `->` / `<-` for transitions, edges, mappings, or directional flow inside a block
- `//` for DSL-local fallback, layering, path-like composition, or rule-combination forms without changing ordinary floor-division semantics
- `===` for DSL-local exact, identity, or shape-equality predicates without making strict equality a global Incan operator

But the same family also needs room for binding-like surfaces such as `:=`, where the glyph is not a global walrus operator, but a block-local alias or slot-binding form.

Beyond operators and bindings, DSLs also need **expression-form surfaces**: syntax shapes that are only valid inside eligible DSL positions. The leading example is `.column` notation — a leading-dot path with an implicit receiver supplied by the owning block's context. Outside DSL positions, `.field` at expression start is not valid Incan syntax and must be rejected. Inside a query or relational block, `.amount` means "the `amount` field of the primary relation" without needing a named receiver.

This RFC therefore covers **scoped DSL surface forms** as the parent category, split into operator-like glyph, binding-like glyph, and expression-form families.

### The real restriction is not the glyph, but the contract

The language does not need to pre-decide that only a handful of symbols are ever valid in DSLs. A DSL may reasonably want shapes like:

```incan
api app:
    route get + delete -> (...)
```

where `+` combines route verbs and `->` maps a route specification to a handler or body.

That is fine. The thing the language must restrict is not *which* glyphs are imaginable, but *how* a DSL claims them:

- the glyph must be explicitly registered
- the DSL must declare the positions where it becomes special
- the DSL must declare its family and surface shape
- conflicts with core grammar must be explicit and tooling-safe

That is the real safety boundary for this RFC.

## Goals

- Define the registration, scoping, parsing-eligibility, semantic artifact, diagnostic, and conflict rules under which explicit DSL blocks may own position-scoped surface forms.
- Cover three surface families: operator-like glyphs (e.g. `>>`, `|>`, `//`, `===`), binding-like glyphs (e.g. `:=`), and expression-form surfaces (e.g. leading-dot `.field` access).
- Ensure scoped surface meaning is block-local and position-scoped: a surface gains DSL-owned semantics only inside eligible positions of an explicit owning block, not as a blanket redefinition for the whole block body.
- Require accepted scoped-surface occurrences to carry descriptor identity, owning block identity, eligible-position identity, parsed payload, source span, and lowering/tooling handoff metadata through later phases.
- Provide targeted misuse diagnostics when DSL-shaped surfaces appear outside eligible positions but inside an activating file or module.
- Coexist cleanly with RFC 028 global operator overloading and the rest of the core grammar.
- Keep the surface set open-ended within RFC 040's base layer: core-token glyph reuse, leading-dot expression forms, and a descriptor path for selected registered symbolic glyphs.

## Non-goals

- Arbitrary ad-hoc punctuation with no registration or conflict policy
- Project-wide or import-only operator redefinition
- Hidden ambient runtime state as the source of block-local meaning
- A global walrus operator for ordinary Incan code
- Scoped identifier symbol semantics (e.g. `sum`, `count` gaining DSL-specific meaning by position) — that belongs to RFC 045
- One-off parser exceptions for individual libraries without a registered descriptor
- General-purpose language embeddings for CSS, HTML, XML, Ruby, JavaScript, TypeScript, Java, Kotlin, Groovy, or similar languages. These belong to RFC 081, not RFC 040.

## Guide-level explanation

### Same spelling, separate semantic namespace

The central rule is:

- the **surface spelling** may be the same
- the **owner of the meaning** is different

Outside an owning DSL block, or outside the DSL positions that block explicitly marks as eligible, the surface falls back to the ordinary language surface and is interpreted according to RFC 028 and the rest of the core grammar.

Inside an explicit DSL block, the enclosing block may own a position-scoped meaning for the same surface spelling.

That means:

- `a >> b` in ordinary code uses global operator resolution
- `a >> b` inside a `pipeline` block may mean "link these two steps"
- `a >> b` outside a `pipeline` block, but in a file that activated the pipeline DSL, may receive a targeted "outside the block" diagnostic

The latter does **not** imply that `a` or `b` globally implement `Shr`.

### Example: pipeline linking

```incan
pipeline user_sync:
    extract >> normalize >> validate >> store
```

Inside the block, the DSL may interpret that chain pairwise:

- `extract >> normalize`
- `normalize >> validate`
- `validate >> store`

Conceptually, the desugared meaning might be:

```incan
_pipeline_ctx.link(extract, normalize)
_pipeline_ctx.link(normalize, validate)
_pipeline_ctx.link(validate, store)
```

Outside the block, but still in a file that activated the pipeline DSL:

```incan
extract >> normalize
```

can produce a targeted diagnostic such as:

```text
`>>` between pipeline steps is only valid inside a `pipeline` block
```

### Example: query-style pipes

```incan
query active_users:
    users |> filter(active=True) |> group_by(country)
```

The exact desugaring is library-defined, but the important rule is that `|>` here is owned by the `query` block, not by a global `PipeForward` implementation on every participating type.

### Example: route-head composition

```incan
api app:
    route get + delete "/users/:id" -> users.destroy
```

Here `+` and `->` do not need to mean anything special everywhere in the `api` block. They only need to be special in the DSL's registered route-head position:

- `+` combines route verbs
- `->` maps the route specification to its handler/body

The same `api` block may still contain ordinary Incan expressions elsewhere. That is why this RFC models scoped surfaces as block-owned but position-scoped, not as blanket operator redefinition for the whole block body.

### Example: Rails-style routing families

Ruby on Rails routing shows how much mileage a DSL can get from a small amount of declarative syntax. Incan should be able to support similarly expressive surfaces:

```incan
api app:
    namespace admin:
        route get + post "/users" -> users.index
        route get + patch + delete "/users/:id" -> users.member
```

The important point is not that Incan must copy Rails literally. The point is that a library author should be able to define:

- a route-head position where `+` combines verbs
- a mapping position where `->` binds a route spec to a handler
- nested DSL blocks such as `namespace admin:` that preserve structure for desugaring

This is a good example of a DSL that is simultaneously:

- block-oriented
- position-scoped
- highly declarative

### Example: R-style data pipelines

R's data DSLs show that users often want left-to-right, readable pipeline syntax with small operator-like connectors:

```incan
query active_users:
    users
        |> filter(active == True)
        |> group_by(country)
        |> summarize(total = count())
```

or, with a block-owned assignment/binding form:

```incan
query revenue:
    net := sales |> mutate(net = gross - tax)
    net |> summarize(total = sum(net))
```

Here the library may want:

- `|>` in a query-expression position
- `:=` in a query-binding position
- ordinary arithmetic like `gross - tax` to remain ordinary Incan even inside the same enclosing block

That combination only works cleanly if the glyph semantics are position-scoped rather than whole-block overrides.

### Example: Matillion-style orchestration graphs

Matillion-style orchestration and transformation flows are another strong fit for scoped surface forms:

```incan
orchestration nightly_sales:
    extract_sales -> stage_raw -> run transform_sales -> publish_dashboard
    on_failure <- notify_slack
```

or with nested orchestration/transformation blocks:

```incan
pipeline nightly_sales:
    orchestration:
        extract -> stage -> run transform

    transformation:
        raw |> clean |> aggregate |> publish
```

This kind of library may want:

- `->` for forward stage dependencies
- `<-` for reverse notification or fallback relationships
- `|>` for transformation threading
- multiple related subgrammars inside one higher-level block

Again, the same DSL might reserve these glyphs only in graph-head or transform-chain positions, while leaving other expressions alone.

### Example: task/build automation DSLs

Ruby's Rake and Groovy/Kotlin-style build DSLs suggest another useful family:

```incan
build app:
    task lint + test + package -> publish
    file "dist/app.tar.gz" <- package
```

or:

```incan
tasks ci:
    namespace release:
        build -> test -> deploy
```

This reinforces a key point of the RFC: a glyph like `+` need not be globally special, or even special throughout the whole DSL block. It only needs a well-defined meaning in the DSL positions that register it.

### Example: binding-like glyphs

```incan
query totals:
    total := count()
```

This RFC reserves space for binding-like glyphs inside explicit blocks. `:=` in this design is **not** a global walrus operator. It is a DSL-owned glyph family that may create aliases, named slots, or intermediate bindings according to the DSL's registered contract.

### Example: leading-dot field access (expression-form surface)

```incan
query active_orders:
    FROM orders
    WHERE .status == "active"
    SELECT .customer_id, .amount
```

`.status`, `.customer_id`, and `.amount` are leading-dot field references. They resolve against the implicit primary relation supplied by the `FROM` clause. Outside this query block, `.status` at expression start is not valid Incan syntax.

The same pattern applies to method-chain relational arguments:

```incan
orders.filter(.amount > 100).select(.customer_id, .region)
```

Here `.amount`, `.customer_id`, and `.region` are in relational argument positions owned by the DSL that registered the `filter` and `select` operations. The implicit receiver is the dataset value the method is called on.

Conceptually, the meaning of `.amount` might be:

```incan
_relation_ctx.field("amount")
```

The DSL supplies the implicit context; the leading dot is the surface syntax. Outside eligible DSL positions, `.field` at expression start must be rejected.

### Scope boundary

Scoped surfaces are not only for compact custom notations. The important boundary is which compiler layer owns the surface.

RFC 040 owns the base scoped-surface layer for query-like blocks, site/application composition, assistant/workflow authoring, and descriptor-shaped app-builder template/style entry points. RFC 081 owns language-shaped lexical modes and token forms.

For RFC 040, "complete" means:

- DSL blocks and selected DSL expression positions can register scoped operator-like and binding-like surfaces.
- Existing core tokens such as `+`, `>`, `==`, `->`, `=>`, `//`, `>>`, and `<<` can be reused with DSL-local meaning only where descriptors make them eligible.
- Selected non-core symbolic glyphs such as `|>`, `%>%`, `:=`, and `===` are compatible with the descriptor model, but broad symbolic-token admission belongs to RFC 081 unless a constrained product DSL needs the exact glyph as a scoped surface.
- Leading-dot expression forms such as `.amount` and `.order.amount` work in eligible query/relational positions with descriptor-provided receiver derivation.
- DSL authors can provide diagnostics that explain why a surface is only legal in the owning DSL position.

RFC 081 owns the language-embedding question: descriptor-gated token forms and lexical submodes for markup, raw text, comments, regex/template literals, sigil identifiers, dimensions, colors, selector tokens, type-position syntax, and language-specific ambiguity rules. RFC 040 may still support a narrow product-specific template/style surface when it can be expressed as a constrained DSL surface built on the same descriptor contract.

### Product-readiness envelope

RFC 040 is not complete merely because the descriptor API exists. The base layer must be expressive enough for at least these product DSL families:

- **Query and relational DSLs** — support explicit query blocks and relational method arguments; leading-dot field paths in predicates and projections; scoped comparison, boolean, pipeline, binding, fallback, and exact-match surfaces where registered; receiver derivation from a block clause or method receiver; and descriptor-owned diagnostics when a field path or glyph escapes its eligible position.
- **Site and application composition DSLs** — support scoped route-head and route-mapping surfaces such as `get + post -> handler`; scoped application/layout/resource blocks; constrained template/style entry points when they can be represented as DSL-owned surfaces rather than full markup or CSS lexical modes; and durable handoff to a desugarer.
- **Assistant and workflow DSLs** — support pipeline, graph, routing, fallback, alias/binding, and shape-check surfaces such as `>>`, `->`, `//`, `:=`, and `===` in registered workflow positions; nested block ownership; ambiguity rejection; and stable metadata for formatter, diagnostics, and desugarer handoff.

These families are the minimum acceptance envelope, not a closed list. A DSL outside these families still belongs in RFC 040 when it can be expressed through scoped glyphs, binding-like surfaces, expression-form surfaces, descriptor metadata, and ordinary Incan expression re-entry without requiring RFC 081 lexical submodes.

### Canonical acceptance examples

A conforming implementation must be able to preserve the local meaning of each accepted surface without treating it as ordinary syntax that later phases rediscover by string matching.

For a pipeline block:

```incan
pipeline user_sync:
    extract >> normalize >> validate >> store
```

The `>>` occurrences are accepted only in the registered pipeline step-chain position. If the descriptor declares pairwise chaining, the semantic payload is the adjacent pairs `(extract, normalize)`, `(normalize, validate)`, and `(validate, store)`, with descriptor identity and source spans preserved for lowering, formatting, and diagnostics.

For a query block:

```incan
query active_orders:
    FROM orders
    WHERE .status == "active" and .amount > 100
    SELECT .customer_id, .order.amount
```

The leading-dot paths are accepted only in registered query predicate and projection positions. Their semantic payload is the path segments (`["status"]`, `["amount"]`, `["customer_id"]`, `["order", "amount"]`) plus the descriptor-provided receiver derivation, such as the primary relation introduced by `FROM orders`.

For an API block:

```incan
api app:
    route get + delete "/users/:id" -> users.destroy
```

The `+` and `->` occurrences are accepted only in the registered route-head and route-mapping positions. The same glyphs remain ordinary Incan syntax elsewhere.

Negative examples must also be precise:

```incan
from std.query import query

value = .amount
```

If the query DSL is active but the occurrence is not in an eligible query position, the compiler emits the descriptor's outside-scope diagnostic for the leading-dot surface. Without the active query descriptor, ordinary parser diagnostics apply.

```incan
from std.pipeline import pipeline

extract >> normalize
```

If ordinary `>>` resolution fails and the active pipeline descriptor exactly matches the surface shape, the compiler emits the descriptor's outside-scope diagnostic rather than a generic unknown-operator error.

## Reference-level explanation

### Core rule

Scoped surface forms are owned by the enclosing explicit DSL block, not by operand types alone and not by ambient runtime state.

Their semantics are **position-scoped** within that block. A DSL does not claim a surface for every expression in the block body; it claims the surface only for the eligible positions or subgrammars it explicitly registers.

Scoped surface activation has two scopes:

- **positive scope**: entering a registered block may activate DSL-owned surface meaning for eligible positions in that block body
- **negative scope**: activating the DSL in a file/module may enable targeted misuse diagnostics for that surface family elsewhere in that same file/module

Imports or other activation hooks do **not** globally change operator meaning. They only make the DSL's surface descriptors available to the current file/module so the compiler can:

- apply DSL-owned meaning inside eligible positions in eligible blocks
- emit better diagnostics for misplaced DSL-shaped surface use outside those blocks

This is analogous to method bodies having an implicit `self`: the body gets extra meaning from the enclosing construct, not from ambient runtime state. The difference here is that the DSL may also reserve a file-local "negative space" for misuse diagnostics.

### Registration and conflict policy

This RFC does **not** standardize a permanently closed surface inventory. Instead, it standardizes the rules under which a DSL may claim block-owned, position-scoped surface meaning.

A scoped surface is allowed if all of the following hold:

- the surface is explicitly registered by the DSL
- the surface shape is explicitly declared (symbolic glyph for operator-like or binding-like forms, or a declared expression-form shape such as leading-dot access)
- the surface declares its family (`OperatorLike`, `BindingLike`, or `ExpressionForm`)
- the surface does not collide with a core grammar form in the same position unless the DSL also declares an explicit eligibility/disambiguation rule
- the formatter and tooling can preserve it without ad-hoc special cases

Common operator-like examples include:

- `>>`
- `<<`
- `|>`
- `<|`
- `->`
- `<-`
- `//`
- `===`
- `+`

These are infix glyphs with operator-like shape. A DSL may interpret them as linking, piping, chaining, directional flow, strict or shape equality, verb composition, or other block-local relations.

Glyphs that already have core-language meanings need extra care. Scoped reuse of `//` must not change ordinary floor-division semantics outside registered positions. Scoped reuse of `->` or `<-` must not silently override existing core-language arrow forms such as function return annotations or other grammar positions that already reserve `->`. They are valid only where the enclosing block grammar explicitly admits a scoped surface occurrence.

Common binding-like examples include:

- `:=`

This is a binding-shaped glyph family, not an RFC 028 global operator. A DSL may interpret it as aliasing, named slots, or block-local binding according to its own surface contract.

### Expression-form surfaces

An expression-form surface is a syntax shape that is only valid inside eligible DSL positions and relies on an implicit receiver or context supplied by the owning block. The leading example is `.field` or `.path.to.field` — leading-dot field-path access.

Expression-form surfaces follow the same scoping contract as operator-like and binding-like surfaces:

- **positive scope**: inside eligible positions of an owning block, the expression form is parsed and resolved against the block-supplied implicit context.
- **negative scope**: outside eligible positions, the form must be rejected by the parser with a targeted diagnostic.
- **no global effect**: expression-form registration must not make the syntax form valid in ordinary Incan code.

A DSL registering an expression-form surface must declare:

- the syntactic shape (e.g. leading-dot followed by one or more identifiers)
- the eligible positions where it is valid
- how the implicit receiver/context is determined (e.g. primary relation from `FROM`, dataset value from method receiver)
- whether chained path segments are supported and how those segments are interpreted

Expression-form surfaces do not use `chain_mode` or `inherits_core_precedence`; those fields apply only to operator-like surfaces.

### Registration model

A vocab provider that registers a block keyword may also register scoped surface forms for that block.

Conceptually, a scoped surface descriptor needs to capture:

- `surface`: the glyph spelling or expression-form shape being claimed
- `family`: whether the surface is `OperatorLikeGlyph`, `BindingLikeGlyph`, or `ExpressionForm`
- `owning_block`: which explicit block kind owns the surface
- `eligible_positions`: which positions within the owning block are allowed to interpret the surface specially
- `misuse_scope`: where targeted misuse diagnostics may fire outside eligible positions
- `payload_shape`: what parsed data the accepted surface contributes, such as ordered operands or leading-dot path segments
- `receiver_context`: for expression-form surfaces, how the implicit receiver or context is derived
- `chain_mode`: whether repeated use is nested, pairwise, or not chainable
- precedence/disambiguation policy: whether the surface reuses ordinary token precedence or requires a narrower DSL-specific rule
- operand or target constraints: used for validation and diagnostics
- diagnostic templates: author-provided messages for outside-scope use, wrong operand kinds, invalid binding targets, ambiguous surface ownership, and related failures
- lowering/desugaring handoff identity: the stable identity later phases use to dispatch to the owning DSL behavior

The descriptor is the single source of truth for surface ownership. Later compiler phases and the formatter must not infer DSL-owned meaning by re-parsing source text or by string-matching raw glyphs. LSP integration must use the same metadata when the editor-facing path is added.

### Surface recognition

Scoped surfaces must remain distinguishable from ordinary language surfaces when they are used with DSL-owned meaning.

The semantic artifact requirement is simple:

- global operator expressions remain global operator expressions
- DSL-owned glyph occurrences remain identifiable as DSL-owned glyph occurrences
- expression-form surfaces such as leading-dot access remain identifiable as DSL-owned forms, not as newly general-purpose Incan syntax

Every accepted scoped-surface occurrence must carry at least:

- descriptor identity
- owning block identity
- eligible-position identity
- parsed payload
- receiver/context derivation, when applicable
- source span
- diagnostic identity
- lowering/desugaring handoff identity

The concrete AST or IR representation is an implementation detail, but the information above is not optional. Typechecking, desugaring/lowering handoff, and formatting must receive enough metadata to preserve the DSL-owned meaning without rediscovering it from raw text. LSP features must use the same artifact model when editor support lands.

### Resolution order

Resolution is deterministic:

1. Core grammar wins in core-only positions.
2. In eligible DSL positions, the innermost eligible owning block wins.
3. Same-depth competing descriptors for the same surface in the same eligible position are an ambiguity diagnostic.
4. Outside eligible DSL positions, ordinary Incan semantics apply, including RFC 028 global operator resolution where applicable.
5. If ordinary semantics fail and an active descriptor exactly matches the surface shape, the compiler may emit that descriptor's outside-scope diagnostic instead of a generic parser/operator error.

This yields the intended behavior:

- inside the owning block and an eligible position: DSL-owned meaning
- outside the owning block but inside the activating file/module: targeted diagnostic when the DSL can exactly recognize the surface shape
- ordinary code elsewhere: ordinary global operator semantics

For glyphs that already have a core syntactic role, such as `->`, ordinary language meaning remains authoritative outside positions the enclosing block has explicitly marked as eligible for DSL-owned interpretation.

### Nested and competing descriptors

Nested DSL blocks compose through an active descriptor stack.

```incan
pipeline nightly:
    extract >> normalize

    query orders:
        FROM orders
        WHERE .amount > 100
        SELECT .customer_id
```

Inside `query orders`, leading-dot expression forms are owned by the `query` descriptor if the current position is an eligible query predicate or projection position. The surrounding `pipeline` block does not claim that surface there merely because it is lexically active outside the nested block.

If both `pipeline` and `query` register `|>`, the innermost eligible block wins inside eligible query positions, and the pipeline descriptor wins inside eligible pipeline positions. If two active descriptors at the same lexical depth both claim `|>` for the same eligible position and neither is more specific, the occurrence is rejected as ambiguous rather than resolved by declaration order.

### Pairwise chaining

Operator-like scoped surfaces may opt into pairwise chaining.

In pairwise mode:

```incan
a >> b >> c >> d
```

means:

- `(a, b)`
- `(b, c)`
- `(c, d)`

not:

- `((a >> b) >> c) >> d`

This is important for DSLs that describe edges, links, or dataflow between adjacent stages.

In nested mode, the DSL receives the ordinary nested parse shape instead.

### Implicit block receiver/context

Scoped surface semantics are not ambient runtime magic. They are lexical semantics supplied by the enclosing block and the specific DSL position being parsed.

Conceptually, a DSL-owned surface acts against an implicit block context such as:

- `_pipeline_ctx`
- `_query_ctx`
- `_machine_ctx`

That context is supplied by the block/desugaring machinery, not discovered at runtime by a plain global dunder method.

So:

```incan
pipeline user_sync:
    extract >> normalize
```

is conceptually closer to:

```incan
_pipeline_ctx.link(extract, normalize)
```

than to:

```incan
extract.__rshift__(normalize)
```

### Diagnostics

This RFC requires targeted diagnostics for compiler-gated scoped surface misuse.

The RFC 040 diagnostic floor is:

- outside-scope use
    - example: "`>>` between pipeline steps is only valid inside a `pipeline` block"
- ambiguous surface ownership
    - example: "`|>` is ambiguous here: both `query` and `pipeline` register it for this position"

Descriptors also reserve diagnostic templates for semantic validators that are not RFC 040 closure blockers:

- wrong operand kinds inside the block
    - example: "`>>` in a `pipeline` block expects `PipelineStep` operands, got `Foo` and `Bar`"
- invalid binding target for binding-like glyphs
    - example: "`:=` in a `query` block expects an identifier on the left-hand side"
- invalid receiver derivation for expression-form surfaces
    - example: "`.amount` needs a query relation introduced by `FROM`"

DSL authors may provide diagnostic templates for these classes as part of the descriptor. The compiler controls when those templates may fire: an outside-scope diagnostic requires an active descriptor, an exact surface-shape match, and failure of ordinary parsing or ordinary operator resolution. The compiler must not use a DSL-authored message merely because the operands are vaguely similar to a DSL example.

### Formatter and LSP metadata

The formatter consumes scoped-surface metadata; it does not infer DSL semantics from punctuation alone. LSP support must consume the same metadata once the editor-facing integration exists.

For a leading-dot path such as `.order.amount`, tooling metadata includes the descriptor family, path segments, owning DSL, eligible position, source span, and receiver/context derivation. A future hover can therefore report that `.amount` is a query field reference resolved against the active relation, while the formatter can preserve the leading-dot path without treating it as ordinary field access.

For operator-like glyphs, the descriptor provides semantic hints such as `chain_mode = pairwise`, but line-breaking remains formatter policy. The formatter may consult `chain_mode` to avoid rewriting a pairwise chain as if it were an ordinary nested binary expression, but the descriptor does not prescribe exact line width or wrapping layout.

## Design details

### Interaction with RFC 028

RFC 028 defines ordinary global operator semantics.

This RFC does **not** add more global operators to the language. Instead, it defines how an explicit DSL block may reuse a registered glyph in eligible local positions without implying global trait adoption.

That means these are different statements:

- "`Query` globally implements `PipeForward`" (RFC 028)
- "`query` blocks own `|>` in registered query positions" (this RFC)

Both may exist in the language, but they are not the same mechanism and must not be conflated.

### Interaction with RFC 027

RFC 027 remains the substrate for:

- block registration
- placement rules
- scoped functions
- block desugaring

This RFC extends that world with block-owned, position-scoped surface forms. It does not replace RFC 027; it builds on it.

### Compatibility / migration

- **Non-breaking**: no existing syntax or semantics change. Scoped surface forms are additive and only activate inside explicit DSL blocks that register them.
- **No migration needed**: code that does not use scoped DSL blocks is unaffected.
- **Library adoption**: DSL authors opt in by registering scoped surface descriptors alongside their block keywords.

## Alternatives considered

1. Force DSL chaining through global RFC 028 operators
    Rejected because it makes block-local syntax pretend to be global type capability. That weakens diagnostics and blurs the boundary between ordinary operator overloading and explicit DSL context.

2. Let plain global dunder methods inspect ambient block state
    Rejected because it turns lexical language context into hidden runtime magic. Scoped surface meaning should come from the compiler's explicit block context, similar to how method bodies get `self`, not from "look around and see where I am" behavior.

3. Allow glyph semantics without explicit registration or conflict rules
    Rejected because it would make parsing, formatting, highlighting, and language tooling much harder to keep coherent. The problem is not that a DSL wants `+` or `->`; the problem is allowing those meanings without an explicit contract about where and how they apply.

## Drawbacks

- Parser and formatter complexity increase because some glyphs can now be block-local as well as global.
- Readers must understand that the same glyph can mean different things in ordinary code versus an explicit DSL block.
- Libraries and tooling need good diagnostics and clear docs; otherwise scoped surfaces can become opaque.

## Layers affected

- **Frontend recognition** — the language frontend must distinguish scoped-surface occurrences in eligible DSL positions from ordinary expressions; `->` and other glyphs with existing core meanings must continue to mean their ordinary language form outside registered positions; expression-form surfaces such as leading-dot access must only be accepted in eligible DSL positions and rejected elsewhere
- **Scoped symbolic recognition** — RFC 040 reuses existing core tokens and defines a descriptor-gated path for selected non-core symbolic glyphs such as `|>`, `%>%`, `:=`, and `===`; broad language-shaped token recognition belongs to RFC 081
- **RFC 081 boundary** — language-shaped token forms and lexical submodes must remain outside RFC 040 except for a deliberately narrowed product-specific template/style surface
- **Semantic analysis** — block-local surface resolution must follow the defined order (DSL-owned first, then core, then outside-scope diagnostic); active DSL descriptors must remain available to the current file/module
- **Lowering / execution handoff** — DSL-owned surface occurrences must preserve their block-owned meaning through later compilation stages; pairwise chaining must be expanded correctly
- **RFC 027 extension** — the vocab registration surface needs a scoped-surface descriptor so DSL authors can declare surfaces alongside block keywords; expression-position block kinds may need a small extension to support forms such as `race for value:`
- **Formatter** — must preserve scoped surface markers without ad-hoc special-casing; repeated chainable surfaces should format coherently
- **LSP** — follow-up editor integration should distinguish block-local glyph use from global operator use; misuse diagnostics should be actionable

## Implementation Plan

### Phase 1: Descriptor contract and manifest transport

- Extend the public vocab API with scoped-surface descriptors for operator-like glyphs, binding-like glyphs, and expression-form surfaces.
- Serialize registered scoped surfaces through library manifests without losing family, eligibility, misuse scope, diagnostic, receiver, and formatter metadata.
- Validate descriptor conflicts early enough that a malformed vocab crate cannot silently publish ambiguous scoped-surface behavior.

### Phase 2: Frontend recognition and semantic artifacts

- Extend syntax/AST support so accepted scoped-surface occurrences produce typed payloads instead of raw punctuation matches.
- Reuse existing core tokens in eligible DSL positions without changing their ordinary meaning outside those positions.
- Add leading-dot expression-form recognition for eligible DSL positions, including chained paths such as `.order.amount`.
- Preserve descriptor identity, owning block identity, eligible-position identity, payload, receiver derivation, and source span for later phases and tooling.

### Phase 3: Activation, resolution, and diagnostics

- Carry active scoped-surface descriptors from imported vocab crates into the file/module surface context.
- Resolve scoped-surface occurrences using the deterministic lexical ownership rules in this RFC.
- Emit author-provided, compiler-gated diagnostics for outside-scope use and ambiguous ownership.
- Preserve descriptor diagnostic classes for later semantic validators such as wrong operands, invalid binding targets, and invalid receiver derivation.

### Phase 4: Formatter and desugaring handoff

- Format scoped-surface expressions from semantic metadata rather than ad-hoc punctuation rewriting.
- Hand accepted scoped-surface artifacts to desugarers without requiring later phases to rediscover meaning from source text.
- Record LSP metadata consumption and direct lowering-hook dispatch as follow-up work rather than RFC 040 closure blockers.

### Phase 5: User-facing docs, release notes, and versioning

- Update vocab-authoring documentation with descriptor examples and diagnostic-template guidance.
- Add release notes for RFC 040 and document current limitations.
- Bump the active development version for the implementation.

## Implementation Log

### Spec / design

- [x] Move RFC 040 from Planned to In Progress for issue 174.
- [x] Keep design decisions synchronized with implementation discoveries.

### Descriptor contract and manifests

- [x] Add public scoped-surface descriptor types and builders to `incan_vocab`.
- [x] Support operator-like glyph, binding-like glyph, and expression-form descriptor families.
- [x] Include positive eligibility and misuse-scope metadata in descriptors.
- [x] Include receiver/context derivation metadata for expression-form descriptors.
- [x] Include author-provided diagnostic templates with stable diagnostic identities.
- [x] Serialize scoped-surface descriptors through library manifests.
- [x] Validate duplicate, ambiguous, or unsupported descriptor combinations.

### Frontend recognition and semantic artifacts

- [x] Carry active scoped-surface descriptors into frontend surface context.
- [x] Recognize eligible DSL-owned operator-like glyph surfaces without changing global RFC 028 behavior.
- [x] Recognize eligible leading-dot expression-form surfaces, including chained paths.
- [x] Reject leading-dot expression starts outside eligible scoped-surface positions.
- [x] Preserve accepted scoped-surface artifacts through later frontend stages.

### Diagnostics and conflict policy

- [x] Apply innermost-eligible-block ownership for nested DSLs.
- [x] Reject same-depth competing descriptors as ambiguous.
- [x] Emit targeted outside-scope diagnostics only when the compiler-gated conditions are met.
- [x] Preserve descriptor diagnostic classes for wrong operands, invalid binding targets, and invalid receiver derivation as follow-up semantic validation hooks.

### Formatter and desugaring handoff

- [x] Format accepted scoped-surface artifacts stably.
- [x] Preserve chain-mode hints for pairwise scoped-surface chains.
- [x] Hand scoped-surface artifacts to desugarers with descriptor identity intact.
- [x] Record LSP metadata consumption and direct lowering-hook dispatch as follow-up work, not RFC 040 closure scope.

### Product-readiness acceptance

- [x] Add a synthetic query/relational DSL example that exercises leading-dot field paths and desugarer handoff.
- [x] Add ignored executable product probes for query method arguments, route mapping, and workflow surfaces so missing RFC 040 coverage is tracked by tests.
- [x] Add synthetic query/relational coverage for method-argument DSL positions, not only block bodies.
- [x] Add a synthetic site/application composition DSL fixture for route-head and route-mapping scoped surfaces such as `get + post -> handler`.
- [x] Add a synthetic assistant/workflow DSL fixture for pipeline, graph, fallback, binding, and shape-check surfaces in registered workflow positions.
- [x] Validate that full template/style lexical needs belong to RFC 081; RFC 040 covers only descriptor-shaped app-builder entry points.

### Tests

- [x] Add descriptor builder and manifest round-trip tests.
- [x] Add parser/frontend tests for accepted scoped operator-like glyphs.
- [x] Add parser/frontend tests for accepted and rejected leading-dot expression forms.
- [x] Add diagnostics tests for outside-scope and ambiguity cases implemented in RFC 040.
- [x] Add formatter regression coverage for accepted scoped-surface payloads where existing test harnesses allow.
- [x] Add an end-to-end vocab/desugarer test that consumes scoped-surface artifacts.

### Docs and release

- [x] Update vocab-authoring docs with scoped-surface descriptor examples.
- [x] Update release notes for RFC 040.
- [x] Bump the active development version.

## Follow-up Work

These items are intentionally not RFC 040 closure blockers:

- Surface scoped-surface metadata to LSP hover, highlighting, and completion paths.
- Add direct lowering-hook dispatch for scoped surfaces that intentionally bypass desugaring.
- Add semantic validators and span-precise diagnostics for wrong operands, invalid binding targets, invalid receiver derivation, and invalid payload cases.
- Add LSP-facing metadata regression tests once an RFC 040-aware LSP harness exists.

## Design Decisions

1. **Scoped surface forms are the parent model.** Operator-like glyphs, binding-like glyphs, and expression-form shapes such as leading-dot paths are all registered scoped surface forms. The RFC keeps these families together because they share the same ownership, eligibility, diagnostic, semantic artifact, formatting, and tooling contracts.

2. **Positive scope and misuse scope are separate descriptor concerns.** A descriptor declares the eligible positions where a surface gains positive DSL-owned meaning and separately declares the misuse scope where targeted diagnostics may fire. Keeping those concerns separate lets a DSL accept a surface narrowly while still offering helpful file- or module-local diagnostics outside the owning block.

3. **Accepted scoped surfaces produce durable semantic artifacts.** The compiler must preserve descriptor identity, owning block identity, eligible-position identity, parsed payload, receiver/context derivation when applicable, source span, diagnostic identity, and lowering/desugaring handoff identity. Later phases and tooling must not rediscover scoped surface meaning by matching raw punctuation.

4. **Nested DSL ownership is lexical and deterministic.** In eligible DSL positions, the innermost eligible owning block wins. Same-depth competing descriptors for the same surface in the same eligible position are rejected as ambiguous. Core grammar remains authoritative in core-only positions.

5. **Formatter layout is formatter policy, but semantic hints come from descriptors.** A descriptor may expose `chain_mode`, payload shape, and surface family so the formatter preserves semantics. Exact line-breaking, wrapping, and width decisions remain formatter policy rather than descriptor metadata.

6. **Formatter consumes scoped-surface metadata now; LSP follows the same contract later.** Formatting and diagnostics receive the same scoped-surface identity and payload metadata as later compiler phases. Hover, highlighting, and completions must use the same artifact model when editor support is added, so tooling can distinguish block-local surface use from ordinary global operator or field-access syntax.

7. **Arrow-shaped glyphs require explicit eligible positions.** Scoped reuse of `->` or `<-` cannot opt out of core grammar globally. It is valid only where the owning block grammar explicitly admits a scoped surface occurrence; ordinary language meaning remains authoritative elsewhere.

8. **Expression-form receivers are descriptor-defined.** The language does not hardcode a single "primary relation" concept. A DSL registering an expression-form surface must declare how the implicit receiver/context is derived, such as a `FROM` clause or a method receiver.

9. **Leading-dot expression forms may support chained paths.** A descriptor may register leading-dot paths with one or more segments. Once a leading-dot path is accepted in an eligible DSL position, subsequent `.name` segments belong to that DSL-owned expression-form payload rather than ordinary field access on an already-resolved value.

10. **Targeted diagnostics are author-provided but compiler-gated.** DSL authors may provide diagnostic templates for outside-scope use, wrong operands, invalid binding targets, ambiguous ownership, and related failures. The compiler decides when those templates may fire; outside-scope diagnostics require an active descriptor, exact surface-shape match, and ordinary parsing or operator-resolution failure.
