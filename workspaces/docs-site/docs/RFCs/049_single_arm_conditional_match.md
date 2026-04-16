# RFC 049: `if let` single-arm conditional match

- **Status:** Draft
- **Created:** 2026-04-02
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 000 (core language surface)
    - RFC 018 (testing)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/333
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC introduces `if let` as a first-class Incan control-flow construct for single-arm conditional pattern matching. It is intended for cases where authors care about exactly one successful pattern and want the non-match case to do nothing, such as replacing boilerplate like `Some(x) => ...` paired with `None => pass` with `if let Some(child) = filter.input:`. This RFC does not replace full `match`; full `match` remains the canonical construct for multi-arm branching, exhaustive reasoning, and cases where the non-match path is semantically meaningful. The design is intentionally closer to Rust than Python because the motivating cases in Incan are primarily explicit `Option` and `Result` management, so Rust-style destructuring control flow is a better fit than Python-style truthiness.

## Core model

1. `if let PATTERN = VALUE:` attempts to match `VALUE` against `PATTERN`.
2. If the match succeeds, the body executes and any names bound by the pattern are available inside that body.
3. If the match fails, the body is skipped and no names are bound.
4. Full `match` remains the preferred construct when more than one arm matters.

## Motivation

In Incan today, it is common to inspect an `Option[T]`, `Result[T, E]`, or enum payload, perform a small action when one shape is present, and otherwise do nothing.

That often produces code like:

```incan
match filter.input:
    Some(child) => return [child]
    None => pass
```

or:

```incan
match result:
    Err(err) => log(err)
    Ok(_) => pass
```

These are explicit, but they are also repetitive. The unmatched arm often adds no meaning beyond "do nothing."

This RFC introduces a surface that says exactly that: perform one pattern match, execute one body on success, and implicitly skip the non-match path.

That choice is deliberate. Incan's problem here is not "how do we make conditionals feel more Pythonic." It is "how do we reduce boilerplate around explicit `Option` and `Result` handling without weakening the language's pattern-matching model." Rust has already established that `if let` is an effective answer for that specific problem shape, and this RFC follows that direction.

## Goals

- Add a concise, explicit single-arm conditional pattern-match form.
- Reuse existing pattern semantics rather than introducing truthiness-based control flow.
- Allow successful matches to bind names with normal lexical scope inside the success branch.
- Keep full `match` as the primary construct for multi-arm or exhaustive branching.
- Align the surface with a familiar and proven construct where that improves readability.

## Non-Goals

- Replacing full `match`.
- Introducing Python-style truthiness such as `if child:`.
- Adding multi-arm shorthand, match guards, or expression-level pattern-match sugar in this RFC.
- Defining raw Rust passthrough syntax as a language feature.
- Changing existing constructor syntax in value position.

## Guide-level explanation

### Basic form

```incan
def first_child(filter: Filter) -> List[Rel]:
    if let Some(child) = filter.input:
        return [child]
    return []
```

This means:

```incan
def first_child(filter: Filter) -> List[Rel]:
    match filter.input:
        Some(child) => return [child]
        None => pass
    return []
```

### More examples

```incan
def log_failure(result: Result[int, str]) -> None:
    if let Err(err) = result:
        log(err)

def first_join_child(rel: Rel) -> List[Rel]:
    if let RelType.Join(join) = rel.rel_type:
        if let Some(left) = join.left:
            return [left]
    return []
```

### What this is for

Use `if let` when:

- exactly one pattern matters;
- the non-match case should do nothing;
- the code reads more clearly as opportunistic extraction than as branching.

### What this is not for

When both outcomes matter, use full `match`:

```incan
match result:
    Ok(value) => cache.store(value)
    Err(err) => logger.error(err)
```

This RFC also does not introduce truthiness:

```incan
# Not part of this RFC
if child:
    return [child]
```

## Reference-level explanation

### Syntax

This RFC introduces an `if let` statement form:

```text
if_stmt ::= "if" if_test ":" block
if_test ::= expr | if_let_test
if_let_test ::= "let" pattern "=" expr
```

The `pattern` grammar is the same pattern grammar already used by `match` arms.

This RFC only introduces `if let` in statement position. It does not introduce `let` patterns in arbitrary boolean expression positions.

### Semantics

- `VALUE` must be evaluated exactly once.
- The pattern match must use the same matching rules as a `match` arm.
- If the pattern matches, the `if let` body executes.
- If the pattern does not match, the body is skipped.
- A failed match must not bind any names.

The following:

```incan
if let PATTERN = VALUE:
    BODY
```

is semantically equivalent to:

```incan
match VALUE:
    PATTERN => BODY
    _ => pass
```

### Scope and binding

- Names bound by the pattern are in scope only within the `if let` success branch.
- Those names are not in scope after the `if let` completes.
- Shadowing behavior follows the same rules as bindings introduced by `match` arms.

### Typing

- `VALUE` must be type-checkable against `PATTERN` under the same rules as a `match` arm.
- Impossible patterns must produce the same kind of type errors as `match`.
- Bound names receive the same types they would receive in the equivalent `match` arm.

### Errors and diagnostics

- Diagnostics should describe this construct as pattern matching, not assignment.
- Unused pattern bindings should follow normal lint behavior.
- Tooling should explain `if let` in terms of its equivalent single-arm `match` when helpful.

## Design details

### Why `if let`

This RFC deliberately chooses `if let` as the primary surface instead of `PATTERN match VALUE`, `value is Pattern`, or walrus-style syntax.

The reasons are:

- it is immediately recognizable to users familiar with Rust-style destructuring control flow;
- it clearly communicates that the construct is pattern-matching-oriented;
- it reads naturally in single-arm extraction cases;
- it scales cleanly from `Option` and `Result` to enum payloads and other destructuring patterns.

Most importantly, it does not require inventing a new control-flow spelling for a problem already well served by an established shape.

It is also a better fit for Incan than a Python-flavored shorthand such as `if child:`. Incan's motivating examples are about matching structured values like `Some(...)`, `Ok(...)`, and `Err(...)`, not about truthiness. Choosing a Rust-aligned surface keeps the semantics explicit and keeps the feature centered on shape-based control flow.

### Why this is not "raw Rust passthrough"

The syntax is Rust-aligned, but this RFC does **not** define `if let` as "whatever Rust accepts."

Incan owns the construct. That means:

- the grammar is specified in Incan terms;
- the semantics are specified in Incan terms;
- lowering to Rust `if let` is an implementation strategy, not the language definition.

That distinction matters because Incan should remain free to evolve its own pattern grammar, diagnostics, and lowering strategy without accidentally turning backend quirks into language law.

### Why not general `let` inside any `if`

This RFC does not propose a general rule like "if the parser sees `let` in an `if` condition, forward it to Rust."

That approach is too broad for a young language because it:

- blurs the boundary between Incan syntax and backend syntax;
- risks surprising edge cases if Rust accepts shapes Incan does not want to standardize;
- makes future non-Rust lowering harder.

This RFC instead standardizes one narrow construct: `if let PATTERN = VALUE:`.

### Supported usage

The intended sweet spot is shallow, single-arm extraction:

```incan
if let Some(child) = filter.input:
    return [child]

if let Ok(value) = result:
    return value

if let RelType.Cross(cross) = rel.rel_type:
    process(cross)
```

### Interaction with full `match`

Use full `match` when:

- more than one arm is meaningful;
- the unmatched path matters to the reader;
- exhaustiveness matters;
- nesting would make `if let` chains harder to read than a single `match`.

This RFC therefore reinforces a style rule:

- use `if let` for opportunistic extraction with implicit no-op on failure;
- use `match` for true branching.

### Interaction with `Option` and `Result`

`if let` is especially useful for:

- `Option[T]` via `Some(...)`;
- `Result[T, E]` via `Ok(...)` and `Err(...)`.

This RFC does not change the meaning of `?`. The `?` operator remains the preferred construct for propagation. `if let` is for side effects, local extraction, and control flow that intentionally continues after non-match.

### Interaction with RFC 018 `is`

RFC 018 already uses `is` in pattern-oriented assertions. That remains valid and useful in assertion contexts.

This RFC does not extend `is` into single-arm conditional control flow. The reason is conceptual clarity:

- `is` reads as a boolean pattern test;
- `if let` reads as a destructuring control-flow construct.

For this RFC's narrow goal, `if let` is the better fit.

## Alternatives considered

1. Keep using full `match` everywhere.
This preserves one construct but keeps the repetitive `None => pass` / `Err(_) => pass` boilerplate that motivated this RFC.

2. `PATTERN match VALUE`.
This is explicit, but it introduces a new dedicated control-flow spelling where a well-understood construct already exists.

3. Extend `is`, as in `if value is Some(child):`.
This is plausible, especially given RFC 018, but it frames the feature more as a boolean pattern test than as a single-arm destructuring branch.

4. Walrus-style binding.
This is awkward for pattern-matching constructs, especially around `Some(...)`, `Ok(...)`, and other constructors. It obscures the fact that the operation is a pattern match rather than assignment.

5. General Rust passthrough for `let` inside `if`.
This was rejected because it weakens Incan's ownership of its own syntax and semantics.

## Drawbacks

- The language gains another control-flow surface.
- Users must learn when to prefer `if let` over full `match`.
- Formatter and linter guidance will matter to prevent overly dense nested `if let` chains.

## Implementation architecture

The preferred implementation strategy is to express `if let` through the same semantic core already used by full `match`.

That can be done either by:

- interpreting `if let` as a single-arm `match` plus implicit `_ => pass`, or
- representing it separately while preserving the same pattern-matching semantics.

This section is non-normative. Any implementation strategy is acceptable if it preserves the semantics above.

## Layers affected

- **Language surface**: `if let PATTERN = VALUE:` must be accepted in `if` statement position.
- **Type system**: the pattern must type-check exactly like a `match` arm, and bindings must stay scoped to the success branch.
- **Execution handoff**: implementations may realize `if let` through the existing pattern-match machinery as long as the observable semantics match this RFC.
- **Formatter**: `if let` should format predictably and avoid unreadable nested chains.
- **LSP / tooling**: hover, completion, and diagnostics should respect branch-local pattern bindings.

## Unresolved questions

- Should RFC 049 be limited to `if let` in statement position for v1, or should `while let` be considered in a follow-up RFC once the core shape lands?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
