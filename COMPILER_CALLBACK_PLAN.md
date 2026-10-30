# Compiler callback experiment

Goal: make an independent Incan executable pass a native rustc callback, then implement a stateful callback in Incan. The dev.8 compiler is the build tool; the consumer imports native rustc libraries directly and never imports Incan's Plan or adapter.

## Pattern intake

The owning boundary is checked Rust interop, followed by typed Body IR, Incan-authored MIR planning, and rustc validation. Precedents: `rust_boundary.rs` trait-object borrowing; `layout_types.incn` receiver storage; `values.rs` checked alias conversion; `rust_facts.rs` native trait solver. Generic foreign adoption requires explicit facts and native implementation headers, not a callback-name special case.

## Capability and red tests

Existing probes 02 and 03 reproduce respectively check/build disagreement and rejection of native `TimePassesCallbacks` as `Callbacks + Send`. Probe 04 already proves direct library invocation and exit status. First resolve the native borrowed trait-object boundary while retaining all bounds. Then add source-authored callback adoption with complete signatures, state mutation and cfg insertion. Reject missing implementations and immutable receiver use. Verify separate instances do not share state.

## Acceptance

1. Native callback executes `--print cfg` through rustc's library entry.
2. Incan callback executes once, adds a cfg value, and retains observable independent state.
3. Negative trait and borrowing probes diagnose failures, no fabricated conformance or erased Send bound.
4. Existing direct-entry, ordinary-linkage, bootstrap-refusal and identity controls still pass.
5. No Rust consumer glue, generated Rust source, or compiler Plan input in the consumer.

Local experimental compiler changes are tracked against CA-2 in the standalone repo's `issues/callback-check-lower-gap.md`. Update results and exact evidence. No public release claims, implementation PR, engine implementation, or typed-body injection in this slice.

## Gates

- `scripts/reduced.sh 03_rustc_native_callback check` and build/run, then custom callback probes.
- `scripts/verify-reduced.sh` for earlier controls (update expected results only when proven).
- Native driver release build with the copied SDK and isolated Oven.
- Changed Rust rustfmt, `make pre-commit-fast` where the seed supports it, Incan formatting, and `git diff --check`.

Broader compiler acceptance remains separate from this explicitly isolated spike. A failed gate is recorded rather than represented as passing.
