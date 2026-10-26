# RFC 134: Pattern bindings with exiting fallbacks

- **Status:** Draft
- **Created:** 2026-10-11
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 029 (union types and narrowing)
    - RFC 049 (`if let` and `while let` pattern control flow)
    - RFC 070 (Result combinators)
    - RFC 071 (pattern alternation)
    - RFC 129 (binding declarations and mutation permissions)
    - RFC 131 (first-class error declarations)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Introduce a statement that matches a value, binds the successful pattern into the surrounding block, and executes an exiting fallback when the pattern does not match: `let PATTERN = VALUE` followed by a separate `else:` block. The fallback must prevent execution from reaching the statements after the binding. This keeps subsequent work flat while preserving explicit handling of missing values, unexpected variants, and other unmatched shapes. Existing defaulting combinators remain the preferred surface when the task is simply choosing a value.

## Motivation

An operation often needs to extract a payload before doing useful work. A full `match` pushes that work into an arm, even when the other arm immediately returns. Repeating this pattern for several prerequisites creates indentation that describes extraction rather than the operation itself.

For example, mapping a list parameter's representation currently needs a branch around all uses of its shape:

```incan
pub def list_parameter_type(ty: PlanType, mutable: bool) -> PlanType:
    match list_shape(ty):
        Some((leaf, depth)) =>
            if mutable:
                return PlanType.ListMutRef(leaf, depth)
            return PlanType.ListRef(leaf, depth)
        None => return ty
```

Here `list_shape` is an application helper returning an optional pair. The important work is choosing the representation from `leaf`, `depth`, and `mutable`. The `None` case simply preserves the original type.

RFC 049's `if let` supports opportunistic extraction, but its bindings are scoped to its success body. It therefore retains the same indentation for this example. A binding whose unsuccessful path exits can safely make the extracted names available to the following statements.

## Goals

- Bind successful patterns into the remainder of the surrounding block.
- Keep meaningful work flat after checking its prerequisites.
- Make unmatched behavior explicit and statically prevent use of uninitialized bindings.
- Reuse existing pattern typing, matching, and ownership rules.
- Work with Option, Result, ordinary enums, and other existing pattern forms.
- Compose with `?`, Result combinators, and proposed typed errors without changing their behavior.

## Non-Goals

- Replacing `match`, `if let`, or `while let`.
- Introducing exceptions, changing `raise`, or implicitly panicking on an unmatched pattern.
- Supplying a replacement payload from the fallback and then continuing.
- Adding Option-to-Result conversion methods or renaming existing combinators.
- Introducing pattern guards or boolean chains in this statement.
- Defining new error declarations, error conversion, or cause traversal.

## Guide-level explanation

The examples using `let … else` describe proposed behavior. Application types and helpers retain their surrounding program's definitions; the typed error example additionally uses proposed RFC 131 behavior.

### Extract a prerequisite, then continue

The list parameter example becomes:

```incan
pub def list_parameter_type(ty: PlanType, mutable: bool) -> PlanType:
    let Some((leaf, depth)) = list_shape(ty)
    else:
        return ty

    if mutable:
        return PlanType.ListMutRef(leaf, depth)
    return PlanType.ListRef(leaf, depth)
```

Read this as: extract the list shape; otherwise return the original type. After the statement, `leaf` and `depth` are available because the unmatched path cannot reach that code. The fallback does not assign `ty` to either name.

### Remove a match around a loop body

Consider this fragment inside a function returning a Result. Each item must have a single value before it can be lowered:

```incan
for item in elements:
    match item.as_one():
        Some(value) =>
            argument = value_operand(builder, value, span)?
            reference = list_borrow(builder, output, true, span)?
            unit = temporary(builder, PlanType.Unit, span)
            list_call(
                builder,
                "append_list",
                element,
                [PlanType.ListMutRef(leaf, depth), element],
                PlanType.Unit,
                [reference, argument],
                unit,
                span,
            )
        None => return Err("unsupported Body IR list spread")
```

The same operation can place rejection before the work it protects:

```incan
for item in elements:
    let Some(value) = item.as_one()
    else:
        return Err("unsupported Body IR list spread")

    argument = value_operand(builder, value, span)?
    reference = list_borrow(builder, output, true, span)?
    unit = temporary(builder, PlanType.Unit, span)
    list_call(
        builder,
        "append_list",
        element,
        [PlanType.ListMutRef(leaf, depth), element],
        PlanType.Unit,
        [reference, argument],
        unit,
        span,
    )
```

The existing fallible calls still use `?`. The new statement handles only the optional shape returned by `as_one`; it adds no conversion or propagation behavior of its own.

### Skip an item or stop a loop

An unsuccessful extraction can skip the current iteration when skipping is the intended behavior:

```incan
def print_present(values: list[Option[str]]) -> None:
    for candidate in values:
        let Some(text) = candidate
        else:
            continue

        println(text)
```

A fallback containing `break` instead stops the enclosing loop. Neither exit makes `text` available after the loop: the binding belongs to the loop body's block.

### Combine extraction with typed errors

Suppose an application helper `parse_count(text: str)` returns `Result[int, ParseError]`. With RFC 131, missing input and malformed input can remain distinct failures:

```incan
error ParseError:
    InvalidInteger(text: str) = f"'{text}' is not an integer."

error InputError:
    Missing(field: str) = f"Required field '{field}' is missing."

    Invalid(field: str, original: ParseError):
        message = f"Invalid value for '{field}'."
        caused_by = original

    OutOfRange(field: str, value: int) = f"'{field}' must be positive; received {value}."

def require_count(candidate: Option[str]) -> Result[int, InputError]:
    let Some(text) = candidate
    else:
        return Err(InputError.Missing(field="count"))

    count = parse_count(text).map_err(
        InputError.Invalid,
        field="count",
    )?

    if count < 1:
        return Err(InputError.OutOfRange(
            field="count",
            value=count,
        ))

    return Ok(count)
```

The binding requires a value. RFC 131's proposed constructor overload for `map_err` translates parsing failures, retaining the original error and its declared causal relationship. The ordinary range check constructs a separate error. None of these operations implies another: a successful pattern does not validate the text, and an unsuccessful pattern does not manufacture an upstream error.

### Prefer a defaulting combinator for value selection

A function that only chooses between a successful value and a default already has a direct expression:

```incan
def port_or_default(result: Result[int, str], fallback: int) -> int:
    return result.unwrap_or(fallback)
```

Using an exiting pattern binding here would add an extraction, an early return, and a final return without clarifying the operation. The proposed statement is useful when extracted values are prerequisites for subsequent work, rather than an alternative spelling for every default.

## Reference-level explanation

### Syntax

The required form is a statement whose fallback begins on a separate logical line, aligned with `let`:

```text
pattern_binding ::= "let" pattern "=" expr NEWLINE
                    "else" ":" indented_block
```

Existing line continuation rules must apply within the initializer expression. Blank lines and comments must not detach the fallback from its binding. The fallback is mandatory, and its body must use the ordinary indented block syntax. An inline `else` clause must be rejected, with or without a colon. The separate aligned `else:` block is the only supported spelling.

The statement must accept the existing pattern forms used by `match`, including nested tuple, variant, record, type, wildcard, binding, literal, and alternation patterns. Alternation must follow RFC 071: every alternative must bind the same names with the same types, and alternatives must be tried in order. Guards must not be accepted in this construct.

### Evaluation and matching

- The initializer must be evaluated exactly once.
- Pattern matching must use the same matching and type rules as an equivalent `match` arm.
- On success, execution must skip the fallback and continue after the statement with the pattern's bindings in scope.
- On failure, execution must run the fallback without introducing any of the pattern's bindings.
- The statement must not yield a value or implicitly convert an unmatched value into a panic, default, or error.
- An irrefutable pattern may be used, but must still include a fallback. Ordinary unreachable-code diagnostics may identify that fallback as unreachable.

### Scope and typing

Each bound name must have the type it would have in the equivalent `match` pattern. The names must be visible only in the remainder of the immediately enclosing block, including nested blocks within that remainder. They must not escape a loop body, conditional body, or other lexical block merely because its fallback exits.

The initializer and fallback must resolve names in the environment preceding the declaration. A same-named existing binding remains visible there; newly extracted names become visible only after a successful match. Name collisions, protected names, and shadowing diagnostics must follow the language's binding declaration rules. This RFC does not introduce an additional shadowing policy.

The fallback's return expressions must satisfy the containing function's return contract. A fallback's `break` or `continue` must have a valid loop target under existing control-flow rules. Pattern incompatibility, incorrect payload arity, private field access, and inconsistent alternatives must be rejected as they are in existing pattern positions.

### The fallback must exit

Every reachable path through the fallback must prevent execution from reaching the statements after the binding without first successfully executing the binding again. A return from the containing function qualifies. A break out of the loop containing the binding qualifies. A continue of that loop qualifies because the next iteration must execute the binding anew before reaching its following statements.

Merely placing an exit somewhere inside the fallback is insufficient. For example, this must be rejected:

```incan
def require_text(candidate: Option[str]) -> str:
    let Some(text) = candidate
    else:
        println("Missing text")

    return text  # rejected: the fallback can reach this without binding text
```

A conditional fallback is valid only if every reachable branch exits appropriately. Exiting a loop nested inside the fallback does not by itself qualify: execution could then leave that inner loop and reach the uninitialized binding's following statements. Returning from a callback defined or called inside the fallback likewise does not return from the containing function.

The check must use the language's control-flow analysis. An expression already known by the compiler never to return may satisfy it; this RFC does not introduce a new public never type or require proving that arbitrary loops or recursive functions never terminate. A call that happens not to return at runtime must not be accepted as an exit without such static knowledge.

### Ownership and mutation

Matching and extraction must preserve the ownership, borrowing, and mutation permissions of the equivalent existing pattern match. The statement must not introduce an implicit clone or silently grant write permission through an immutable source. Pattern bindings themselves remain immutable under existing pattern rules; changes through their payloads remain subject to the matched source's permissions.

A failed pattern must not commit partial payload moves or expose partial bindings. An existing subject may therefore be used in the fallback when ordinary ownership rules permit it. A temporary initializer must remain alive for as long as successful bindings borrow its contents; compiler-generated storage must not cause an otherwise valid binding to dangle.

If RFC 129 changes the general binding or borrowing contract, this construct must follow that contract consistently with other pattern positions. It must not establish a separate ownership regime.

### Diagnostics

An invalid fallback diagnostic must identify the path that can reach the following statements without a successful binding and explain that the fallback must exit. Diagnostics for missing `else`, bad indentation, unavailable names, and invalid loop exits must refer to the authored statement. Existing diagnostic policies apply to unreachable fallback bodies and unused bindings.

## Design details

### The successful work stays in the surrounding block

The conceptual control flow is an ordinary match with an exiting unmatched arm. Its distinguishing feature is the scope of successful bindings: subsequent statements can use them without occupying a match arm. This is not an expression that assigns either a payload or the fallback's result.

### A separate fallback block is the only spelling

The newline and `else:` make the unmatched behavior visible before the subsequent work. A colon consistently introduces an indented body. Long typed error constructors, logging, and conditional recovery can grow inside that body without changing the binding's shape.

Compactness alone is not the goal. An inline `return` can make an assignment-shaped statement misleading, especially when the whole function merely selects a default. Examples should demonstrate a genuine prerequisite and enough subsequent work to justify extraction.

### Existing constructs retain their roles

- `match` describes several meaningful alternatives or produces a value from branches.
- `if let` performs optional work inside a successful match's body.
- `while let` repeats work while a pattern continues to match.
- `let … else` requires a shape before continuing in the current block.
- `?` propagates an existing compatible failure.
- `unwrap_or` chooses an existing successful value or a supplied default.
- Result combinators transform, observe, chain, or recover according to their existing contracts.

The proposed binding works independently of RFC 131. A fallback may return an existing string error, a declared typed error, another permitted return value, or exit a loop. Typed error construction and provenance remain governed by their own contracts.

## Alternatives considered

- **Keep using full matches.** Fully expressive and still appropriate for meaningful alternatives, but introduces an arm around all subsequent work when the unmatched case immediately exits.
- **Use `if let` for every extraction.** Useful for optional work, but success-body scoping retains the nesting this proposal addresses. Extending `if let` bindings beyond that body would change its existing meaning.
- **Use a match expression to produce the payload.** Already useful for value selection, but repeated extraction of nested payloads still needs an explicit successful arm and an exiting fallback. The proposed statement states that relationship directly.
- **Use only combinators.** Defaults, transformations, and propagation already have appropriate combinators. Arbitrary enum patterns, several extracted values, and loop exits are less naturally expressed as callback pipelines.
- **Allow the fallback to supply replacement bindings.** This requires a different value-selection contract and agreement about which values initialize every bound name. It is outside the meaning of an exiting fallback.
- **Use an inline exiting fallback, with or without a colon.** Rejected: the inline return makes the assignment-shaped extraction harder to read and introduces a second form. The separate aligned `else:` block keeps the exit visible and permits logging, conditional handling, or longer constructors without changing the statement shape.
- **Implicitly panic when a pattern does not match.** This would conceal expected unmatched behavior and change extraction into a runtime assertion. The fallback remains explicit.

## Drawbacks

- Adds another pattern-bearing statement alongside `match`, `if let`, and `while let`.
- Begins like an assignment even though its unmatched path exits; readers must understand the entire statement.
- Makes borrowing from temporary initializers and the scope of successful bindings additional cases for compiler and tooling correctness.
- Can be overused for simple value selection, producing more control flow than an existing defaulting expression needs.
- A fallback accepting every unmatched shape does not require authors to reconsider each newly added enum variant; use exhaustive `match` when that review pressure is desired.

## Layers affected

- **Parser / AST:** a pattern-binding statement with a separate required fallback block.
- **Typechecker / Symbol resolution:** pattern typing, success-only binding introduction into the surrounding block, shadowing, and fallback exit analysis.
- **Ownership analysis / IR lowering:** branch-aware extraction, failed-pattern preservation, temporary lifetimes, and existing mutation permissions.
- **Emission:** control flow and storage that preserve authored binding scope and exits.
- **Formatter:** consistent placement and indentation of the required `else:` block.
- **LSP / Tooling:** binding scopes, completions, navigation, and diagnostics across success and fallback paths.
- **Documentation:** examples distinguishing prerequisite extraction from value selection and ordinary conditional matching.

## Design Decisions

- An exiting pattern binding requires a separate `else:` block aligned with `let`. Inline fallbacks are rejected with or without a colon. This is the only spelling, allowing fallback bodies to grow without changing the binding's structure.
- Every reachable fallback path must prevent execution from reaching subsequent statements without a successful binding. Existing return, valid loop exits, and statically known nonreturning behavior follow the ordinary control-flow rules.
- The statement extracts prerequisites for subsequent work. Existing combinators remain the preferred surface for simply choosing a default value; the statement introduces no replacement-value fallback or implicit failure conversion.
