# 4. Add a builtin

A builtin needs a language-owned spelling and contract, a checked representation and native behavior. Begin with a behavior that cannot be expressed as an ordinary library function.

## Trace a precedent

Inspect the builtin registry in `loaves/kernel/incan_lang/src/lang/builtins.rs`, then trace a nearby builtin through the frontend and Body IR into `incan_mir_lowering`. Use codegraph definitions and caller queries before assuming two similarly named helpers own the same boundary.

## State the contract

Specify parameters, return type, coercions and refusals. Extend an existing contract case when it can hold the new behavior. Run that selector to establish the failure.

## Implement each boundary

Register the spelling once. Have checking record resolved facts in Body IR. Have Incan lowering consume them and select the native operation. Keep Rust-specific translation in the existing MIR adapter.

Do not add another semantic decision in the adapter or restore generated-Rust emission.

## Verify

Run the focused contract, then the affected families. Run `make pre-commit-fast` and update public language documentation. A compiler rebuild alone does not prove the runtime contract.
