# RFC 136: Chained comparisons

- **Status:** Planned
- **Created:** 2026-10-11
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 000 (expressions and comparison operators)
    - RFC 028 (operator overloading)
    - RFC 068 (membership and identity syntax)
    - RFC 110 (proposed comparison traits)
    - RFC 111 (proof-aware contracts)
    - RFC 135 (Option defaults using existing `or` precedence)
- **Issue:** [#2465](https://github.com/encero-systems/incan/issues/2465)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Support chained comparisons such as `start <= value < end`. A chain compares each adjacent pair, evaluates reached operands once from left to right, and stops at the first false comparison. Its result is a Boolean. Existing pairwise comparison rules continue to determine which operand types are compatible and which comparison implementations are called. Parentheses preserve explicit binary grouping.

## Motivation

Range checks repeat the value being checked:

```incan
if entry.key >= start and entry.key < end:
    out.append(entry.key)
```

The intended interval is more visible as:

```incan
if start <= entry.key < end:
    out.append(entry.key)
```

Datetime validation repeats the same structure for hours, minutes, seconds, and nanoseconds. Chaining states the valid range directly, including whether each endpoint is inclusive. It also avoids repeating a computed middle operand when that computation has a cost or observable effects.

Current comparison syntax groups successive operators as ordinary binary expressions. A numeric chain consequently attempts to compare a Boolean intermediate result with a number and is rejected. This proposal deliberately defines comparison chains as adjacent comparisons instead.

## Goals

- Express ranges and adjacent comparison relationships without repeating operands.
- Evaluate every reached operand exactly once in source order.
- Short-circuit before later operands and comparisons when an earlier comparison is false.
- Preserve existing comparison typing, overload selection, and numeric rules per adjacent pair.
- Make grouping, error propagation, and compatibility changes explicit.

## Non-Goals

- Adding comparison implementations or changing total and partial ordering rules.
- Converting arbitrary comparison results to Boolean through truthiness.
- Defining an all-distinct operation or comparing every pair of operands.
- Inferring validation guarantees from arbitrary user-defined comparisons.
- Reinterpreting explicitly parenthesized binary comparisons.
- Defining new syntax for ranges, patterns, or constrained types.

## Guide-level explanation

The chained forms in this RFC describe proposed behavior.

### Check a range

```incan
def valid_hour(hour: int) -> bool:
    return 0 <= hour < 24

def valid_port(port: int) -> bool:
    return 1 <= port <= 65535
```

The first expression includes zero and excludes 24. The second includes both endpoints. These are ordinary Boolean expressions, usable in assignments, returns, conditions, and comprehension filters.

An existing rejection check can describe the valid range and negate it:

```incan
if not 0 <= hour < _HOURS_PER_DAY:
    return Err(DateTimeError(detail="hour must be in 0..23"))
```

### Filter a collection

An entry-range helper can keep its existing loop:

```incan
for entry in entries:
    if start <= entry.key < end:
        out.append(entry.key)
```

The same condition fits an existing comprehension:

```incan
return [entry.key for entry in entries if start <= entry.key < end]
```

Chaining is independent of whether the author chooses a loop or comprehension.

### Evaluate a computed value once

Suppose `read_temperature()` returns an integer:

```incan
safe = minimum <= read_temperature() <= maximum
```

The read happens once. Expanding this to two calls joined by `and` would not be equivalent: the calls could return different values or perform effects twice. An explicit temporary preserves the intended behavior:

```incan
temperature = read_temperature()
safe = minimum <= temperature and temperature <= maximum
```

The chained form provides that single evaluation without an authored temporary.

### Stop before unnecessary work

Suppose `first()`, `middle()`, and `last()` return integers:

```incan
ordered = first() < middle() < last()
```

The program evaluates `first()`, then `middle()`, then compares them. If that comparison is false, `last()` is never called. Otherwise it is called once and compared with the retained middle value.

The same rule applies to explicit propagation:

```incan
accepted = minimum <= read_value()? <= read_limit()?
```

Assuming the helpers return compatible Results and the containing function permits their errors, a failure in `read_value()` propagates immediately. `read_limit()` runs only if the first comparison succeeds; its failure then propagates normally.

### Read equality chains as adjacent relationships

```incan
same = first == second == third
adjacent_differ = first != second != third
```

The first requires `first == second` and `second == third`. The second requires only `first != second` and `second != third`; it does not require `first != third`. For example, `1 != 2 != 1` is true.

### Mix comparison operators

Membership and identity operators belong to the same chain family as ordering and equality. For example:

```incan
def supported_worker_count(count: int) -> bool:
    return 0 < count in [0, 1, 2, 4, 8]
```

This checks `0 < count` and then, only if true, `count in [0, 1, 2, 4, 8]`. It accepts a positive count belonging to the listed supported sizes; zero is excluded. The middle operand is evaluated once. Every adjacent pair must independently satisfy its operator's existing typing and ownership rules, including any restrictions on identity comparisons. Mixing operators must not grant capabilities that a pairwise comparison lacks.

### Preserve explicit grouping

```incan
adjacent = first == second == third
grouped = (first == second) == third
```

The second expression compares the Boolean result of the parenthesized comparison with `third`, under existing binary typing rules. For Boolean operands, these expressions can produce different results. Parentheses are therefore semantically significant.

## Reference-level explanation

### Syntax and grouping

A comparison chain consists of at least two comparison operators and their intervening operands. The chain operator family must include `<`, `<=`, `>`, `>=`, `==`, `!=`, `in`, `not in`, `is`, and `is not`. Each token sequence must retain its existing pairwise meaning. Mixed chains must use the same adjacent-pair evaluation and typing rules as ordering-only chains.

At the existing comparison precedence, an unparenthesized sequence of comparison operators must be treated as one chain. Arithmetic, calls, indexing, and postfix `?` must form operands according to their existing higher precedence. Boolean `and`, Boolean or Option `or`, and prefix `not` must combine with the completed comparison according to their existing lower precedence.

An explicitly parenthesized comparison must remain one operand rather than being merged into an outer chain. A single comparison must retain its existing behavior. Operators outside the comparison family must not become chain links.

### Evaluation contract

For a chain `e0 op0 e1 op1 e2 ... opN eN+1`:

1. The program must evaluate `e0`, then `e1`, exactly once each.
2. It must compare those values using `op0`.
3. If that comparison is false, it must produce false without evaluating any later operand or comparison.
4. Otherwise it must retain the already evaluated right operand, evaluate the next operand once, and compare the adjacent pair using the next operator.
5. It must repeat until a comparison is false or all comparisons succeed. Only the latter case produces true.

The comparison implementations themselves must be invoked once per reached link, in order. No operand may be duplicated, reordered, or eagerly evaluated to simplify lowering. A failure propagated by a reached `?` must retain its ordinary meaning; an unreached propagation expression must not execute.

Retaining an operand means retaining the value or checked borrow produced by its evaluation. It does not imply a deep snapshot of a mutable object, an implicit clone, or re-reading its original expression.

### Typing and comparison behavior

Every adjacent pair must satisfy the existing rules for its comparison operator. The result of each comparison must be Boolean under the ordinary operator contract, and the chain's type must be `bool`. The typechecker must check every link even when an earlier link appears statically false.

An interior operand participates in two comparisons. Both uses must be compatible with its type and ownership permissions. The chain must not require all operand types to be identical, infer a new common type for the whole chain, or compare an earlier Boolean comparison result with a later operand.

Existing numeric conversions, user-defined comparison dispatch, identity restrictions, and membership protocols must apply independently to each pair. A chain must not add ordering to a type lacking it. Floating-point NaN and other partial-order behavior must follow each existing comparison; the chain must not reinterpret false as an ordering or validation error.

### Ownership and mutation

The checker must preserve the ordinary borrowing and mutation rules across adjacent comparisons. An interior operand must remain valid through its final reached use. If a selected comparison consumes that operand and leaves it unavailable for the next link, the chain must be rejected unless the existing contract already permits both uses.

Later operand evaluation must not invalidate an earlier operand or borrow that the next comparison still needs. Conflicts must receive source diagnostics; an implicit clone must not repair them. Temporary storage must preserve reached operands for their required lifetimes without extending observable resource lifetimes beyond what the comparison requires.

### Compatibility and diagnostics

The new grouping must apply consistently to all accepted chains in the chosen operator family. The compiler must not retain left-associated binary behavior merely because the old grouping happened to typecheck.

This can change previously accepted Boolean comparison sequences. For example, `true == false == false` becomes false because its first adjacent comparison fails; `(true == false) == false` remains true. Migration guidance must explain how parentheses preserve intentional binary grouping.

An incompatible-link diagnostic must identify the particular operator and its adjacent operand types. A conflict involving reuse of an interior operand must identify both uses. Diagnostics must describe comparisons in authored source rather than compiler-generated temporary names.

## Design details

### More than textual sugar for `and`

For pure values, `a < b < c` has the truth condition of `a < b and b < c`. For arbitrary expressions, duplicating `b` changes evaluation. The defining contract is single evaluation plus short-circuit adjacent comparisons, not textual substitution.

### Comparison traits remain independent

RFC 028 and any accepted comparison-trait extensions determine how a pair is compared. This RFC controls how multiple pairwise comparisons compose. It does not require RFC 110's proposed `Comparable` trait and must not bypass existing overload resolution.

Refactoring `value >= start` into `start <= value` assumes those two operators express inverse relationships for the selected types. That holds for ordinary consistent ordering, but chaining does not establish it for arbitrary user-defined hooks. Tooling must not reverse an overloaded comparison merely to produce a visually ascending chain unless the equivalence is known.

### Proofs need the selected operator's semantics

A built-in integer range check may supply useful narrowing or validation facts where the checker supports them. An identically spelled chain over a user-defined comparison is not automatically such a proof. RFC 111 remains responsible for the evidence needed to establish those facts.

## Alternatives considered

- **Keep explicit comparisons joined by `and`.** Already expressive and appropriate when separate names clarify the values, but repeats operands and requires authored temporaries to preserve single evaluation of calls.
- **Use an interval-membership helper.** Useful for some domains, but adds a library abstraction to ordinary comparisons and does not cover equality or longer adjacent relationships.
- **Treat a chain as left-associated binary comparisons.** This is the current grouping, but does not express the intended numeric range and can accidentally compare Boolean results.
- **Expand directly into repeated expressions.** Rejected because it duplicates calls, effects, and failure boundaries.
- **Require every operand to have the same type.** Unnecessary when each adjacent pair already has a valid comparison contract.

## Drawbacks

- Changes grouping for some previously accepted Boolean comparison sequences.
- Interior operands have two uses, which can expose ownership conflicts that independent comparisons do not.
- Long or direction-changing chains can be harder to understand than named intermediate conditions.
- Equality and inequality chains can be misread as comparing every pair; teaching examples must make adjacency explicit.

## Layers affected

- **Parser / language surface registry:** comparison-chain grouping and interaction with existing precedence and parentheses.
- **Typechecker:** per-link comparison resolution, Boolean results, and diagnostics.
- **Ownership analysis / IR lowering:** single evaluation, retained interior operands, and short-circuit control flow.
- **Emission:** evaluation order, propagation boundaries, and temporary lifetimes.
- **Formatter / LSP:** stable chain formatting, grouping, and diagnostics on individual links.
- **Documentation / compatibility tests:** range examples and migration from intentionally parenthesized Boolean comparisons.

## Design Decisions

- Include the full comparison family: ordering, equality, membership, and identity operators may form mixed chains. Each adjacent pair retains its existing typing, ownership, and operator semantics, without adding membership or identity capabilities to unsupported types.
- Every reached operand is evaluated once from left to right, and a false comparison stops evaluation of subsequent links. Explicit parentheses preserve intentional binary grouping.
- The shared operator family and chaining behavior follow [Python's comparison syntax](https://docs.python.org/3/reference/expressions.html#comparisons). Incan retains its own Boolean-result requirement and checked pairwise comparison contracts.
