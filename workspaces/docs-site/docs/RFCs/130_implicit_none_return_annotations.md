# RFC 130: Implicit `None` return annotations

- **Status:** Planned
- **Created:** 2026-10-08
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 000 (explicit function signature types)
    - RFC 044 (body-less abstract method declarations)
    - RFC 128 (shared callable signature model)
- **Issue:** [#2456](https://github.com/encero-systems/incan/issues/2456)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

A function or method declaration may omit its return annotation. Omission means exactly `-> None`, without inspecting the body or inferring a return type. Explicit `-> None` remains valid, and all other return types remain explicitly annotated.

## Motivation

Entry points, logging helpers, setters, and other operations that perform an effect repeatedly spell `-> None`. Allowing omission makes these declarations shorter while retaining a complete, statically known signature. Readers can determine the return type from the declaration alone.

## Goals

- Make `-> None` optional on function and method declarations.
- Preserve the same type checking, callable contract, and runtime behavior for both spellings.
- Explain accidental omissions when a body returns an incompatible value.

## Non-Goals

- Inferring return types from function bodies.
- Making parameter annotations optional or changing closure inference.
- Changing computed property syntax or function-type annotations.
- Changing the meaning of `None`, `Option`, `Result`, or existing return statements.

## Guide-level explanation

These declarations have the same signature and behavior:

```incan
def log(message: str):
    println(message)


def log_explicit(message: str) -> None:
    println(message)
```

A function that returns a useful value still needs an annotation:

```incan
def answer() -> int:
    return 42


def missing_annotation():
    return 42  # Error: the omitted annotation means None.
```

Methods use the same rule, including abstract trait declarations:

```incan
trait Flushable:
    def flush(self)
```

Here `flush` is abstract and returns `None`, exactly as if it declared `-> None`. Omission does not supply an implementation.

## Reference-level explanation

1. A `def` declaration without a return annotation must have the effective return type `None`. The compiler must establish this contract independently of the body and call sites.
2. The rule must apply to free functions and every method declaration that accepts `-> None`, including abstract methods, associated methods, trait-targeted methods, and declarations with decorators or supported modifiers.
3. Omitted and explicit `-> None` declarations must obey identical return checking. Falling through, bare `return`, and any return expression must retain their existing `-> None` rules. A value incompatible with `None` must be rejected; the compiler must not silently discard it or infer a different return type.
4. Callable compatibility, trait conformance, overload resolution, imported signatures, and runtime behavior must depend on the effective signature, not on whether `-> None` was written.
5. Any other return type must remain explicit. In particular, `Result[None, E]` and `Option[None]` must not be inferred from omission or from use of `?`. Generator return annotations must remain explicit under existing generator rules.
6. Existing body delimiters and abstract-method rules must remain unchanged. A body still requires `:`, and omission must not make a body-less declaration legal outside contexts that already permit it.

When an incompatible return value appears in a declaration with an omitted annotation, the diagnostic should identify the return expression and explain that the declaration defaults to `None`. It should suggest writing the intended return annotation, without automatically choosing one. Illustrative wording: “This function defaults to returning None because its return annotation is omitted; this expression has type int.”

## Design details

### Syntax and interactions

The optional declaration suffix is `["->" return_type]`. This changes only the return annotation on `def` declarations; parameters, generic bounds, receivers, trait targets, and bodies retain their existing syntax.

For an async declaration, omission has exactly the meaning of explicit `-> None` under the existing async rules. Decorators receive the same declared callable contract for either spelling and retain their existing transformation rules. Computed properties continue to require an explicit result type. Function types continue to spell their result, for example `(str) -> None`; closures retain their existing syntax and inference rules.

### Compatibility

This is an additive syntax change. Existing explicit annotations remain valid, and no source migration is required. Older compilers may reject the new spelling.

This RFC amends RFC 000's requirement for explicit return annotations: a function signature must still have a statically known return type, but a missing annotation declares `None`. It does not introduce broader signature inference.

### Inspectability and acceptance

Signature help, hover, generated API documentation, and checked signature metadata must expose the effective `None` return type. Return diagnostics must remain anchored to the source declaration and offending expression. A formatter must accept and preserve either spelling rather than requiring a rewrite between them.

Conformance requires equivalent checking and execution for paired omitted/explicit declarations, including free functions, concrete and abstract methods, and supported async declarations. It also requires rejection of incompatible return values and unchanged callable and trait compatibility across module and package boundaries. New syntax in this Draft is proposed behavior, not evidence of an implemented feature.

## Alternatives considered

- **Keep all return annotations mandatory.** This maximizes visible uniformity but retains repetitive syntax for declarations whose default contract can be fixed unambiguously.
- **Infer an omitted return type from the body.** This makes the signature depend on implementation details and can hide an accidentally missing annotation.
- **Allow omission only on free functions or private declarations.** This creates different spellings for the same contract depending on declaration context, without adding type safety.

## Drawbacks

The language gains two spellings for the same signature, and an accidentally omitted non-`None` annotation becomes a return-type error rather than a syntax error. Clear diagnostics are necessary. Authors who prefer a visible result annotation can continue writing `-> None`.

## Layers affected

- **Parser / AST:** declarations must accept omission while retaining source provenance for diagnostics and formatting.
- **Typechecker / signature resolution:** omission must establish `None` before body checking, with the same contract across callable and trait checks.
- **Formatter / LSP / documentation:** source formatting must preserve the chosen spelling, while signature displays expose the effective return type.

## Design Decisions

- An omitted return annotation declares None; it does not request inference from the body.
- This draft has no remaining semantic questions identified by the review. It remains Draft pending lifecycle review.
- Implementation may be considered late in v0.6 or in v0.7; this RFC does not commit to a release.
