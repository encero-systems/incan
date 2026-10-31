# RFC 139: Sequence patterns

- **Status:** Planned
- **Created:** 2026-10-11
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 029 (union types and narrowing)
    - RFC 049 (`if let` and `while let` pattern control flow)
    - RFC 068 (collection protocol hooks)
    - RFC 071 (pattern alternation)
    - RFC 129 (binding declarations and mutation permissions)
    - RFC 134 (pattern bindings with exiting fallbacks)
    - RFC 138 (chained pattern conditions)
- **Issue:** [#2466](https://github.com/encero-systems/incan/issues/2466)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Introduce bracketed sequence patterns for exact-length matching of built-in lists and frozen lists. A pattern such as `[name, alias, role, digest]` checks the length and matches each element with an ordinary nested pattern, making named values available only after the whole match succeeds. Sequence patterns participate in existing pattern-bearing constructs and proposed fallback bindings without changing tuple patterns, consuming arbitrary iterators, or introducing implicit copies. Capturing a variable-length remainder is left for separate design work.

## Motivation

Structured input often arrives as homogeneous lists with an externally defined row shape. Decoding currently repeats a length check followed by indexed access:

```incan
if len(row) != 4:
    return Err(CompilationError(fields=path))
name = string(row[0], path + ["0"])?
```

Subsequent checks continue using `row[1]`, `row[2]`, and `row[3]`. A reader must reconstruct the positional roles, and the compiler has no pattern-level relationship between the checked width and those accesses.

An exact-length sequence pattern expresses the shape and names its positions together. It also lets ordinary matching describe cases such as no candidates, exactly one candidate, and several candidates without separate length and indexing operations.

## Goals

- Match list shape and element patterns in one construct.
- Give positional input meaningful local names.
- Support empty and exact-length list patterns.
- Compose with nested constructors, literals, tuples, and pattern alternation.
- Preserve matching, binding scope, ownership, and frozen-container permissions.
- Require exhaustive handling of remaining lengths when a full match needs it.

## Non-Goals

- Capturing or discarding a variable-length remainder through starred patterns.
- Destructuring assignment to an unchecked list length.
- Implicit conversion between lists and tuples.
- Matching arbitrary iterators, generators, strings, bytes, or custom indexing protocols.
- Adding collection slicing, rest parameters, or collection literal spreading.
- Introducing deep freezing or changing frozen-list semantics.

## Guide-level explanation

Bracketed patterns below describe proposed behavior. Fallback bindings additionally use proposed RFC 134 syntax. Application error declarations and helpers retain their surrounding program's definitions.

### Decode a row by its shape

```incan
let [name_value, alias_value, role_value, digest_value] = row
else:
    return Err(CompilationError(fields=path))

name = string(name_value, path + ["0"])?
logical(name, path + ["0"])?
if not alias_value.is_null():
    nonempty(string(alias_value, path + ["1"])?, path + ["1"])?
nonempty(string(role_value, path + ["2"])?, path + ["2"])?
digest(string(digest_value, path + ["3"])?, path + ["3"])?
```

The first statement requires exactly four elements. It does not parse a JSON string or validate a digest; those remain the explicit responsibilities of the subsequent helpers. No indexed access is needed to connect a field's role to its value.

### Select an empty, single, or larger result

```incan
def select_unique(candidates: list[str]) -> Option[str]:
    match candidates:
        [] => return None
        [candidate] => return Some(candidate)
        _ => return None
```

`[candidate]` means exactly one element. It does not silently choose the first item from a longer list. The wildcard covers every remaining length.

### Combine element literals with bindings

```incan
def parse_assignment(parts: list[str]) -> Option[tuple[str, str]]:
    match parts:
        [name, "=", value] => return Some((name, value))
        _ => return None
```

This requires three elements and a literal equals sign in the middle. A three-element list with another separator does not match.

### Nest patterns in either direction

```incan
def single_success(results: list[Result[int, str]]) -> Option[int]:
    match results:
        [Ok(value)] => return Some(value)
        _ => return None
```

A sequence can also appear inside a constructor pattern:

```incan
if let Some([first, second]) = optional_pair:
    println(f"{first}, {second}")
```

Here `optional_pair` has type `Option[list[str]]`. The body runs only for a present list containing exactly two elements.

### Keep frozen-list permissions

```incan
def join_pair(labels: FrozenList[str]) -> Option[str]:
    match labels:
        [first, second] => return Some(f"{first}: {second}")
        _ => return None
```

The bracketed pattern checks the same shape as it does for a list. It does not make `labels` mutable, and it does not add deep immutability to its elements.

## Reference-level explanation

### Syntax and target types

A sequence pattern must use brackets containing zero or more comma-separated patterns. A trailing comma may appear after the final element. Empty brackets must match an empty supported sequence.

The supported target types are built-in `list[T]` and `FrozenList[T]`. For a pattern with `N` elements, matching must first establish that the target has exactly `N` elements, then match each element against its corresponding nested pattern. Every nested element pattern must be checked against `T` using ordinary pattern typing rules.

Tuple patterns must retain their parenthesized syntax and tuple typing. Bracketed patterns must not coerce tuple values to lists. Iterability, length, or indexing methods on a custom type must not implicitly make it eligible for a sequence pattern.

A starred member, including `*rest` and `*_`, must be rejected under this RFC. Those spellings must not silently acquire collection-literal spreading or rest-parameter semantics.

### Matching behavior

The scrutinee must be evaluated according to the enclosing construct's existing once-only evaluation rule. A length mismatch must be an ordinary pattern non-match. It must not panic, index outside the sequence, or partially initialize visible bindings.

After a length match, element patterns must be considered in source order and stop at the first non-match. Pattern inspection must not call arbitrary user indexing or iteration hooks. Nested literal and constructor comparisons must retain their ordinary matching behavior.

All bindings must become available only when the complete sequence pattern succeeds. Their scopes must be those of the enclosing match arm, conditional body, or fallback binding. Duplicate bindings and pattern alternation must obey the existing name and type compatibility rules.

### Pattern-bearing contexts

Sequence patterns must be available wherever existing refutable patterns are accepted, including `match`, `if let`, and `while let`. They must also participate in RFC 134 fallback bindings and RFC 138 conditional chains if those proposals are adopted.

An exact-length list pattern is refutable even when every element subpattern is a binding. It must not become a legal unchecked ordinary assignment or loop binding merely because tuple destructuring is legal there. Contexts requiring irrefutable patterns must reject it unless a separate language rule explicitly provides a checked failure path.

### Exhaustiveness

Exhaustiveness checking must account for both sequence length and nested element coverage. A finite collection of exact-length patterns cannot cover all possible lengths of a general list or frozen list. A full match on such a sequence must therefore retain an applicable catch-all pattern, unless surrounding type facts already restrict the values sufficiently.

A pattern such as `[Ok(value)]` must not be treated as covering every one-element list of Results; an `Err` element remains unmatched. Alternations of sequence patterns must use the same binding agreement rules as other pattern alternations.

### Ownership and mutation

Successful extraction must follow the existing ownership and borrowing rules for the target and its elements. A sequence pattern must not introduce an implicit clone, allocate a replacement list, or remove elements from the matched list merely to bind them.

Failed matching must not leave partial moves that invalidate the source for later match arms or fallback inspection. The compiler must check the complete match before committing any ownership transfer required by the successful path.

Borrowed element access must remain tied to its source lifetime. Mutation permissions must follow the matched place and existing binding rules. Matching a frozen list must not grant permission to replace its elements or mutate its structure. The frozen container's existing rules, rather than this pattern syntax, determine which operations on an element value remain permitted.

## Design details

### Exact shape before remainder capture

Exact-length patterns solve a concrete decoding problem without deciding whether a captured remainder is a new owned list, a borrowed view, or a collection-specific value. A later proposal may define `[first, *remaining]`, including its type, ownership, and allocation behavior. This RFC does not reserve an implicit copying policy for that extension.

### Lists and tuples remain different

`[left, right]` checks a runtime length on a homogeneous sequence. `(left, right)` decomposes a tuple whose shape and position-specific types are already part of its type. Sharing nested pattern rules does not merge those collection contracts.

### Diagnostics

A bracketed pattern on an unsupported target should identify the target type and supported sequence families. An unchecked destructuring context should explain that an exact-length list pattern can fail and point toward conditional matching or an exiting fallback.

Diagnostics for nested mismatches should identify the affected element pattern. A non-exhaustive match should explain whether another length or an uncovered element shape remains possible.

### Compatibility

This is additive pattern syntax. Existing list expressions, list comprehensions, collection spreads, indexing, and tuple patterns must keep their existing parsing and behavior. No runtime collection representation or public serialized shape changes are required.

## Alternatives considered

- **Keep length checks and indexing.** Fully expressive, but separates shape validation from field naming and repeats positional information.
- **Use tuple patterns on lists.** Saves punctuation choices but hides the distinction between homogeneous runtime-sized lists and statically shaped tuples.
- **Convert lists to tuples before matching.** Requires an additional fallible shape conversion and creates another representation solely for extraction.
- **Match every Iterable.** Can consume generators, perform effects, or never finish determining a length. Built-in finite sequences provide a predictable matching contract.
- **Include starred captures immediately.** Useful for token streams, but their borrowing and allocation behavior require a separate decision. Exact-length decoding does not need to depend on that decision.

## Drawbacks

- Adds a pattern form whose apparent simplicity can conceal ownership restrictions on element extraction.
- Exact-length patterns cannot express a nonempty sequence of arbitrary length without another construct.
- Restricting supported sequence families means custom collection authors do not automatically receive the feature.
- Finite exact-length arms still require a catch-all for general list exhaustiveness.

## Layers affected

- **Parser / AST:** bracketed patterns distinguished from collection expressions.
- **Typechecker / Symbol resolution:** supported target types, element pattern typing, refutability, and binding scopes.
- **Exhaustiveness analysis:** length partitions combined with nested element coverage.
- **Ownership analysis / IR lowering:** checked-length access, success-only extraction, and failed-pattern preservation.
- **Emission:** length tests and element access without hidden allocation or copying.
- **Formatter:** bracketed patterns, trailing commas, and nested pattern layout.
- **LSP / Tooling:** element types, binding navigation, and refutability diagnostics.
- **Documentation:** sequence shape matching distinguished from tuple destructuring and collection spreading.

## Design Decisions

- Bracketed patterns target built-in lists and frozen lists; tuples keep tuple patterns.
- Every pattern specifies an exact length, including zero.
- Nested patterns retain their existing typing and matching behavior.
- Refutable sequence shapes require an enclosing construct with a defined non-match path.
- No starred remainder patterns, custom iterable matching, implicit clones, or replacement list allocation are introduced.
