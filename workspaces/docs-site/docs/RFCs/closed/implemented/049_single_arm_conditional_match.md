# RFC 049: `if let` and `while let` pattern control flow

- **Status:** Implemented
- **Created:** 2026-04-02
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 000 (core language surface)
    - RFC 018 (testing)
- **Issue:** https://github.com/encero-systems/incan/issues/333
- **RFC PR:** —
- **Written against:** v0.3.0-dev.1
- **Shipped in:** v0.3.0-dev.1

## Summary

This RFC introduces `if let` and `while let` as first-class Incan control-flow constructs for pattern-oriented destructuring. `if let` is intended for cases where authors care about exactly one successful pattern and want the non-match case to do nothing, such as replacing boilerplate like `Some(x) => ...` paired with `None => pass` with `if let Some(child) = filter.input:`. `while let` covers the looping counterpart: continue iterating while a pattern keeps matching, then stop when it no longer does. This RFC does not replace full `match`; full `match` remains the canonical construct for multi-arm branching, exhaustive reasoning, and cases where the non-match path is semantically meaningful. The design is intentionally closer to Rust than Python because the motivating cases in Incan are primarily explicit `Option`, `Result`, and enum destructuring, so Rust-style pattern control flow is a better fit than Python-style truthiness.

## Core model

1. `if let PATTERN = VALUE:` attempts to match `VALUE` against `PATTERN` once.
2. If that match succeeds, the body executes and any names bound by the pattern are available inside that body.
3. If that match fails, the body is skipped and no names are bound.
4. `while let PATTERN = VALUE:` attempts the same match on each loop condition check and continues iterating while it succeeds.
5. Full `match` remains the preferred construct when more than one arm matters.

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

This RFC introduces surfaces that say exactly that: perform one pattern match and execute one body on success, or keep looping while one pattern keeps matching.

That choice is deliberate. Incan's problem here is not "how do we make conditionals feel more Pythonic." It is "how do we reduce boilerplate around explicit `Option` and `Result` handling without weakening the language's pattern-matching model." Rust has already established that `if let` is an effective answer for that specific problem shape, and this RFC follows that direction.

## Goals

- Add concise, explicit pattern-oriented control-flow forms for one-arm extraction and loop-while-match cases.
- Reuse existing pattern semantics rather than introducing truthiness-based control flow.
- Allow successful matches to bind names with normal lexical scope inside the success branch or loop body.
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

def drain(queue: Queue[Option[int]]) -> List[int]:
    values = []
    while let Some(value) = queue.pop_front():
        values.append(value)
    return values
```

### What this is for

Use `if let` when:

- exactly one pattern matters;
- the non-match case should do nothing;
- the code reads more clearly as opportunistic extraction than as branching.

Use `while let` when:

- each iteration should continue only while one pattern keeps matching;
- the loop naturally ends on first non-match;
- writing the same destructuring `match` or `while true` + `break` pattern would be noisier.

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

This RFC introduces `if let` and `while let` statement forms:

```text
if_stmt ::= "if" if_test ":" block
while_stmt ::= "while" while_test ":" block
if_test ::= expr | if_let_test
while_test ::= expr | while_let_test
if_let_test ::= "let" pattern "=" expr
while_let_test ::= "let" pattern "=" expr
```

The `pattern` grammar is the same pattern grammar already used by `match` arms.

This RFC introduces `if let` and `while let` in statement position. It does not introduce `let` patterns in arbitrary boolean expression positions.

### Semantics

- `VALUE` must be evaluated exactly once.
- The pattern match must use the same matching rules as a `match` arm.
- If the pattern matches, the `if let` body executes.
- If the pattern does not match, the body is skipped.
- A failed match must not bind any names.
- In `while let`, the condition expression must be re-evaluated on each iteration, just as an ordinary `while` condition is re-checked on each iteration.
- In `while let`, the loop body executes only for iterations whose condition pattern matched successfully.
- In `while let`, the first non-match exits the loop without binding names for that failed attempt.

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

The following:

```incan
while let PATTERN = VALUE:
    BODY
```

is semantically equivalent to:

```incan
while true:
    match VALUE:
        PATTERN => BODY
        _ => break
```

### Scope and binding

- Names bound by the pattern are in scope only within the `if let` success branch.
- Names bound by a `while let` condition are in scope only within the loop body for the successful iteration that produced them.
- Those names are not in scope after the `if let` or `while let` completes.
- Shadowing behavior follows the same rules as bindings introduced by `match` arms.
- In v1, `if let` remains single-arm only: it does not accept `elif` or `else` branches. When the non-match path matters, use `match`.

### Typing

- `VALUE` must be type-checkable against `PATTERN` under the same rules as a `match` arm.
- Impossible patterns must produce the same kind of type errors as `match`.
- Bound names receive the same types they would receive in the equivalent `match` arm.

### Errors and diagnostics

- Diagnostics should describe this construct as pattern matching, not assignment.
- Unused pattern bindings should follow normal lint behavior.
- Tooling should explain `if let` in terms of its equivalent single-arm `match` when helpful.

## Design details

### Why `if let` and `while let`

This RFC deliberately chooses `if let` and `while let` as the primary surfaces instead of `PATTERN match VALUE`, `value is Pattern`, or walrus-style syntax.

The reasons are:

- it is immediately recognizable to users familiar with Rust-style destructuring control flow;
- it clearly communicates that the construct is pattern-matching-oriented;
- it reads naturally in single-arm extraction cases;
- it scales cleanly from `Option` and `Result` to enum payloads and other destructuring patterns;
- it covers both the one-shot and looping variants of the same pattern-control-flow idea instead of standardizing an asymmetrical surface.

Most importantly, it does not require inventing a new control-flow spelling for a problem already well served by an established shape.

It is also a better fit for Incan than a Python-flavored shorthand such as `if child:`. Incan's motivating examples are about matching structured values like `Some(...)`, `Ok(...)`, and `Err(...)`, not about truthiness. Choosing a Rust-aligned surface keeps the semantics explicit and keeps the feature centered on shape-based control flow.

### Why this is not "raw Rust passthrough"

The syntax is Rust-aligned, but this RFC does **not** define `if let` / `while let` as "whatever Rust accepts."

Incan owns the construct. That means:

- the grammar is specified in Incan terms;
- the semantics are specified in Incan terms;
- lowering to Rust `if let` / `while let` is an implementation strategy, not the language definition.

That distinction matters because Incan should remain free to evolve its own pattern grammar, diagnostics, and lowering strategy without accidentally turning backend quirks into language law.

### Why not general `let` inside any `if`

This RFC does not propose a general rule like "if the parser sees `let` in an `if` or `while` condition, forward it to Rust."

That approach is too broad for a young language because it:

- blurs the boundary between Incan syntax and backend syntax;
- risks surprising edge cases if Rust accepts shapes Incan does not want to standardize;
- makes future non-Rust lowering harder.

This RFC instead standardizes two narrow constructs: `if let PATTERN = VALUE:` and `while let PATTERN = VALUE:`.

### Supported usage

The intended sweet spot is shallow, single-arm extraction:

```incan
if let Some(child) = filter.input:
    return [child]

if let Ok(value) = result:
    return value

if let RelType.Cross(cross) = rel.rel_type:
    process(cross)

while let Some(token) = stream.next():
    process(token)
```

### Interaction with full `match`

Use full `match` when:

- more than one arm is meaningful;
- the unmatched path matters to the reader;
- exhaustiveness matters;
- nesting would make `if let` chains harder to read than a single `match`.

This RFC therefore reinforces style rules:

- use `if let` for opportunistic extraction with implicit no-op on failure;
- use `while let` for repeated extraction that should stop on first non-match;
- use `match` for true branching.

### Interaction with `Option` and `Result`

`if let` and `while let` are especially useful for:

- `Option[T]` via `Some(...)`;
- `Result[T, E]` via `Ok(...)` and `Err(...)`.

This RFC does not change the meaning of `?`. The `?` operator remains the preferred construct for propagation. `if let` is for side effects, local extraction, and control flow that intentionally continues after non-match, while `while let` is for repeated extraction that naturally stops on first non-match.

### Interaction with RFC 018 `is`

RFC 018 already uses `is` in pattern-oriented assertions. That remains valid and useful in assertion contexts.

This RFC does not extend `is` into destructuring conditional or loop control flow. The reason is conceptual clarity:

- `is` reads as a boolean pattern test;
- `if let` / `while let` read as destructuring control-flow constructs.

For this RFC's narrow goal, `if let` / `while let` are the better fit.

## Alternatives considered

1. Keep using full `match` everywhere. This preserves one construct but keeps the repetitive `None => pass` / `Err(_) => pass` boilerplate that motivated this RFC.

2. `PATTERN match VALUE`. This is explicit, but it introduces a new dedicated control-flow spelling where a well-understood construct already exists.

3. Extend `is`, as in `if value is Some(child):`. This is plausible, especially given RFC 018, but it frames the feature more as a boolean pattern test than as a single-arm destructuring branch.

4. Walrus-style binding. This is awkward for pattern-matching constructs, especially around `Some(...)`, `Ok(...)`, and other constructors. It obscures the fact that the operation is a pattern match rather than assignment.

5. General Rust passthrough for `let` inside `if` / `while`. This was rejected because it weakens Incan's ownership of its own syntax and semantics.

6. Ship only `if let` now and defer `while let`. This was rejected because the two constructs share the same mental model, pattern semantics, and implementation machinery. Deferring `while let` would force users back into `while true` plus `match` / `break` boilerplate for the looping form of the exact same problem.

## Drawbacks

- The language gains additional control-flow surface area.
- Users must learn when to prefer `if let` / `while let` over full `match`.
- Formatter and linter guidance will matter to prevent overly dense nested `if let` or `while let` chains.

## Implementation architecture

The preferred implementation strategy is to express `if let` and `while let` through the same semantic core already used by full `match`.

That can be done either by:

- interpreting `if let` as a single-arm `match` plus implicit `_ => pass`, or
- interpreting `while let` as repeated single-arm matching plus implicit loop exit on non-match, or
- representing them separately while preserving the same pattern-matching semantics.

This section is non-normative. Any implementation strategy is acceptable if it preserves the semantics above.

## Layers affected

- **Language surface**: `if let PATTERN = VALUE:` must be accepted in `if` statement position, and `while let PATTERN = VALUE:` must be accepted in `while` statement position.
- **Type system**: the pattern must type-check exactly like a `match` arm, and bindings must stay scoped to the success branch or successful loop iteration body.
- **Execution handoff**: implementations may realize `if let` / `while let` through the existing pattern-match machinery as long as the observable semantics match this RFC.
- **Formatter**: `if let` / `while let` should format predictably and avoid unreadable nested chains.
- **LSP / tooling**: hover, completion, and diagnostics should respect branch-local and loop-body-local pattern bindings.

## Implementation Plan

### Phase 1: Parser, AST, and formatter

- Extend the statement grammar so `if` and `while` conditions can carry `let PATTERN = VALUE` tests in statement position.
- Represent `if let` and `while let` explicitly in the frontend AST, preserving spans for the pattern, value, and body.
- Teach the formatter to print both constructs predictably and keep nested pattern-control-flow readable.

### Phase 2: Typechecker and scope

- Validate `if let` / `while let` patterns under the same rules as `match` arms.
- Bind names only within the success branch or successful loop iteration body.
- Emit span-precise diagnostics for impossible or otherwise invalid patterns in these positions.

### Phase 3: Lowering and emission

- Lower `if let` to the existing conditional/match machinery while preserving single-evaluation semantics.
- Lower `while let` to repeated pattern checking with loop exit on first non-match.
- Emit correct Rust for both constructs without broadening the accepted Incan surface beyond this RFC.

### Phase 4: Tooling, tests, and docs

- Preserve hover, completion, and diagnostics behavior for pattern bindings inside `if let` and `while let`.
- Add parser, typechecker, codegen snapshot, formatter, and integration coverage for both constructs.
- Update user-facing docs and release notes for the new control-flow surface.

## Implementation log

### Spec / design

- [x] Settle RFC 049 as `if let` plus `while let`, both in statement position only.
- [x] Record the binding, scope, and non-goal rules in the RFC body.

### Parser / AST

- [x] Parser: accept `let PATTERN = VALUE` tests in `if` statements.
- [x] Parser: accept `let PATTERN = VALUE` tests in `while` statements.
- [x] AST: represent `if let` with span-precise structure.
- [x] AST: represent `while let` with span-precise structure.
- [x] Formatter: round-trip both constructs stably.

### Typechecker

- [x] Validate `if let` patterns with `match`-equivalent checking.
- [x] Validate `while let` patterns with `match`-equivalent checking.
- [x] Scope bound names to the success branch or successful loop iteration body only.
- [x] Emit clear diagnostics for invalid pattern usage in these positions.

### Lowering / emission

- [x] Lower `if let` to the existing control-flow core.
- [x] Lower `while let` to the existing control-flow core.
- [x] Emit correct Rust for both constructs.

### Tooling

- [x] Keep diagnostics wording aligned with pattern-matching semantics rather than assignment.
- [x] Preserve LSP behavior for pattern bindings inside `if let` and `while let`.

### Tests

- [x] Parser unit tests for `if let`.
- [x] Parser unit tests for `while let`.
- [x] Typechecker unit tests for valid and invalid `if let`.
- [x] Typechecker unit tests for valid and invalid `while let`.
- [x] Codegen snapshot tests for `if let`.
- [x] Codegen snapshot tests for `while let`.
- [x] Integration coverage for end-to-end behavior.

### Docs

- [x] Update docs-site pages that describe control flow or pattern matching.
- [x] Add a release-notes entry for RFC 049 / issue #333.

## Design Decisions

- RFC 049 includes both `if let` and `while let`; `while let` is not deferred to a follow-up RFC.
- Both constructs are statement-position control-flow forms only in v1. This RFC does not introduce general `let` patterns in arbitrary boolean expressions.
- Both constructs inherit the same pattern semantics and diagnostics contract as `match` arms rather than creating a separate matching model.
