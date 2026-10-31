# RFC 138: Chained pattern conditions

- **Status:** Planned
- **Created:** 2026-10-11
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 029 (union types and narrowing)
    - RFC 049 (`if let` and `while let` pattern control flow)
    - RFC 068 (protocol hooks and truthiness)
    - RFC 071 (pattern alternation)
    - RFC 129 (binding declarations and mutation permissions)
    - RFC 134 (pattern bindings with exiting fallbacks)
    - RFC 135 (lazy Option defaults with `or`)
    - RFC 139 (sequence patterns)
    - RFC 157 (separate match-guard extension)
- **Issue:** [#1183](https://github.com/encero-systems/incan/issues/1183)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Allow parenthesized `if` and `while` conditions to combine pattern bindings and ordinary conditions through short-circuiting `and`. Each successful pattern makes its bindings available to later terms and the body, while any failed term skips the body. Parentheses distinguish this condition form from existing single-pattern `if let` and `while let` initializers, whose expression parsing remains unchanged. The feature keeps optional work flat without introducing bindings into arbitrary boolean expressions.

## Motivation

Request dispatch, optional configuration inspection, and parser lookahead frequently require several successful extractions before a small action. Existing `if let` expresses each extraction correctly, but repeated success bodies obscure the action:

```incan
if let Some(declared) = document.get("schema"):
    if let Some(name) = declared.as_str():
        if name != SCHEMA and name != SCHEMA_V2:
            return Err(SelectionError(
                kind=SelectionErrorKind.InvalidInput,
                fields=["schema"],
            ))
```

Missing or non-string schema values deliberately fall through to later validation. RFC 134's exiting fallback would change that behavior if it returned immediately on failed extraction. A chain describes the same conditional action with one body and the same fallthrough.

## Goals

- Combine dependent extractions and ordinary conditions without nested success bodies.
- Preserve left-to-right evaluation and short-circuiting.
- Keep bound names local to successful conditions and their body.
- Reuse existing patterns, truthiness, narrowing, and ownership rules.
- Preserve the parsing and behavior of existing single-pattern conditions.
- Support both one-shot conditions and repeated conditional extraction.

## Non-Goals

- Binding names inside general boolean expressions, assignments, assertions, or comprehensions.
- Combining binding terms with top-level `or` or `not`.
- Adding `else` or `elif` to pattern-bearing `if` statements.
- Adding match-arm guards or new pattern forms.
- Extending bindings into statements following the conditional.
- Rolling back side effects or moves when a later term fails.

## Guide-level explanation

All chained conditions below describe proposed behavior. Application declarations such as `SelectionError` retain their surrounding program's definitions.

### Inspect several optional layers

The schema check becomes:

```incan
if (
    let Some(declared) = document.get("schema")
    and let Some(name) = declared.as_str()
    and name != SCHEMA
    and name != SCHEMA_V2
):
    return Err(SelectionError(
        kind=SelectionErrorKind.InvalidInput,
        fields=["schema"],
    ))
```

The second extraction runs only after the first succeeds. The string comparisons run only after the string exists. If any term fails, execution resumes after the entire `if`, just as it does after the nested original.

### Put inexpensive checks before extraction

Ordinary conditions can precede or follow bindings:

```incan
if (
    enabled
    and let Some(path) = configured_path
    and path != ""
):
    println(f"Using {path}")
```

When `enabled` is false, the remaining terms are not evaluated. `path` is available to the final condition and the body, but not after the statement.

### Repeat extraction while a condition holds

```incan
while (
    len(pending) > 0
    and let item = pending.pop()
    and should_process(item)
):
    process(item)
```

Here `pending` is a mutable list of integers, `should_process` is an application predicate, and `process` accepts an integer. The length check prevents calling `pop()` on an empty list; the existing list method returns an element directly, rather than an Option. Each iteration pops at most one item. An item rejected by `should_process` has already been removed; the chain does not put it back. This is appropriate only when stopping after consuming that item is intended.

### Use ordinary expressions inside a term

An ordinary term may contain a grouped alternative:

```incan
if (
    let Some(name) = configured_name
    and (name == "primary" or name == "fallback")
):
    println(name)
```

A binding initializer containing its own boolean `and` must group that expression:

```incan
if (
    let false = (enabled and authorized)
    and should_report()
):
    report_disabled_operation()
```

The outer `and` separates condition terms. The inner `and` belongs to the matched value.

### Choose the construct from the intended control flow

Use a chain when all checks authorize one conditional action and failure should fall through. Use RFC 134's `let … else` when failure must exit and successful bindings should remain available afterward. Use exhaustive `match` when several alternatives require different behavior.

## Reference-level explanation

### Condition syntax

A chained pattern condition must appear directly as the parenthesized condition of an `if` or `while` statement. It consists of one or more terms separated by top-level `and`, with at least one term of the form `let PATTERN = EXPRESSION`. A term that is not a binding must be an ordinary condition expression.

The outer parentheses are required even when the complete condition fits on one line:

```incan
if (let Some(port) = configured_port and port > 0):
    println(port)
```

Top-level `and` inside these parentheses must separate terms. Operators inside nested parentheses, calls, collection literals, or other delimited subexpressions must retain their ordinary parsing. A binding initializer requiring an `and` of its own must therefore group it explicitly.

Top-level `or` between chain terms and negation of a binding term must be rejected. Ordinary terms and binding initializers may contain those operators within their own grouped expressions. A parenthesized boolean condition containing no binding must retain existing boolean-expression semantics.

Existing unparenthesized `if let PATTERN = EXPRESSION:` and `while let PATTERN = EXPRESSION:` must retain their full expression initializer. Their parsing must not be reinterpreted as a chain by this RFC.

### Evaluation and control flow

Terms must be evaluated once each in source order, stopping at the first nonmatching pattern or false ordinary condition. Every ordinary condition must use the existing condition truthiness rules, including applicable protocol hooks. The body must execute only after all terms succeed.

A failed `if` condition must continue after the statement. A failed `while` condition must terminate that loop. The entire chain must be reevaluated at the next loop condition check, including after a `continue` targeting that loop. A `?` or other existing control-flow operation inside a term must retain its ordinary enclosing-function semantics.

Pattern-bearing `if` statements must remain single-arm, consistent with RFC 049. An `else` or `elif` branch on a chained pattern condition must be rejected. This RFC does not change ordinary boolean `if` branching.

### Binding and narrowing scope

Each pattern must be checked against its own initializer using the existing pattern rules. A successful binding must be visible to all later terms and the body, but not its own initializer, an earlier term, or statements after the conditional. An earlier binding must be available in a later binding's initializer.

Each name must be introduced at most once within one chain. An outer name may be shadowed according to ordinary binding rules, but a later term must not rebind an earlier chain-local name. Pattern alternation must obey the existing requirement that alternatives bind compatible names and types.

Successful truthiness and type checks must carry existing narrowing facts forward only along their successful paths. No chain-local binding or success-only narrowing fact may leak into a path reached after failure.

### Ownership and lifetime

Matching must preserve existing move, borrow, and mutation permissions. The compiler must not clone, allocate replacement payloads, or undo earlier effects merely to make a chain usable.

When a later term fails, earlier expressions have already executed. Ownership checking of the continuation must account for every possible failed prefix. An authored move on an earlier successful term cannot be treated as if it never happened just because a later condition was false.

Required initializer temporaries and borrows must remain valid through their uses in later terms and the body, within the existing lifetime rules. A binding must not grant mutation permission absent from the matched source. Loop iterations must not reuse a binding from a previous successful condition check.

## Design details

### Parentheses protect existing programs

An existing condition such as `if let false = enabled and authorized:` matches the result of the entire boolean expression. Treating the `and` as a chain separator would change its behavior. The new parenthesized condition form creates an explicit boundary without changing that existing source.

Parentheses here group a statement condition, not a reusable expression producing a boolean value. Writing `accepted = (let Some(value) = candidate and value > 0)` remains invalid.

### Patterns compose independently

Tuple, constructor, record, and alternation patterns keep their existing meaning. RFC 139 sequence patterns would participate through the same pattern interface if adopted. The chain introduces no constructor shorthand or special treatment of Option and Result.

### Diagnostics and formatting

Diagnostics should identify the particular failed typing obligation, invalid binding use, or duplicate name rather than reporting the entire chain as one malformed boolean expression. References to a binding after its condition should explain its success-body scope.

The formatter should retain a compact parenthesized form for short chains and use one term per line for long chains, with `and` beginning continuation terms. It must preserve grouping that distinguishes a term initializer from the chain itself.

## Alternatives considered

- **Keep nested `if let`.** Correct and still readable for a single extraction, but repeated conditional layers add indentation without describing additional actions.
- **Use exiting fallback bindings.** Appropriate for prerequisites, but changes cases that should continue after failed extraction unless authors introduce another control-flow wrapper.
- **Recognize chains in every existing `if let`.** More compact, but can reinterpret an existing boolean initializer. Required parentheses preserve compatibility.
- **Allow bindings in every boolean expression.** Makes binding availability difficult to explain across assignments, negation, and alternatives. Statement conditions provide a bounded scope and explicit success body.
- **Allow `or` between binding terms.** Requires additional rules for names introduced by different successful alternatives. Pattern alternation already serves alternatives sharing one matched value; cross-expression binding alternatives are deferred.

## Drawbacks

- Adds a distinct parenthesized condition form that authors must learn alongside single-pattern conditions.
- Long chains can still become difficult to read; named helpers remain useful when checks represent a reusable concept.
- Failed prefixes require careful ownership and narrowing analysis.
- Preserving single-arm behavior means some conditional workflows still require `match`.

## Layers affected

- **Parser / AST:** parenthesized statement-condition chains with distinct binding and expression terms.
- **Typechecker / Symbol resolution:** term-local checking, forward binding visibility, narrowing, duplicate-name diagnostics, and success-body scope.
- **Ownership analysis / IR lowering:** short-circuit branches, failed-prefix ownership, temporary lifetimes, and loop condition reevaluation.
- **Emission:** evaluation order and branch behavior equivalent to the authored chain.
- **Formatter:** parenthesized conditions and term-aligned continuation lines.
- **LSP / Tooling:** precise completion scopes, binding navigation, and term-specific diagnostics.
- **Documentation:** conditional inspection examples distinguished from exiting prerequisite extraction.

## Design Decisions

- Pattern-bearing chains require outer parentheses; existing unparenthesized single-pattern conditions retain their initializer parsing.
- Terms combine through top-level `and`, evaluated once each from left to right.
- Successful bindings are available to later terms and the body, with no escape into subsequent statements.
- Pattern-bearing `if` statements retain RFC 049's single-arm rule. Adding `else` or `elif` requires separate design work.
- Earlier side effects and ownership transfers are not rolled back when a later term fails.
