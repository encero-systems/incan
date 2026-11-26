---
id: DD-0018
title: Drop elaboration by rustc and directly serialized plan records
status: Draft
type: design-decision
date: 2026-11-14
sources:
  - DD-0004
  - DD-0015
  - RFC 173
  - Maintainer decision of 2026-11-14 on the structure of lowering and the native plan
---

<!-- markdownlint-configure-file {"MD025": {"front_matter_title": ""}} -->

# DD-0018: Drop elaboration by rustc and directly serialized plan records

## Context

Incan lowering turns checked Body IR into a native plan, and the compiler driver supplies each planned function to rustc as its MIR body at the `mir_built` query. rustc then runs its own passes on that body. It borrow-checks the body, and it elaborates drops: at each drop it decides whether the value is still initialized, with a drop flag where control-flow paths disagree.

Lowering has also tracked moves itself. It removes a moved value from the frame's owners, saves the owners before each branch, and reconciles them where branches join. That state is read and written across most of lowering, and a value left live on only some paths is refused unless releasing it early is equivalent.

A plan product is stored through a second set of types that mirrors the plan's own types. The plan's own types already serialize, and the driver hands the whole plan to rustc that way.

## Decision

- **Lowering schedules drops by lexical scope.** A scope records the values initialized inside it. Leaving the scope emits their drops in reverse order.
- **rustc decides whether a whole value is still initialized.** Lowering records a move only as a move operand. It does not remove a moved value from a scope's owners, and it does not reconcile owners where branches join.
- **Lowering builds the unwind path for each scope.** rustc elaborates the drops on that path as it does on the normal path.
- **Two cases stay with lowering.** Lowering keeps tracking a tuple whose fields move separately, and it does not rely on rustc's elaboration for a type whose drop does more than release storage. Each case moves to rustc once it is shown to be elaborated correctly.
- **A plan record is the plan's own types.** One fold over the plan translates nominal positions, and the result is serialized as it is. The mirror types retire.
- **Refusal is an explicit check.** A plan that holds a variant a record cannot carry is refused by a check over the plan, not by the absence of a mirror case.
- **Crumb remains the destination for plan records.** DD-0015 decides that move. It happens when reading a plan through Crumb costs no more than the format it replaces and typed encoding covers nested and recursive types.

## Consequences

- The branch-join reconciliation and its refusal go away. A program in which one branch moves a value and another keeps it is accepted, and rustc drops the value through a drop flag.
- A move that lowering records wrongly is a rustc borrow-check error on the planned body. It is a compiler defect to fix in lowering, never a diagnostic for the program's author.
- Ownership state leaves most lowering functions. What remains is held by the scope.
- The record format follows the plan's types. A change to a plan type changes the format, and stored products written before the change are cache misses.
- Renames that the mirror types carried are no longer available. A stored record names a plan type's variants as the plan does.

## Non-goals

- This record does not fix how the rest of lowering's construction state is divided. The contributor page on lowering and plan structure describes that direction.
- It does not define the fold's interface or the scope's interface.
- It does not change what a drop does, the order of drops within a scope, or the plan's statements and terminators.
- It does not schedule the move of plan records to Crumb.

## Revisit condition

Revisit when rustc's elaboration is shown to cover, or not to cover, a tuple whose fields move separately or a type whose drop does more than release storage. Revisit also if the driver supplies bodies to rustc at a later query than `mir_built`, since drop elaboration would then no longer run on them.

## Provenance

This record derives from DD-0004, which routes native code through the pinned rustc, and from DD-0015, which sends compiler records to Crumb. It records the maintainer's 2026-11-14 decision and the measurements behind it: rustc elaborates drops on and borrow-checks a supplied body, and plan products and plans restore equal through the mirror types and through direct serialization. It supersedes neither record.

## References

- [DD-0004](0004_native_route_through_the_pinned_rustc.md): the native route through the pinned rustc
- [DD-0015](0015_compiler_producer_map.md): the compiler producer map
- [Lowering and plan structure](../contributing/explanation/lowering_and_plan_structure.md)
- RFC 173: walking and reconstructing typed values
