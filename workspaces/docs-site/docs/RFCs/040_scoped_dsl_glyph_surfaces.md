# RFC 040: Scoped DSL Glyph Surfaces

- **Status:** Draft
- **Created:** 2026-03-08
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 027 (`incan-vocab` block registration and desugaring)
    - RFC 028 (global operator overloading)
    - RFC 045 (scoped DSL symbol surfaces — companion RFC for identifier-level scoping)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/332
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

Introduce **scoped glyph surfaces** for explicit DSL blocks and their registered subgrammars.

A registered DSL may declare scoped glyph surfaces with two layers of effect:

- **positive meaning** inside an explicit owning block and its eligible registered positions
- **negative misuse diagnostics** across the activating file/module outside that block

Operator-like glyphs such as `>>`, `<<`, `|>`, `<|`, `->`, `<-`, or `+` can support chaining, routing, linking, or composition within a DSL block without implying that the operand types globally implement the corresponding RFC 028 operator traits. Binding-like glyphs such as `:=` are reserved for block-local alias/binding forms and are specified as a separate family from operators. Expression-form surfaces such as leading-dot field access (`.column`) provide implicit-receiver syntax that is valid only inside eligible DSL positions, without becoming a general Incan expression form.

This RFC does **not** define a closed whitelist of legal DSL surfaces. It defines the registration, scoping, parsing-eligibility, and conflict rules that make scoped surfaces safe to use. The surface set is open-ended as long as a DSL registers the shape explicitly and it coexists cleanly with the core grammar and tooling.

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

If the types globally implement RFC 028 operator traits, the compiler can no longer honestly say "this is only valid inside a pipeline block." That is why block-local glyph meaning needs its own mechanism.

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

But the same family also needs room for future binding-like surfaces such as `:=`, where the glyph is not a global walrus operator, but a block-local alias or slot-binding form.

Beyond operators and bindings, DSLs also need **expression-form surfaces**: syntax shapes that are only valid inside eligible DSL positions. The leading example is `.column` notation — a leading-dot field access with an implicit receiver supplied by the owning block's context. Outside DSL positions, `.field` at expression start is not valid Incan syntax and must be rejected. Inside a query or relational block, `.amount` means "the `amount` field of the primary relation" without needing a named receiver.

This RFC therefore covers **scoped DSL surfaces** as a broader category, split into operator-like, binding-like, and expression-form families.

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

- Define the registration, scoping, parsing-eligibility, and conflict rules under which explicit DSL blocks may own position-scoped glyph surfaces.
- Cover three surface families: operator-like glyphs (e.g. `>>`, `|>`), binding-like glyphs (e.g. `:=`), and expression-form surfaces (e.g. leading-dot `.field` access).
- Ensure scoped glyph meaning is block-local and position-scoped: glyphs gain DSL-owned semantics only inside eligible positions of an explicit owning block, not as blanket redefinition for the whole block body.
- Provide targeted misuse diagnostics when DSL-shaped glyphs appear outside eligible positions but inside an activating file or module.
- Coexist cleanly with RFC 028 global operator overloading and the rest of the core grammar.
- Keep the surface set open-ended: any glyph that satisfies the registration and conflict rules is valid, with no pre-closed whitelist.

## Non-goals

- Arbitrary ad-hoc punctuation with no registration or conflict policy
- Project-wide or import-only operator redefinition
- Hidden ambient runtime state as the source of block-local meaning
- A global walrus operator for ordinary Incan code
- Scoped identifier symbol semantics (e.g. `sum`, `count` gaining DSL-specific meaning by position) — that belongs to RFC 045

## Guide-level explanation

### Same glyph, separate semantic namespace

The central rule is:

- the **glyph** may be the same
- the **owner of the meaning** is different

Outside an owning DSL block, or outside the DSL positions that block explicitly marks as eligible, the glyph falls back to the ordinary language surface and is interpreted according to RFC 028 and the rest of the core grammar.

Inside an explicit DSL block, the enclosing block may own a position-scoped meaning for the same glyph.

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

The same `api` block may still contain ordinary Incan expressions elsewhere. That is why this RFC models scoped glyphs as block-owned but position-scoped, not as blanket operator redefinition for the whole block body.

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

Matillion-style orchestration and transformation flows are another strong fit for scoped glyph surfaces:

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

### Example: future binding-like glyphs

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

## Reference-level explanation

### Core rule

Scoped glyphs are owned by the enclosing explicit DSL block, not by the operand types alone.

Their semantics are **position-scoped** within that block. A DSL does not claim a glyph for every expression in the block body; it claims the glyph only for the eligible positions or subgrammars it explicitly registers.

Scoped glyph activation has two scopes:

- **positive scope**: entering a registered block may activate DSL-owned glyph meaning for eligible positions in that block body
- **negative scope**: activating the DSL in a file/module may enable targeted misuse diagnostics for that glyph family elsewhere in that same file/module

Imports or other activation hooks do **not** globally change operator meaning. They only make the DSL's glyph descriptors available to the current file/module so the compiler can:

- apply DSL-owned meaning inside eligible positions in eligible blocks
- emit better diagnostics for misplaced DSL-shaped glyph use outside those blocks

This is analogous to method bodies having an implicit `self`: the body gets extra meaning from the enclosing construct, not from ambient runtime state. The difference here is that the DSL may also reserve a file-local "negative space" for misuse diagnostics.

### Registration and conflict policy

This RFC does **not** standardize a permanently closed glyph inventory. Instead, it standardizes the rules under which a DSL may claim block-owned, position-scoped glyph meaning.

A scoped glyph is allowed if all of the following hold:

- the glyph is explicitly registered by the DSL
- the surface shape is explicitly declared (symbolic glyph for operator-like or binding-like forms, or a declared expression-form shape such as leading-dot access)
- the surface declares its family (`OperatorLike`, `BindingLike`, or `ExpressionForm`)
- the glyph does not collide with a core grammar form in the same position unless the DSL also declares an explicit eligibility/disambiguation rule
- the formatter and tooling can preserve it without ad-hoc special cases

Common operator-like examples include:

- `>>`
- `<<`
- `|>`
- `<|`
- `->`
- `<-`
- `+`

These are infix glyphs with operator-like shape. A DSL may interpret them as linking, piping, chaining, directional flow, verb composition, or other block-local relations.

Arrow-shaped glyphs deserve one extra constraint: scoped reuse of `->` or `<-` must not silently override existing core-language arrow forms such as function return annotations or other grammar positions that already reserve `->`. They are valid only where the enclosing block grammar explicitly admits a scoped glyph occurrence.

Common binding-like examples include:

- `:=`

This is a binding-shaped glyph family, not an RFC 028 global operator. A DSL may interpret it as aliasing, named slots, or block-local binding according to its own surface contract.

### Expression-form surfaces

An expression-form surface is a syntax shape that is only valid inside eligible DSL positions and relies on an implicit receiver or context supplied by the owning block. The leading example is `.field` — leading-dot field access.

Expression-form surfaces follow the same scoping contract as operator-like and binding-like surfaces:

- **positive scope**: inside eligible positions of an owning block, the expression form is parsed and resolved against the block-supplied implicit context.
- **negative scope**: outside eligible positions, the form must be rejected by the parser with a targeted diagnostic.
- **no global effect**: expression-form registration must not make the syntax form valid in ordinary Incan code.

A DSL registering an expression-form surface must declare:

- the syntactic shape (e.g. leading-dot followed by identifier)
- the eligible positions where it is valid
- how the implicit receiver/context is determined (e.g. primary relation from `FROM`, dataset value from method receiver)

Expression-form surfaces do not use `chain_mode` or `inherits_core_precedence`; those fields apply only to operator-like surfaces.

### Registration model

A vocab provider that registers a block keyword may also register scoped glyph surfaces for that block.

Conceptually, a scoped glyph descriptor needs to capture:

- `surface`: the glyph or expression-form shape being claimed
- `family`: whether the surface is operator-like, binding-like, or expression-form
- `owning_block`: which explicit block kind owns the surface
- `positive_scope`: where the surface gains DSL-owned meaning
- `misuse_scope`: where targeted misuse diagnostics may fire
- `eligible_positions`: which positions within the owning block are allowed to interpret the surface specially
- `chain_mode`: whether repeated use is nested, pairwise, or not chainable
- precedence/disambiguation policy: whether the surface reuses ordinary token precedence or requires a narrower DSL-specific rule
- operand or target constraints: used for validation and diagnostics
- `outside_scope_diagnostic`: optional targeted messaging for likely misuse

### Surface recognition

Scoped glyphs should remain distinguishable from ordinary language surfaces when they are used with DSL-owned meaning.

The semantic requirement is simple:

- global operator expressions remain global operator expressions
- DSL-owned glyph occurrences remain identifiable as DSL-owned glyph occurrences
- expression-form surfaces such as leading-dot access remain identifiable as DSL-owned forms, not as newly general-purpose Incan syntax

How a frontend chooses to represent that distinction internally is an implementation detail. The requirement is that later phases and tooling can still tell which semantic path the user invoked.

### Resolution order

For a glyph-shaped form such as `a >> b`:

1. If the current file/module has activated a DSL that declares this glyph family, keep its descriptor available for diagnostics and scoped parsing eligibility.
2. If the current occurrence is inside an eligible owning block and an eligible registered DSL position, and it matches that block's scoped glyph descriptor, use the DSL-owned glyph resolution path.
3. Otherwise use the ordinary language path (including RFC 028 global operator resolution where applicable).
4. If ordinary resolution fails, and an active file-local descriptor matches the operand constraints or shape closely enough, emit that descriptor's outside-scope diagnostic instead of a generic operator error.

This yields the intended behavior:

- inside the owning block and an eligible position: DSL-owned meaning
- outside the owning block but inside the activating file/module: targeted diagnostic when the DSL can recognize likely misuse
- ordinary code elsewhere: ordinary global operator semantics

For glyphs that already have a core syntactic role, such as `->`, ordinary language meaning remains authoritative outside positions the enclosing block has explicitly marked as eligible for DSL-owned interpretation.

### Pairwise chaining

Operator-like scoped glyphs may opt into pairwise chaining.

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

Scoped glyph semantics are not ambient runtime magic. They are lexical semantics supplied by the enclosing block and the specific DSL position being parsed.

Conceptually, a DSL-owned glyph acts against an implicit block context such as:

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

This RFC requires targeted diagnostics for scoped glyph misuse.

Required classes:

- outside-scope use
    - example: "`>>` between pipeline steps is only valid inside a `pipeline` block"
- wrong operand kinds inside the block
    - example: "`>>` in a `pipeline` block expects `PipelineStep` operands, got `Foo` and `Bar`"
- invalid binding target for binding-like glyphs
    - example: "`:=` in a `query` block expects an identifier on the left-hand side"

These should be preferred over generic "unknown operator" or "type mismatch" messages when the compiler has enough information to recognize the scoped intent.

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

This RFC extends that world with block-owned, position-scoped glyph surfaces. It does not replace RFC 027; it builds on it.

### Compatibility / migration

- **Non-breaking**: no existing syntax or semantics change. Scoped glyph surfaces are additive and only activate inside explicit DSL blocks that register them.
- **No migration needed**: code that does not use scoped DSL blocks is unaffected.
- **Library adoption**: DSL authors opt in by registering scoped surface descriptors alongside their block keywords.

## Alternatives considered

1. Force DSL chaining through global RFC 028 operators
    Rejected because it makes block-local syntax pretend to be global type capability. That weakens diagnostics and blurs the boundary between ordinary operator overloading and explicit DSL context.

2. Let plain global dunder methods inspect ambient block state
    Rejected because it turns lexical language context into hidden runtime magic. Scoped glyph meaning should come from the compiler's explicit block context, similar to how method bodies get `self`, not from "look around and see where I am" behavior.

3. Allow glyph semantics without explicit registration or conflict rules
    Rejected because it would make parsing, formatting, highlighting, and language tooling much harder to keep coherent. The problem is not that a DSL wants `+` or `->`; the problem is allowing those meanings without an explicit contract about where and how they apply.

## Drawbacks

- Parser and formatter complexity increase because some glyphs can now be block-local as well as global.
- Readers must understand that the same glyph can mean different things in ordinary code versus an explicit DSL block.
- Libraries and tooling need good diagnostics and clear docs; otherwise scoped glyphs can become opaque.

## Layers affected

- **Frontend recognition** — the language frontend must distinguish scoped-surface occurrences in eligible DSL positions from ordinary expressions; `->` and other glyphs with existing core meanings must continue to mean their ordinary language form outside registered positions; expression-form surfaces such as leading-dot access must only be accepted in eligible DSL positions and rejected elsewhere
- **Semantic analysis** — block-local glyph resolution must follow the defined order (DSL-owned first, then core, then outside-scope diagnostic); active DSL descriptors must remain available to the current file/module
- **Lowering / execution handoff** — DSL-owned glyph occurrences must preserve their block-owned meaning through later compilation stages; pairwise chaining must be expanded correctly
- **RFC 027 extension** — the vocab registration surface needs a scoped-glyph descriptor so DSL authors can declare glyphs alongside block keywords; expression-position block kinds may need a small extension to support forms such as `race for value:`
- **Formatter** — must preserve scoped glyph markers without ad-hoc special-casing; repeated chainable surfaces should format coherently
- **LSP** — hover and syntax highlighting should distinguish block-local glyph use from global operator use; misuse diagnostics should be actionable

## Unresolved questions

1. What are the exact APIs for `PositiveScope` and `MisuseScope`? These types appear in `ScopedGlyphDescriptor` but are not fully defined in this RFC.
2. Can scoped glyph registrations nest? If a DSL block contains a nested DSL sub-block, and both register `|>`, which descriptor wins inside the inner block?
3. How does the formatter determine when to line-break a pairwise chain vs. keep it inline? Is that a formatter config or part of the glyph descriptor?
4. Should `PositiveScope` and `MisuseScope` be separate concerns, or should a single `ScopeDescriptor` express both simultaneously?
5. How does the LSP communicate the block-local vs global semantic distinction in completions and hover? Does the language server need access to which DSL blocks are active in the current file?
6. Does this RFC need to define how a DSL explicitly opts `->` out of core function-return-annotation parsing in a position that admits both?
7. For expression-form surfaces, should the implicit receiver be a single well-known concept (e.g. "primary relation") or should DSLs be able to register arbitrary implicit receiver shapes?
8. Should expression-form surfaces support chained leading-dot access (e.g. `.order.amount`) or only single-level `.field`? If chained, how does the parser distinguish DSL-owned chained access from ordinary Incan field access on the resolved value?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
