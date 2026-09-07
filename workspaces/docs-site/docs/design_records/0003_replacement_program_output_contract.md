---
id: DD-0003
title: Deliver replacement program output during execution and bind it into the receipt
status: Draft
type: design-decision
date: 2026-09-07
review_target: v0.6
sources:
  - https://github.com/encero-systems/incan/issues/1249
  - https://github.com/encero-systems/incan/issues/1254
---

# DD-0003: Deliver replacement program output during execution and bind it into the receipt

## Context

The experimental direct replacement backend runs a compiled program without going through Rust emission. That backend has to answer a question the emission backend answers by construction: when the program prints, what actually reaches the user's terminal, and when?

An earlier reading of this boundary treated program output as evidence to be captured rather than delivered — the executor accumulated rendered lines and the CLI relayed them only after the execution receipt had been persisted. That reading has since been reversed in the implementation, and the reversal is the reason this record exists. Output is now written through caller-supplied writers as execution proceeds, which changes what a failure leaves behind and what a receipt can honestly claim.

The contract needs a durable statement because two wrong descriptions are easy to reach for. It is not a general effect system, and the line-oriented projection that survives in reports is not a typed effect trace. Naming the boundary once is cheaper than re-deriving it from `src/backend/replacement/program_io.rs` each time a builtin is admitted.

## Decision

- **Operation identity is compiler-owned.** A call dispatches on `BuiltinFnId` only when the typechecker proved it binds no source declaration; a same-module declaration retains `direct_call_id` and keeps its own meaning. Consumers must dispatch from that identity, never from the spelling `print` or `println`. This makes a user-defined function named `println` an ordinary call, not a builtin.

- **Output is delivered during execution, not after it.** Builtin print writes and flushes each rendered line through writers borrowed from the caller before execution continues. The executor opens no files or descriptors of its own: the CLI supplies its normal stdout and stderr handles, and a capture harness supplies in-memory writers instead. There is no whole-program delivery buffer.

- **Accepted bytes survive later failure.** `ProgramOutput` retains the bytes each supplied writer accepted even if execution or receipt persistence subsequently fails. A program that printed three lines and then panicked has printed three lines; the failure does not retract them. `OutputCheckpoint` separates one execution's writes from earlier writes through a reused caller-owned adapter, so sequential executions over one adapter stay individually observable.

- **The line projection is narrow and named.** `printed_lines` retains completed builtin-print calls for the line-oriented successful report projection, reached through `ReplacementExecution::emitted_output()`. It is a `Vec<String>` serving reports and output identity — not an effect vocabulary, and no promise that filesystem, provider, or process operations will ever use the same carrier.

- **Receipts commit to what was printed.** The output identity includes a length-prefixed summary of the emitted lines, so two distinct streams cannot collide on a digest input through embedded separators or newlines. `BackendExecutionReceipt` binds that identity alongside the rest of the execution evidence. A receipt therefore commits to the observable source effect rather than treating a returned value as the whole program result.

- **An unrunnable shadow comparison stays visibly non-green.** `unavailable_shadow_comparison` preserves the distinction between "nobody asked" and "asked and could not run": the first is `NotRequested`, the second is `Unavailable` carrying a reason that names the concrete boundary. Only the second is a non-green outcome. An unavailable comparison is a backend-selection regression signal, never grounds for falling back to the other backend.

- **Builtin admission is a registry decision, not a spelling decision.** `EXECUTABLE_BUILTINS` in `src/backend/replacement/mod.rs` is the single source of truth for which builtins the direct profile executes. A builtin is admitted when its direct answer is proven not to diverge from the Rust-emission backend; anything else is refused. This record deliberately does not transcribe the membership list, because the list moves and a copy of it here would rot into a false boundary.

## Consequences

- Program output behaves the way a user expects from a running program: it appears as the program runs, and a crash does not erase what already appeared.
- A test harness can still observe output exactly, by supplying its own writers rather than by relying on the executor to withhold delivery.
- Reports and receipts remain reproducible even though delivery is streaming, because identity is derived from the retained line projection rather than from the delivered byte stream.
- Admitting a further builtin is a change to one registry plus its proof of non-divergence, not a change to this contract.
- An output-bearing shadow comparison is honest and unavailable rather than silently green. A profile that wants to compare output must first define how both routes transport and compare ordered lines.

## Non-goals

- Defining a general effect system, or replacing the line projection with a typed effect trace.
- Granting the direct profile filesystem, provider, or process effects. Captured print output is not authority for any of those.
- Making output-bearing shadow comparisons green, or comparing a returned value while discarding output.
- Promising parity between the replacement backend and Rust emission for any builtin not admitted by the registry.
- Superseding the backend-selection receipt contract or the issues this record derives from.

## Revisit condition

Revisit when any of the following is proposed:

- an externally observable effect beyond ordered output lines needs direct execution;
- a shadow profile can transport and compare both returned values and ordered output without relying on an ambiguous stdout result frame; or
- a consumer needs output identity to cover delivered bytes rather than the retained line projection.

Such work must state its own operation identity, authority, receipt, replay, and comparison contract. It must not treat this narrow output carrier as a general-purpose effect mechanism by implication.

## Provenance

This record derives from the builtin-execution representation work in [#1249](https://github.com/encero-systems/incan/issues/1249) and the session-analysis execution work in [#1254](https://github.com/encero-systems/incan/issues/1254), both now closed. It documents the contract as implemented, including the reversal of the earlier capture-then-relay reading described in Context. It does not supersede those issues or the backend-selection receipt contract.

## References

- [Issue #1249: decide and represent how the replacement executor calls builtins like `println`](https://github.com/encero-systems/incan/issues/1249)
- [Issue #1254: execute replacement profiles from one CompilationSession analysis](https://github.com/encero-systems/incan/issues/1254)
- `ProgramOutput`, `OutputCheckpoint` — `src/backend/replacement/program_io.rs`
- `EXECUTABLE_BUILTINS`, `ReplacementExecution::emitted_output`, `canonical_emitted_output_summary` — `src/backend/replacement/mod.rs`
- `ShadowComparisonState`, `unavailable_shadow_comparison`, `BackendExecutionReceipt` — `src/backend/selection.rs`
