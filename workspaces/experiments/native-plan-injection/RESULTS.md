# Native plan injection results

The typed handoff works. An Incan-authored producer constructed executable native plans, and the existing pinned rustc adapter compiled them without parsing a target program or deriving its body through the Incan frontend. The experiment also exposes a preflight diagnostic defect and language policy embedded in the current native representation.

This is evidence for a private compiler authoring boundary. It does not establish full self-hosting, an implementation of SCIR, or a stable public API.

## Observed controls

| Control | Observed result |
| --- | --- |
| Constant-returning function, producer input 42 | Executable prints 42 |
| Same producer, input 73 | Executable prints 73 |
| Integer argument and checked addition, input 40 | Executable prints 42 |
| Positive branch, input 42 | Executable prints 42 |
| Negative branch, input -4 | Executable prints 0 |
| Zero branch, input 0 | Executable prints 0 |
| Construct Pair(40, 2), read both fields, add | Executable prints 42 |
| Same record body, input -3 | Executable prints -1 |
| Boolean return declaration with integer return local | Preflight refuses; no executable is published |
| Checked addition at the maximum signed 64-bit integer | Executable fails with an arithmetic overflow |
| Runtime overflow source attribution | Supplied producer file and coordinates 1:1 are retained |
| Invalid-return preflight source attribution | **Gap:** supplied source span is absent from the error |

All ten execution/rejection controls passed. Runtime source attribution passed; preflight attribution did not. The test locations are deliberately supplied coordinates, not a claim that the adapter independently inferred a source expression.

A separate ordinary Incan source program also compiled and printed 42 through the same final driver. The repository's pre-commit-fast gate and the producer's native formatter check passed. The full compiler contract census and Rust facet suites were not run for this isolated experiment.

The negative control reports:

```text
Error: Plan(Invalid { function: "answer", reason: "return local must match a scalar return type" })
```

The existing `validation::invalid` helper retains the function name and reason but not its span. The experiment does not mask this by manufacturing a location in the command wrapper.

## What crossed the boundary

`Scenario` is defined as an enum in the Incan producer. The Rust entry imports that same type, constructs its variants, and calls `build_plan`. There is no Rust mirror of either Scenario or Plan.

The producer owns the program's function declarations, locals, blocks, arithmetic, branches, model fields, calls, and source locations. Its main function invokes the planned answer, then calls the existing runtime's integer formatting and printing functions. Changing the producer input changes a constant in the compiled program; the produced executable takes no corresponding command-line input.

The temporary Rust entry only selects the scenario, reads the explicit toolchain manifest, verifies identity, invokes the producer, and submits the returned Plan to the unchanged adapter. That entry remains an experiment, not a new public command.

## Minimum supported surface established

| Concern | Existing surface exercised |
| --- | --- |
| Authoring | Functions, models, enums, typed lists, local mutation, conditions, imports, and ordinary native library calls |
| Shared types | Incan-authored Scenario and the canonical Incan-authored Plan consumed by Rust |
| Program structure | Function signatures, typed locals, indexed basic blocks, assignments, call destinations, returns, and branch edges |
| Value handling | Literal operands, explicit copy/move operands, record field projections, and an owned formatted string passed into the runtime |
| Arithmetic failure | Checked value/overflow pairs and an explicit overflow assertion |
| Provenance | SourceSpan values supplied by the producer and retained in runtime failure reporting |
| Native authority | Verified pinned compiler identity, explicit dependency artifacts and search paths, existing external-signature checks, crate kind, and overflow policy |
| Output | Native executable or a refusal from the existing validation/adapter boundary |

No new language feature was needed to construct these plans. That does not prove that ordinary Incan can yet implement the adapter's rustc callbacks, lifetime-bound compiler values, or complete ownership analysis.

## Constraints the next design must address

1. **Preflight errors need structured source evidence.** The current error shape loses the location even though the input carries it. Preserve the failing declaration/node location and enough identity to connect the failure to the authoring operation.
2. **A native record currently implies Incan model policy.** The first record attempt with no derives was refused. The accepted record explicitly selects Clone, FieldInfo, and IncanClass, as required by `valid_nominal_derives`. Decide which layer supplies language defaults before presenting this as a general low-level construction API.
3. **Constructible is not the same as checked.** Plan is ordinary mutable data. Existing preflight checks signatures, indices, and shapes; its documentation explicitly says it is not an ownership checker. The supported API needs a clear checked handoff and authority contract.
4. **Packaging is coupled to the compiler.** The adapter is a private module in a binary, not an importable Incan-facing library. This experiment adds a small temporary executable entry; it does not resolve that boundary.
5. **Bootstrap generations must be coherent.** Compiler support, runtime, standard providers, Oven, and the native driver must agree on identities and supported interfaces. Never relax those checks to make the authoring experiment pass.

## Bootstrap observations

An arbitrary installed dev toolchain was not sufficient for rebuilding the current source compiler. Early attempts hit relative artifact paths in copied manifests, stale transitive package paths in an older installation, and conflicting incan_lang artifacts. These are bootstrap inputs and packaging failures, not failures of the typed plan handoff.

The successful sequence used the pinned seed compiler to build the checkout's Incan native-support helper and Oven. The helper then produced a fresh support generation. The repository's source-interface, native-runtime, and standard-library scripts built core, testing, and async against that generation. Matching Oven built the experimental driver using that private home.

The existing Incan lowering library was rebuilt once as a dependency of the driver. Subsequent producer changes reused it. A separately importable adapter would make this proof much smaller to build and explain.

To establish an equivalent build home, use the source-generation stages in `scripts/release-dev8.sh`: prepare a private home, build the matching native-support helper, produce source support, check the core/async interfaces, build the runtime, and build the required standard providers. Keep the source checkout and compilation working directory fixed. The experiment's `build.sh` then takes the matching Oven and that home explicitly; `verify.sh` records each control and input digests.

## Evidence identity and limits

- Base source: `ced7d6c35909e642af57522ef246248baad19b90`.
- Host: `aarch64-apple-darwin`.
- rustc: `1.98.0`, commit `88d9e12ae178fab0fb5cc050a94da85685d449ea`, LLVM `22.1.8`.
- Verified producer SHA-256: `0b9c744df8121ed4fbeacc892bab35bc2810bc4e75573967d3642436326bac1c`.
- Verified experimental driver SHA-256: `d13e4608ac74a5dbb7aa44e6cd2c5e1ab3eaf463b85215c0f3b7d20904ca784c`.
- Raw outputs, build logs, and machine-local manifest digests are retained with the isolated experiment; verification writes a new results table and input receipt on every run.

The experiment has not tested other targets, ABI compatibility across versions, generic program construction, async/cancellation, arbitrary ownership graphs, untrusted plan submission, or performance. The production adapter is still Rust. SCIR and the public std.compiler contract remain design work.

The next step is to define the smallest private adapter session around the demonstrated handoff: inputs, checked-state transition, structured diagnostics, and dependency/target authority. Then replace one narrow piece of the Rust adapter with Incan and record any actual language or interop gap that prevents it.
