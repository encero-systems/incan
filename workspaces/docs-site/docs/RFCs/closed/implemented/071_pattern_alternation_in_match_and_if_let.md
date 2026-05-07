# RFC 071: Pattern alternation in `match` and `if let`

- **Status:** Implemented
- **Created:** 2026-04-23
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 000 (core language surface)
    - RFC 029 (union types and type narrowing)
    - RFC 049 (`if let` single-arm conditional match)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/387
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** v0.3.0-dev.38

## Summary

This RFC introduces pattern alternation in Incan pattern positions used by `match` arms and `if let`. In practical terms, a form such as `PATTERN_A | PATTERN_B` means "match either pattern and execute the same branch." The feature removes repetitive duplicated branches for cases where several patterns share identical behavior, keeps the branch structure explicit, and aligns Incan with the pattern alternation capability readers already expect from languages such as Rust and Python structural matching. Alternation is deliberately constrained: all alternatives in one alternation must bind the same names with the same types, or bind no names at all.

## Core model

Read this RFC as one foundation plus three mechanisms:

1. **Foundation:** a single branch may be guarded by more than one pattern.
2. **Mechanism A:** `P1 | P2 | P3` is a valid pattern in `match` arms and `if let`.
3. **Mechanism B:** alternation does not weaken type safety; alternatives must agree on binding shape and binding types.
4. **Mechanism C:** alternation is branch-sharing syntax, not boolean logic and not value-level operator overloading.

## Motivation

Today, when several patterns should take the same branch, authors must spell those branches separately. That is noisy in ordinary code and especially awkward in compiler, planner, and runtime-adjacent code where enum dispatch is common.

Typical current code has to look like:

```incan
match node.kind:
    PrismNodeKind.Filter => return lower_passthrough(node)
    PrismNodeKind.OrderBy => return lower_passthrough(node)
    PrismNodeKind.Limit => return lower_passthrough(node)
    PrismNodeKind.Explode => return lower_passthrough(node)
```

That is repetitive without being more precise. The important semantic fact is that these four patterns share one branch. The language should let authors say that directly.

This matters beyond cosmetics. Incan uses Python-shaped syntax in places where readers often expect compiler or systems code. Repetition in those parts of the language surface makes dispatch logic longer, harder to scan, and easier to drift when one branch is edited but its siblings are not. Pattern alternation is the honest construct for "these cases are semantically identical."

RFC 049 already extends pattern matching into `if let`, and RFC 029 already makes pattern matching more expressive for unions and narrowing. Without alternation, both features still force duplicated boilerplate when multiple patterns share the same success path. The absence of alternation is now a real ergonomic gap in the pattern-matching story.

## Goals

- Allow multiple patterns to share one `match` arm.
- Allow `if let` to use the same alternation grammar as `match`.
- Preserve explicit branch structure without forcing duplicated branch bodies.
- Define strict rules for name binding across alternatives.
- Keep diagnostics clear when alternatives disagree on bindings or types.

## Non-Goals

- Introducing guards on `match` arms in this RFC.
- Extending alternation to every pattern-bearing construct in the language.
- Defining nested destructuring over arbitrary new pattern families.
- Turning `|` in pattern position into value-level boolean logic or operator overloading.
- Changing union-type syntax or value-level `|` behavior.

## Guide-level explanation

Pattern alternation lets one branch match more than one shape.

### Basic `match` example

```incan
match node.kind:
    PrismNodeKind.Filter | PrismNodeKind.OrderBy | PrismNodeKind.Limit | PrismNodeKind.Explode =>
        return lower_passthrough(node)
    PrismNodeKind.Project =>
        return lower_project(node)
```

That reads as: if `node.kind` is any of those four enum variants, run the same branch body.

### `if let` example

```incan
if let Some(value) | Ok(value) = result_like:
    print(value)
```

That shape is only valid if both alternatives bind the same name with the same type. If they do not, the language must reject it.

### Binding rules

These are good:

```incan
match value:
    Some(item) | Ok(item) => handle(item)
    None | Err(_) => pass
```

These are not:

```incan
match value:
    Some(item) | None => handle(item)     # one side binds `item`, the other does not
```

```incan
match value:
    Ok(value) | Err(error) => log(value)  # different binding names
```

Alternation is for "same branch, same binding shape." If the alternatives do not agree on that, authors should use separate branches.

## Reference-level explanation

### Syntax

This RFC extends pattern grammar in `match` arms and `if let` with pattern alternation:

```text
pattern             ::= alternative_pattern ("|" alternative_pattern)*
alternative_pattern ::= existing_match_pattern
```

`|` in pattern position is alternation, not a value-level operator. It is valid only where the grammar expects a pattern.

This RFC applies that grammar extension to:

- `match` arm patterns
- `if let` patterns as defined by RFC 049

It does not, by itself, require alternation to be supported in every other construct that may later reuse pattern grammar.

### Semantics

`P1 | P2 | ... | Pn` matches if any alternative pattern matches the scrutinee.

For `match`, the arm is selected when any alternative matches.

For `if let`, the success branch executes when any alternative matches.

Alternation is semantically equivalent to repeating the same branch body for each alternative in source order, except that it is one branch for formatting, diagnostics, and readability purposes.

### Binding agreement

All alternatives in one alternation must satisfy one of the following:

- every alternative binds no names, or
- every alternative binds exactly the same set of names, and each name has the same type in every alternative

The language must reject alternations where:

- one alternative binds a name and another does not
- alternatives bind different name sets
- the same binding name would have different inferred types across alternatives

### Scope

Bindings introduced by a successful alternation are in scope exactly where the surrounding pattern construct would normally place them:

- within the selected `match` arm body
- within the success branch of `if let`

Bindings from failed alternatives must not leak.

### Type checking

Type checking must validate each alternative against the scrutinee type under the same rules already used for ordinary patterns.

If all alternatives are valid, the alternation itself is valid only if binding agreement also holds.

Diagnostics should point to the specific alternatives that disagree on bindings or binding types.

### Exhaustiveness

For `match`, alternation contributes to exhaustiveness exactly as though each alternative were written as its own separate arm with the same body.

For example:

```incan
match value:
    int(_) | str(_) => use_present_value()
    None => pass
```

must be checked as covering `int`, `str`, and `None`, subject to the existing exhaustiveness rules for the scrutinee type.

## Design details

### Syntax

The chosen surface is infix `|` between patterns:

```incan
match kind:
    A | B | C => ...
```

This is the most familiar spelling for grouped pattern arms and keeps the "shared branch" structure visually obvious.

### Semantics

Alternation is a pattern-level construct, not a new expression feature. The same token already appears in type expressions and value expressions in different grammatical positions. That reuse is acceptable because the parser can distinguish pattern position from type position and expression position.

Binding agreement is the key semantic constraint. Without it, alternation would either force ad-hoc partial binding rules or create confusing branch-local name availability. Requiring agreement keeps alternation predictable.

### Interaction with existing features

- **RFC 029 (union types and type narrowing):** type patterns such as `int(n)` and `str(s)` may participate in alternation as long as binding agreement holds. Exhaustiveness must count each alternative normally.
- **RFC 049 (`if let`):** `if let` reuses the same pattern grammar as `match`, so it should gain alternation together with `match` rather than drifting into a separate capability set.
- **Testing / assertion patterns:** this RFC does not automatically extend alternation to any limited pattern-binding forms introduced by RFC 018. Those should opt in deliberately if and when their own pattern subset is expanded.
- **Operator overloading / expressions:** `|` in pattern position must not consult value-level operator overloading or expression semantics.

### Compatibility / migration

This feature is additive. Existing code remains valid.

The migration effect is optional simplification. Authors may collapse repeated identical branches into one alternation arm, but they are not required to do so.

Because alternation is only accepted in pattern position, there is no compatibility risk for existing value-level uses of `|`.

## Alternatives considered

- **Keep duplicated branches.** Rejected because it preserves unnecessary repetition in a part of the language where enum dispatch and narrowing are already central.
- **Support grouped arms only in `match`, not `if let`.** Rejected because RFC 049 explicitly reuses `match` pattern grammar; diverging them would create an avoidable inconsistency.
- **Allow alternatives with different bindings and make some names optional.** Rejected because that would complicate scope and typing rules significantly and make branch-local names harder to reason about.
- **Use a different grouped-arm syntax such as comma-separated patterns.** Rejected because `|` is the pattern-alternation spelling most readers already expect, and it distinguishes alternation from tuple-like or list-like punctuation.

## Drawbacks

- Reusing `|` in one more grammatical position increases parser and formatter complexity.
- Readers must understand the distinction between pattern-position `|`, type-expression `|`, and value-expression `|`.
- Binding-agreement rules add one more diagnostic path to pattern checking.

## Implementation architecture

Non-normative recommended shape:

- extend the internal pattern representation with an alternation node rather than desugaring too early
- type-check each alternative independently, then enforce binding agreement at the alternation node
- lower alternation through the existing match-oriented control-flow machinery rather than inventing a separate execution path

## Layers affected

- **Parser / AST**: pattern grammar must accept `|` alternation in `match` and `if let` pattern positions, and the AST should preserve alternation as a first-class pattern shape.
- **Typechecker / Symbol resolution**: pattern checking must validate each alternative, enforce binding agreement, and emit clear diagnostics when alternatives disagree.
- **IR Lowering**: lowering must preserve the semantics of grouped alternatives without changing existing branch behavior or exhaustiveness reasoning.
- **Emission**: emitted Rust must represent grouped alternatives faithfully, whether through native Rust pattern alternation or an equivalent lowered form.
- **Formatter**: formatter support is needed so long alternation arms remain readable and stable.
- **LSP / Tooling**: completions, hover, and diagnostics should treat grouped alternatives as one branch with ordinary per-pattern checking.

## Implementation Plan

### Phase 1: Parser, AST, and formatter

- Extend the pattern AST with a first-class alternation shape.
- Parse `|` as pattern alternation in `match` arm and `if let` pattern positions.
- Preserve ordinary value-level and type-level `|` behavior outside pattern positions.
- Format short alternations on one line and wrap long alternatives with existing multiline grouping rules.

### Phase 2: Typechecker and diagnostics

- Type-check every alternative against the same scrutinee type.
- Enforce identical binding name sets across alternatives.
- Enforce identical binding types across alternatives.
- Emit span-precise diagnostics for mismatched bindings and mismatched binding types.
- Treat wildcard-containing alternations such as `A | _` as valid patterns without adding a style diagnostic.

### Phase 3: Lowering, IR, and emission

- Lower AST alternation into the existing IR pattern alternation representation.
- Emit Rust pattern alternation or an equivalent lowered form without changing branch semantics.
- Preserve exhaustiveness behavior as though each alternative appeared as a separate arm with the same body.

### Phase 4: Tests and docs

- Add parser and formatter coverage for `match` and `if let` alternation.
- Add typechecker coverage for valid no-binding and same-binding alternatives.
- Add diagnostic coverage for missing bindings, different binding names, and different binding types.
- Add codegen snapshot coverage showing emitted branch sharing.
- Update user-facing language docs and release notes for the new pattern syntax.

## Implementation log

### Spec / design

- [x] Resolve scope inheritance: RFC 071 applies to `match` arms and `if let`; future pattern-bearing constructs opt in deliberately.
- [x] Resolve formatter policy: short alternations stay inline; long alternatives use existing multiline grouping behavior, including parenthesized grouping when needed.
- [x] Resolve wildcard policy: `_` may appear in alternations, `A | _` is valid and exhaustive, and this RFC does not add redundancy/style diagnostics.

### Parser / AST / formatter

- [x] AST: add a first-class pattern alternation shape.
- [x] Parser: parse `|` alternation in `match` arm patterns.
- [x] Parser: parse `|` alternation in `if let` patterns.
- [x] Parser: keep expression/type `|` behavior unchanged outside pattern positions.
- [x] Formatter: render short alternations stably.
- [x] Formatter: render long alternations with stable multiline grouping.

### Typechecker / diagnostics

- [x] Typechecker: validate each alternative against the scrutinee type.
- [x] Typechecker: accept alternatives with identical empty binding sets.
- [x] Typechecker: accept alternatives with identical binding names and types.
- [x] Diagnostic: reject alternatives where one side binds a name and another does not.
- [x] Diagnostic: reject alternatives with different binding names.
- [x] Diagnostic: reject alternatives with different inferred binding types.

### Lowering / IR / emission

- [x] Lowering: map AST alternation to IR pattern alternation.
- [x] Emission: emit faithful Rust grouped patterns or equivalent lowered branches.
- [x] Exhaustiveness: count every alternative as branch coverage for `match`.

### Tests / docs / release

- [x] Parser tests cover `match` and `if let` alternation.
- [x] Formatter tests cover inline and multiline alternation formatting.
- [x] Typechecker tests cover valid and invalid binding agreement.
- [x] Codegen snapshots cover emitted alternation.
- [x] User-facing language docs explain pattern alternation.
- [x] Release notes mention RFC 071 and issue #387.
- [x] Development version is bumped from the active `0.3.0-dev.N` line.

## Design Decisions

- Pattern alternation is limited to `match` arms and `if let` for this RFC. Future pattern-bearing constructs should opt in deliberately when they adopt the same pattern grammar.
- The formatter should keep short alternations on one line. When an alternation becomes too long, it may use multiline parenthesized grouping so the result remains parseable as one pattern.
- `_` may appear in an alternation like any other no-binding pattern. `A | _` is valid and exhaustive. This RFC does not introduce redundancy or style diagnostics for wildcard-containing alternations; a future lint policy may add those separately.
