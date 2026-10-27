# RFC 157: Chained pattern guards

- **Status:** Draft
- **Created:** 2026-10-17
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 049 (single-pattern conditions)
    - RFC 071 (pattern alternatives)
    - RFC 129 (binding and ownership)
    - RFC 134 (exiting pattern bindings)
    - RFC 138 (parenthesized condition chains)
    - RFC 139 (sequence patterns)
- **Issue:** [#1183](https://github.com/encero-systems/incan/issues/1183)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Extend RFC 138's parenthesized pattern-condition chains to match-arm guards. A guard may combine ordinary conditions and let pattern bindings using left-to-right short-circuiting and. Successful bindings are visible to later terms and that arm's body only. Guard failure resumes ordinary arm selection without rolling back effects. This supplement leaves RFC 138's if/while contract intact and keeps guarded arms non-covering for exhaustiveness.

## Motivation

RFC 138 handles dependent optional extraction in if and while, but explicitly excludes match guards. A dispatcher can therefore match an event cleanly and still need nested conditionals plus repeated fallback logic to express the real arm condition. The missing capability belongs in the guard, where the match decides whether the arm applies.

## Goals

- Reuse RFC 138's chain syntax and evaluation rules in guards.
- Keep arm-pattern bindings available during guard evaluation.
- Make additional successful guard bindings local and predictable.
- Preserve ordinary match ordering and exhaustiveness checks.

## Non-Goals

- Adding bindings to arbitrary boolean expressions or allowing partially bound names through or.
- Undoing effects or ownership changes after a failed guard.
- Making guarded patterns exhaustive.
- Changing the already drafted if/while syntax in RFC 138.

## Guide-level explanation

The pattern guard is proposed. The enum, lookup, and dispatcher are application definitions, supplied in full.

```incan
enum Event:
    Request(str)
    Tick


def route_for(name: str) -> Option[str]:
    if name == "health":
        return Some("/health")
    return None


def dispatch(event: Event) -> str:
    match event:
        Event.Request(name) if (
            name != ""
            and let Some(path) = route_for(name)
            and path != ""
        ) => return path
        Event.Request(_) => return "unrouted request"
        Event.Tick => return "tick"
```

A health request returns /health. An unknown or empty name reaches the second arm; route_for is not called for an empty name. name originates in the arm pattern; path originates in the successful guard extraction. Neither binding exists in the second arm.

The fallback Request arm remains necessary. The guarded Request arm cannot establish that every Request value is handled, regardless of what route_for currently returns.

## Reference-level explanation

- A pattern-bearing guard must use the same parenthesized chain grammar as RFC 138.
- Terms must evaluate once, left to right. A false condition or failed pattern must skip the remaining guard terms and that arm's body.
- Names bound by the arm pattern must be available to the guard. Successful guard bindings must be available to later terms and the arm body only.
- A binding under top-level or or not must be rejected under the same restrictions as RFC 138.
- Guard failure must continue ordinary ordered arm selection.
- Guarded arms must not contribute unconditional exhaustiveness coverage.
- Effects already performed must not be rolled back. Moves or mutations must remain valid for any subsequent arm that can execute; unsafe shapes must receive an ownership diagnostic.
- A guard binding that duplicates a name from its arm pattern or an earlier guard binding must be rejected. The diagnostic must identify both binding locations and suggest a distinct capture name. Ordinary outer-scope shadowing rules remain unchanged.

## Design details

This RFC deliberately supplements rather than expands the scope of RFC 138 silently. Both are associated with the original proposal issue, which asked for ordinary condition chains and guards. They should share one condition model even if reviewed or delivered separately.

The collision boundary is the current arm and its guard chain. Duplicate captures there are rejected; this rule does not introduce a global prohibition on shadowing an outer binding. A failed guard cannot be used to smuggle a moved payload into later matching; source ownership, not a backend cloning workaround, decides what is permitted.

## Alternatives considered

- Put nested if statements inside the arm: supported, but selects the arm before testing its real condition and often duplicates fallback behavior.
- Permit unparenthesized let chains only in guards: introduces a second grammar and parser ambiguity.
- Allow or with conditional bindings: requires a separate definite-binding model and is excluded here.

## Drawbacks

A guard can hide substantial work if authors use effectful extraction. Ownership across a failing guard is subtle and requires useful diagnostics. The new form adds another place where binding scope depends on successful evaluation.

## Layers affected

- **Parser / AST:** reuse of chained conditions in guards.
- **Typechecker / ownership:** guard binding scopes and safe continuation.
- **Match checking / lowering:** non-covering guards and ordered short-circuit execution.
- **Formatter / LSP:** multiline chains and binding diagnostics.

## Design Decisions

- Duplicate bindings within an arm and its guard chain are rejected with both source spans and a renamed-capture suggestion. Shadowing an outer binding follows ordinary language rules.
- A failed guard continues ordered arm selection. Ordinary checked ownership must keep every reachable continuation valid; this construct introduces no rollback, hidden cloning, or separate permission to move a payload needed by a later arm.
