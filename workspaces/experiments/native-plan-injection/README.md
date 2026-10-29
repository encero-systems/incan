# Native plan injection experiment

This experiment asks whether ordinary Incan can author the typed input to the existing native adapter, without a source frontend producing that input. It is an executable design probe, not an implementation of SCIR or a public compiler API.

The producer constructs the canonical `incan_mir_plan.Plan` directly. The existing adapter validates that value, injects declarations and MIR into pinned rustc, and links the resulting executable. No generated Rust source represents the injected program.

## Ownership of the experiment

- `producer/src/lib.incn` owns declarations, bodies, control flow, constants, runtime calls, and source locations.
- The small `plan_spike.rs` entry loads an explicit toolchain manifest, verifies its identity, calls the Incan producer, and passes its return value unchanged to `adapter::compile`.
- Existing Rust adapter code still owns validation and conversion to rustc internals. This experiment does not claim that layer has been rewritten in Incan.
- The native plan is the existing backend-specific representation. It is not SCIR.

The experimental command is intentionally private to this branch. Its Rust entry is removable packaging glue for the binary-only, rustc-private boundary documented in #1337. A supported Incan caller API must eventually replace it. It defines no new semantics and performs no source parsing, checking, or Body IR lowering.

## Reproduce

Use matching native Oven and a private compiler build home whose source support, runtime, standard packages, and pinned rustc form one coherent generation. An arbitrary installed runtime is insufficient when the compiler sources select different support artifacts; see the bootstrap observations in the results.

From the repository root:

```sh
experiment=workspaces/experiments/native-plan-injection
"$experiment/build.sh" "$OVEN" "$SPIKE_HOME"

driver="$PWD/loaves/toolchain/incan-rustc-driver/target/incan/rust/release/bin/incan-rustc-driver"
manifest="$SPIKE_HOME/toolchains/0.6.0-dev.8/toolchain.json"
"$experiment/verify.sh" "$driver" "$manifest" "$NEW_EVIDENCE_DIRECTORY"
```

Both build arguments are explicit: the matching Oven executable and the experiment's writable compiler build home. The verification output directory must not already exist. The build validates and reuses the home's selected support artifacts. It uses Oven; it does not create a Cargo project or a second plan schema.

## Acceptance

The first controls require the generated executable to print 42, then 73 when the producer's input changes. An incompatible function return declaration must be rejected without publishing an executable. Diagnostic source fidelity is recorded separately; refusing a bad plan is not enough to claim a useful source diagnostic.

Further controls exercise arguments, arithmetic, branches, and a record through the same construction boundary. See [observed results and limitations](RESULTS.md) and the [recorded control table](results.tsv).

## Boundary under examination

The minimum input currently includes declarations, local types, explicit copy/move operands, blocks and continuations, external signatures, and source locations. Native compilation additionally needs the verified toolchain identity, admitted dependency artifacts, output location, crate kind, and compilation policy.

The producer owns its plan until it transfers the value into the adapter. Compiler-session internals and rustc types do not enter that value. External signatures remain subject to metadata checks; constructing a plan does not grant authority to invent a dependency's type.

A future API needs a clear distinction between constructible input and checked input, structured validation failures with exact source evidence, and explicit target/dependency provenance. Those requirements should be established by this experiment before choosing public names or publishing a stable API.
