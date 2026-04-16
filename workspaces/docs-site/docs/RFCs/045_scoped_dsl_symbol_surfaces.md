# RFC 045: Scoped DSL symbol surfaces

- **Status:** Draft
- **Created:** 2026-03-27
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 027 (`incan-vocab` library registration and desugaring)
    - RFC 028 (global operator overloading)
    - RFC 040 (scoped DSL glyph surfaces)
    - RFC 022 (namespaced stdlib and compiler handoff)
    - RFC 031 (library system phase 1)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/202
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC introduces scoped DSL symbol surfaces: library-registered identifier semantics that are active only inside explicit owning DSL blocks and eligible DSL positions. It allows names such as `sum`, `count`, `avg`, or other DSL-defined symbols to be ambient where the DSL owns meaning, while preserving ordinary Incan resolution outside those positions. The feature is designed to let multiple DSL ecosystems define concise domain vocabulary without global namespace collisions or ad-hoc compiler special cases.

## Core model

Read this RFC as one foundation plus three mechanisms:

1. **Foundation:** symbol meaning can be scoped to an explicit owning DSL block and eligible positions, not only to global lexical scope.
2. **Mechanism A:** DSL authors register scoped symbol descriptors alongside existing DSL surfaces.
3. **Mechanism B:** typechecking resolves scoped symbols first in eligible positions, then falls back to ordinary language resolution.
4. **Mechanism C:** core builtins remain globally available through explicit namespaced access even when scoped symbols reuse the same unqualified name.

## Motivation

Incan currently has globally recognized builtin names such as `sum`, and unqualified call resolution treats those names as core language vocabulary. This creates friction for DSLs that need concise, domain-specific meaning for the same symbols in constrained contexts. Query-like DSLs often need `sum(...)` and `count(...)` to express aggregate intent without import boilerplate or naming workarounds, while still preserving ordinary builtin behavior in non-DSL code. Without a scoped symbol mechanism, library authors must either pick awkward substitute names, force mandatory imports for common domain primitives, or ask the compiler for one-off special handling per DSL. Those options do not scale as more DSLs and libraries adopt concise, context-owned authoring surfaces. The language already has a scoped model for glyphs (RFC 040). Identifier symbols need the same class of capability so that DSL meaning can be explicit, local, and tooling-safe.

## Goals

- Allow libraries to register scoped identifier symbols (for example `sum`, `count`) as part of DSL surfaces.
- Make scoped symbols ambient inside eligible DSL positions without requiring explicit import of each symbol.
- Keep ordinary Incan name resolution unchanged outside scoped DSL positions.
- Define deterministic precedence between scoped symbols, lexical names, and core builtins.
- Provide explicit escape hatches so users can still call core builtins and non-DSL symbols when names overlap.
- Preserve parser, typechecker, lowering, formatter, and LSP parity through a shared registry-driven contract.

## Non-Goals

- Replacing or removing core builtin functions.
- Introducing runtime dunder-style operator or function dispatch for scoped symbols.
- Making imports globally mutate language semantics outside explicit DSL activation.
- Defining one DSL-specific aggregate catalog in this RFC; symbol sets remain library-defined.
- Standardizing backend-specific execution semantics for any one domain library.
- Scoped glyph surfaces (`>>`, `|>`, `:=`) or scoped expression forms (`.column`) — those belong to RFC 040.

## Guide-level explanation (how users think about it)

A DSL can own short names in the places where that DSL has semantic authority.

Inside an owning query-like block, `sum` and `count` can mean aggregate functions by default:

```incan
from pub::analytics import query

result = query {
    FROM orders
    GROUP BY .region
    SELECT
        region,
        count() as order_count,
        sum(.amount) as total_revenue,
}
```

No per-symbol import is required in that scoped position.

Outside that query block, ordinary Incan behavior applies:

```incan
totals = sum([1, 2, 3])  # ordinary language meaning outside scoped DSL positions
```

When users need to force core builtin meaning even inside a scoped DSL position, they use explicit namespaced builtin access:

```incan
result = query {
    FROM metrics
    SELECT std.builtins.sum(.raw_values) as scalar_sum
}
```

When users need to force a non-DSL symbol with the same name, they use explicit qualification or aliasing:

```incan
from my_math import sum as numeric_sum

result = query {
    FROM metrics
    SELECT numeric_sum(.value) as adjusted
}
```

## Reference-level explanation (precise rules)

### Scoped symbol descriptor

A DSL may register a scoped symbol descriptor with at least:

- `symbol` (identifier spelling, e.g. `sum`)
- `family` (e.g. aggregate-like, function-like)
- `owning_block`
- `positive_scope`
- `misuse_scope`
- `eligible_positions`
- `outside_scope_diagnostic` (optional)

Field names intentionally mirror RFC 040 scoped surface descriptors so vocab registration and tooling can treat glyph surfaces and symbol surfaces as parallel, composable metadata. For symbols, `eligible_positions` describes **name-resolution** contexts (for example where an unqualified call target may be interpreted as a scoped DSL symbol), not parser-only glyph eligibility.

Descriptor naming and shape should follow the same registry discipline used for other DSL metadata.

### Activation

- Scoped symbol descriptors must be activated only through explicit DSL activation in the current file/module.
- Activation must not change symbol meaning globally across unrelated files/modules.

### Resolution order in eligible positions

For an unqualified call-form identifier `name(...)` in an eligible position of an active owning block:

1. If `name` matches an active scoped symbol descriptor for that owning block and position, it must resolve as the scoped DSL symbol.
2. If not matched by scoped descriptors, ordinary language resolution must apply (lexical bindings, imports, and module-scoped names per existing rules).
3. If ordinary resolution fails, builtin resolution may still apply per existing language rules.

Precedence when **both** an active scoped symbol descriptor and an ordinary lexical binding could apply to the same unqualified call (for example an imported alias that spells the same as a scoped DSL symbol) is not settled in this Draft; see **Unresolved questions**.

### Resolution outside eligible positions

- Scoped symbol descriptors must not alter ordinary language resolution outside eligible positions.
- Outside eligible positions, names must resolve exactly as they do without scoped symbol registration.

### Explicit builtin access

- Core builtins must be reachable through explicit namespaced access (`std.builtins.<name>`).
- Scoped symbol matching must not intercept explicit namespaced builtin calls.

### Diagnostics

Implementations should emit targeted diagnostics for:

- likely outside-scope DSL symbol misuse
- ambiguous intent where both scoped DSL meaning and ordinary lexical meaning are plausible
- missing activation when a scoped symbol is used in DSL-styled contexts

Diagnostics should suggest explicit rewrites (`std.builtins.<name>` or qualified lexical symbol) when overlap occurs.

### Compatibility expectations

- Existing code outside scoped DSL positions must preserve behavior.
- Inside scoped DSL positions, behavior may change from ordinary builtin resolution to DSL-owned resolution where descriptors are active.
- DSL authors should document scoped symbol sets as part of their language surface contract.

## Design details

### Syntax

This RFC does not require new token syntax. It reuses ordinary identifier call syntax and explicit namespaced calls.

### Semantics

Scoped symbol semantics are lexical and compile-time, not runtime. Meaning comes from the enclosing DSL block and eligible position metadata, not from dynamic value dispatch.

### Interaction with existing features

- **RFC 027 (`incan-vocab`)**: scoped symbol descriptors should be registered through the same extension surface and activation pipeline.
- **RFC 040 (scoped DSL surfaces)**: scoped symbols and scoped glyphs/expression forms are parallel mechanisms; neither replaces the other. Leading-dot field access (`.column`) is an RFC 040 expression-form surface, not an RFC 045 concern.
- **RFC 028 (global operators)**: scoped symbol semantics do not modify global operator contracts.
- **Imports/modules**: explicit qualification remains the escape hatch for overlapping names.
- **LSP/formatter**: tooling must reflect scoped symbol meaning in eligible positions and ordinary meaning elsewhere.

### Compatibility / migration

The feature is additive at language surface level, but it can change behavior for overlapping unqualified names inside active DSL positions. DSL maintainers should provide migration guidance where previous versions required per-symbol imports or alternate spellings.

## Alternatives considered

- **Keep import-only DSL symbols forever.** Rejected because it keeps repetitive boilerplate and naming workarounds for common DSL primitives; every DSL user must import `sum`, `count`, etc. explicitly even though the meaning is unambiguous in context.
- **Require globally unique DSL symbol names.** Rejected because it produces unnatural user-facing APIs (`inql_sum`, `hees_count`) and does not scale across independent DSL ecosystems that may reasonably want the same short names.
- **Allow global shadowing of builtins by libraries.** Rejected because it is too broad and unsafe; it breaks predictability outside DSL contexts and makes it impossible for the compiler to provide helpful diagnostics about intent.
- **Use runtime dunder-like dispatch for function calls.** Rejected because scoped language meaning should be compile-time and lexical, not ambient runtime magic; the enclosing DSL block owns the meaning, not a runtime inspection of "where am I."
- **Fold scoped symbols into RFC 040 (scoped glyph surfaces).** Rejected because scoped identifiers are a distinct problem class from scoped syntax tokens; glyphs need parser-level token handling; identifiers need name-resolution-level precedence rules; keeping them in separate RFCs keeps each RFC focused and independently implementable.

## Drawbacks

- Adds complexity to name-resolution rules and diagnostics.
- Increases mental-model surface: the same identifier may mean different things by context.
- Requires careful tooling parity to prevent editor/compiler drift.
- Requires explicit compatibility guidance for DSL upgrades when scoped symbol sets evolve.

## Implementation architecture

Non-normative recommended shape:

- Extend DSL registration metadata with scoped symbol descriptors.
- Carry owning-block and eligible-position context through semantic analysis.
- Resolve scoped symbols before ordinary fallback in eligible positions.
- Preserve scoped symbol identity through later compilation stages so DSL-owned meaning remains unambiguous.
- Preserve explicit namespaced builtin calls as direct builtin targets.

## Layers affected

- **Frontend recognition**: the language frontend must preserve enough DSL block and clause-position context for scoped symbol eligibility checks.
- **Name resolution**: symbol resolution must apply scoped symbol precedence in eligible positions and preserve ordinary resolution elsewhere.
- **Lowering / execution handoff**: later compilation stages must keep scoped symbol identity so DSL-owned semantics are emitted through the correct path.
- **Emission**: emitted output must preserve DSL-scoped meaning and explicit builtin calls without ambiguity.
- **`incan-vocab` registry**: registration metadata must support scoped symbol descriptors and activation information.
- **Formatter**: formatting should preserve explicit qualification used to disambiguate scoped vs builtin calls.
- **LSP / tooling**: completions, hover, go-to-definition, and diagnostics should reflect scoped symbol meaning by context.

## Unresolved questions

- Should scoped symbol precedence in eligible positions always outrank lexical names, or should lexical aliases outrank scoped symbols when explicitly imported?
- Is `std.builtins.<name>` the correct canonical builtin escape path, or should a dedicated reserved root be introduced instead?
- Should misuse diagnostics outside eligible positions be warning-level or error-level by default when a likely scoped intent is detected?
- How should scoped symbol descriptor families be modeled for non-aggregate DSL symbols so tooling can provide semantic hints consistently?
- Should descriptor activation be file-local only, or should there be an opt-in module subtree propagation model?
- How should scoped symbol overloading across nested DSL blocks be resolved when multiple active descriptors share a name?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
