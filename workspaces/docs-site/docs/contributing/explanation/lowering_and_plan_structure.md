# Lowering and plan structure

This page describes the structure that Incan lowering (`loaves/compiler/incan_mir_lowering/src/`) and the native plan (`loaves/compiler/incan_mir_plan/src/`) follow. [DD-0018](../../design_records/0018_drop_elaboration_and_plan_records.md) decides the ownership rule and the record format. The division of construction state is the direction lowering takes as its files change; until a part is built, `Builder` in `loaves/compiler/incan_mir_lowering/src/frame.incn` holds that part's fields.

## What rustc does after lowering

The driver supplies each planned function to rustc as its MIR body at the `mir_built` query (`loaves/toolchain/incan-rustc-driver/src/bodies.rs`). Every later rustc pass runs on that body as it runs on a body rustc built itself.

- **Borrow checking runs.** A value moved twice in a planned body is a rustc error. Such an error is a defect in lowering.
- **Drop elaboration runs.** At each drop, rustc decides whether the value is still initialized. Where one path moved the value and another kept it, rustc adds a drop flag.

Lowering therefore states where a value's scope ends and where the value moves. It does not work out whether a whole value is still there when its scope ends.

## Ownership

A scope is one lexical region of a frame: a function body, a branch, a match arm or a loop body.

- A scope records the locals that are initialized inside it and need a drop.
- Leaving a scope emits those drops in reverse order of initialization.
- Each scope has an unwind path that drops the same locals when a call inside it unwinds. Scopes that hold the same locals share one path.
- A loop is a scope that also records, for each `break`, the block it leaves from.
- A move is a move operand in the plan. Lowering does not remove the moved local from its scope.

Two cases stay with lowering, inside the scope, until rustc is shown to handle them:

- a tuple whose fields move separately, where only the remaining fields are dropped;
- a type whose drop does more than release storage.

Code outside the scope does not read or write the list of owners. It enters a scope, records an owner and leaves the scope.

## Construction state

A frame's construction state is divided by who changes it.

| Type | Holds | Changes |
| --- | --- | --- |
| Frame | Locals, blocks, the current block, external functions the frame calls, functions it synthesizes, and its scopes | Through its own methods only |
| Function context | The program tables, the source and its file, the module and body identities, type parameters and arguments, the trait environment and the decorator role | Never during lowering; a lifted child frame shares its parent's |
| Frame facts | The frame's name, its checked local types, which locals hold function pointers or references, and how its result is viewed | Set when the frame is created, read afterward |
| Iteration state | The iterators a loop polls and the items it borrows or changes in place | By the functions that lower iteration |
| Closure state | Which locals hold closures and how each closure's storage was fixed | By the functions that lower closures and partial calls |
| Generator step | The yield handle and the bindings and clauses of a generator expression's step | Set on the frame that builds the step |

A lowering function takes the parts it uses. A function that only reads the function context takes it without `mut`. A function that reserves a temporary and emits a call takes the frame.

Lowering one function is recursive: a statement holds expressions, an expression can hold a closure, and a closure's body holds statements. The functions that lower those constructs call each other, and they remain functions. [Readable Incan](readable-incan.md#state-and-its-owner) states when a function becomes a method of the type that owns its state.

## Runtime calls

Lowering calls the native runtime by path. The runtime's entry points are one closed set, and each entry gives its path, parameter types and result type. A call names its entry; it does not repeat the path and signature. The record codec's list of paths it can store is derived from the same set.

## One fold over the plan

Several passes rebuild a plan with something changed at its leaves:

- capturing and binding the nominal positions of a plan product;
- folding carrier positions after the source enums at publication;
- moving a generator's yield handle into its parameter slot.

One fold walks every node of the plan and rebuilds it. A pass overrides only the leaves it changes: a nominal position, an enum or model owner, or a local.

A fold visits every node that holds another node, so a pass cannot skip a variant by leaving it out of a `match`. A pass that only inspects the plan, such as the check for variants a record cannot carry, uses the same walk without rebuilding.

## Plan records

A plan record is the plan's own types, serialized after the fold has translated nominal positions.

- A stored product names each model and enum by its product-local ordinal. Binding translates each ordinal to the position its consumer resolved.
- A variant that a record cannot carry is refused by a check before the plan is stored.
- The format follows the plan's types. A change to a plan type makes earlier stored products cache misses.

[DD-0015](../../design_records/0015_compiler_producer_map.md) sends compiler records to Crumb. Crumb's format can describe the plan, including its recursive types. Plan records move to it when reading a plan through Crumb costs no more than the format it replaces and typed encoding covers nested and recursive types.

## See also

- [Architecture](architecture.md) for the pipeline around lowering.
- [DD-0004](../../design_records/0004_native_route_through_the_pinned_rustc.md) for the route through the pinned rustc.
