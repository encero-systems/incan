# RFC 148: Must-use values and deliberate discard

- **Status:** Draft
- **Created:** 2026-10-17
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 036 (declaration decorators)
    - RFC 070 (Result combinators)
    - RFC 103 (secret-safe diagnostics)
    - RFC 128 (callable signatures)
    - RFC 131 (typed error propagation)
    - RFC 163 (cross-target builds and compile-time declaration selection)
- **Issue:** [#1286](https://github.com/encero-systems/incan/issues/1286)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Propose compiler-defined `@must_use` annotations on value types and callable declarations, with source-level diagnostics when an expression statement discards a protected result. `@may_discard` is the candidate callable-specific exemption. The proposal covers discarded expressions; it does not claim that a bound resource was finalized or that a variable was subsequently used. Violating a declared obligation is an Incan source error by default; the explicit-discard spelling, built-in defaults, and exemption precedence remain open.

## Motivation

A checked conversion can fail correctly while the application accidentally ignores its Result. A transformation can return a replacement value while the caller mistakenly expects in-place mutation. These errors deserve a diagnostic on the Incan expression that discards the value, with the declaration explaining why consumption matters.

## Goals

- Diagnose accidental discard before generated code is compiled.
- Let an API author explain the obligation at its declaration.
- Allow a deliberate discard with a visible escape hatch.
- Preserve the distinction between discarded expressions and unused bindings.

## Non-Goals

- Proving transaction commit, resource cleanup, or exactly-once consumption.
- Making containers inherit obligations from their elements or fields.
- Changing Result propagation or introducing implicit error handling.
- Treating assignment to a named variable as consumption verification.

## Guide-level explanation

These compiler-defined annotations use decorator syntax; they do not resolve to ordinary decorator functions or add runtime wrapper calls. They are proposed. The application function below is fully defined; Result and ordinary control flow are existing language facilities.

```incan
error CountError:
    Negative(value: int) = "Count must be non-negative; got {value}."

@must_use(message="check whether the requested count is valid")
def checked_count(raw: int) -> Result[int, CountError]:
    if raw < 0:
        return Err(CountError.Negative(value=raw))
    return Ok(raw)


def configure() -> None:
    checked_count(-1)             # Proposed diagnostic: discarded checked result.
    result = checked_count(-1)    # Outside this check: the result has a binding.
    match result:
        Ok(count) => println(count)
        Err(error) => println(error)
```

The check deliberately does not report the binding itself. A separate unused-binding diagnostic may exist, but it must not be represented as this feature proving that the Result was handled.

A proposed explicit discard is `_ = checked_count(-1)`. That spelling is a candidate for discussion, not a claim about current wildcard-assignment semantics. The diagnostic should show the selected escape hatch alongside handling or propagation; it should not suggest discarding as the preferred fix.

A type author could similarly mark a replacement-value API:

```incan
@must_use(message="use the replacement price")
model Price:
    cents: int

    def discounted(self, reduction: int) -> Price:
        return Price(cents=self.cents - reduction)
```

Here the returned Price matters because the method constructs a replacement. The example does not depend on all model instances being immutable.

## Reference-level explanation

- `@must_use` and the candidate `@may_discard` must be compiler-defined declaration annotations, not ordinary callable decorators under RFC 036. They must not add runtime wrappers.
- A must-use check must apply to an expression statement whose resolved callable or resulting type carries the obligation. Violating a declared obligation must be an Incan source error by default. The diagnostic must identify the responsible declaration and reason, and explain handling, explicit `?` propagation where legal, or deliberate per-expression discard. Existing diagnostic-policy controls must remain explicit; generated Rust suppression is not a substitute for this check.
- A binding, return, argument, or enclosing expression must not be diagnosed as a direct discard merely because its value may later go unused.
- Annotation arguments must be compile-time metadata. Messages must remain subject to RFC 103's non-disclosure rules.
- The obligation must follow canonical declaration identity across imports, aliases, and package metadata.
- RFC 163 compilation selection must happen before must-use annotation checks, regardless of annotation order. An excluded declaration, type, member or statement must contribute no must-use obligation. Calls retained in selected method or function bodies must receive the same discard checks as ordinary calls. A retained declaration must follow this RFC unchanged; the compilation predicate namespace must not change ordinary metadata argument lookup.
- A container must not implicitly become must-use because its element or field type is marked.
- Result and Option are proposed built-in must-use types; this default requires an explicit compatibility decision before acceptance.
- If both a callable and its result type carry an obligation, one diagnostic must identify both relevant declarations rather than emitting duplicates.
- An explicit-discard form must suppress this obligation at that expression only. It must not convert Err into success.

## Design details

The recommended starting policy is opt-in obligations plus built-in Result and Option protection. A callable-specific `@may_discard` exemption is useful for documented best-effort operations, but whether it overrides an explicit callable-level obligation or only a result-type obligation remains open. Contradictory annotations should be rejected rather than resolved by source order.

Compiler diagnostics and editor hovers should expose the reason and declaration location as structured facts. The generated Rust lint is insufficient as the public diagnostic boundary. Rust's [diagnostic attributes](https://doc.rust-lang.org/reference/attributes/diagnostics.html) provide prior art for declaration-carried must-use obligations; Incan's exact defaults remain its own decision.

## Alternatives considered

- Make every non-None function must-use by default: broader protection, with a much larger compatibility and noise cost. Retain this as a policy alternative for review.
- Require resource consumption tracking: valuable, but it answers a different question from expression discard.
- Rely on generated Rust warnings: loses the source-level explanation and stable Incan diagnostic contract.

## Drawbacks

Built-in Result and Option protection will surface existing silent discards. An exemption can also hide real failures if applied broadly. This feature cannot prove that an application acted on a bound result.

## Layers affected

- **Checked declaration metadata:** obligations, reasons, and exemptions across package boundaries.
- **Typechecker / diagnostics:** expression-discard checks and source locations.
- **LSP / documentation:** visible obligations and deliberate-discard guidance.

## Design Decisions

- **Default diagnostic severity:** discarding a declared must-use result is an Incan source error, with a visible deliberate local escape once its spelling is settled. This does not choose built-in obligations or imply that a bound value was handled.

- **Composition with compile-time selection:** RFC 163's `@when(condition)` and `when condition:` select declarations before annotation semantics. `@must_use` applies only to retained declarations and types. This ordering does not decide the diagnostic defaults or discard mechanisms still open below.

## Unresolved questions

- Should Result and Option be protected by default, or should every non-None callable be protected?
- What is the explicit-discard spelling and how does it interact with wildcard binding?
- Is `may_discard` the final name, and which type-level obligations may it override?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
