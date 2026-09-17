# RFC 126: Oven debugging and evidence-backed investigations

- **Status:** Draft
- **Created:** 2026-09-17
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 018 and RFC 019 (assertions and test runner)
    - RFC 048, RFC 085, RFC 086, RFC 087, and RFC 113 (checked metadata, schemas, and declaration registries)
    - RFC 076 and RFC 078 (mutation policy and typed workflow actions)
    - RFC 080 (AI assets and execution observations)
    - RFC 093, RFC 094, and RFC 095 (telemetry, context managers, and spans)
    - RFC 103 and RFC 104 (secrets, authority, receipts, and replay classifications)
    - RFC 102, RFC 105, and RFC 106 (semantic inspection, advice, and compiler-backed context)
    - RFC 111 (proof-aware contract feedback)
    - RFC 116, RFC 117, RFC 118, and RFC 119 (foreign boundaries and Oven operational authority)
    - RFC 120, RFC 121, RFC 123, and RFC 124 (source, type, executable, and compiled-unit identity)
- **Issue:** [#1638](https://github.com/encero-systems/incan/issues/1638)
- **RFC PR:** [#1645](https://github.com/encero-systems/incan/pull/1645)
- **Written against:** v0.6.0-dev.4
- **Shipped in:** —

## Summary

This RFC proposes an Oven-owned debugging and investigation capability for humans and agents working on Rust, Incan, and mixed projects. Its north star is that humans and agents can investigate unexpected behavior, test competing explanations, and verify repairs across Oven-managed Rust and Incan projects, with inspectable evidence throughout. Compiler services supply checked meaning, runtime integrations supply supported observations, and Oven coordinates the exact artifacts and bounded execution used to test a hypothesis. Editor and MCP clients consume the same service contracts. The intended delivery is v0.8, with design closure and focused feasibility evidence required before v0.7 exits; this Draft is not a shipment claim.

## Core model

1. **An investigation is durable.** A failure, question, or counterexample opens a record connecting the relevant source snapshot, builds, experiments, observations, and verification outcomes.
2. **Meaning and execution have different owners.** Compiler services own language semantics; Oven owns project selection, artifact lifecycle, execution coordination, and investigation sessions.
3. **Rust and Incan are first-class.** Pure Rust, pure Incan, and mixed execution are acceptance cases from the beginning. Capabilities can differ, but those differences must be explicit.
4. **Explanations are tested.** A hypothesis states supporting evidence, competing explanations, and an observation that could contradict it. An experiment records what was controlled and what remained uncontrolled.
5. **Evidence retains its kind.** Checked facts, advisory findings, proof results, observed values, and agent inferences remain distinct even when presented together.
6. **Clients share state.** CLI, editor, and MCP consumers use one session model, with explicit execution-control ownership and immutable captured observations.
7. **Recording is bounded.** Time, retained bytes, events, inspected values, and external-operation budgets are inspectable. Missing or truncated evidence is never silently treated as absence.
8. **Investigation ends with reviewable evidence.** A repair links back to the original reproduction and its verification, without implying general correctness from one passing run.

## Motivation

A native debugger can stop a process without giving a developer a reliable path from an unexpected result to a justified explanation. Source and executable mismatches, unreadable values, missing async relationships, transient failure state, and repeated manual reproduction all break that path. Agents face the same problems and additionally pay for unstructured output and a tool round trip for every small inspection operation.

The surrounding RFCs already establish much of the required vocabulary. RFC 106 supplies compiler-backed context; RFC 105 supplies deterministic advisory findings; RFC 111 supplies obligations, assumptions, and counterexamples; RFC 104 supplies execution-bound receipts and honest replay classifications. What is missing is the workflow that connects those facts to an actual native execution, permits bounded experiments, and preserves the result for independent inspection.

This work assumes the post-v0.6 architecture. Generated Rust is not the source authority, public debugging contract, or required semantic handoff. A backend may expose implementation artifacts for advanced inspection, but ordinary debugging must refer to authored source and checked identities.

## Goals

- Make file, executable, and failing-test debugging accessible without reconstructing Oven's build context manually.
- Provide source-faithful breakpoints, stepping, stacks, and value inspection for the declared supported target and debugger matrix.
- Let humans and agents formulate, run, compare, cancel, and inspect bounded experiments.
- Connect native observations to compiler-backed declarations, test cases, contracts, and operation receipts through verified identity mappings.
- Preserve investigations across process exit and client handover when the required evidence was captured.
- Provide structured, budgeted agent operations as well as ordinary interactive debugger controls.
- Define acceptance evidence for pure Rust, pure Incan, mixed calls in both directions, and supported runtime integrations.

## Non-Goals

- Replacing existing native debugger engines, editor interfaces, compiler services, or the test runner.
- Universal deterministic replay, arbitrary reverse execution, or recovery of information that was never captured.
- Making an effect receipt a full execution trace, a telemetry span a proof, or a passing reproduction proof of general correctness.
- Implementing RFC 111's verifier, RFC 105's rules, or RFC 106's graph extraction inside Oven.
- Requiring the complete RFC 121 type-substrate roadmap or every optional integration before basic debugging can ship.
- Making arbitrary foreign runtimes, GPUs, or freestanding targets support hosted debugging facilities by implication.
- Defining an autonomous agent that chooses repairs, modifies contracts, or deploys changes without the invoking workflow's authority.
- Freezing CLI spellings, MCP protocol revisions, or a debugger vendor in this Draft.

## Guide-level explanation

### Start from the failure

A developer selects a failed test, an executable, or a captured failure. Oven resolves the selected workspace member, target, environment, build, and available source/debug artifacts. A missing artifact or mismatched source revision is reported before a source location is presented as trustworthy. A direct-file Incan convenience may delegate project execution to the same service under RFC 118.

The first stop shows authored source, meaningful frames, local variables, and structured values. A list exposes elements, an enum exposes its active variant, and a model exposes fields. Compiler-generated machinery can be expanded deliberately. Selecting a Rust frame selects Rust expression semantics; selecting an Incan frame selects Incan semantics.

### Test an explanation

Suppose a mixed-language test returns an incorrect total. Compiler context identifies the relevant functions. An Architect finding suggests that a recoverable failure may be discarded, but does not establish the cause. The investigator proposes two explanations: the caller supplied the wrong value, or the callee transformed the right value incorrectly.

A bounded experiment stops at the call boundary and records the selected argument and result. Its output contains the stop reason, source/build identities, the requested values, and unavailable or omitted fields. The observation may distinguish the hypotheses or leave both open. A subsequent experiment can reduce the failing input while retaining the same failure predicate and declared external fixtures.

After an implementation repair, the investigation links the changed source and new executable to reruns of the original reproduction and appropriate regression tests. A human can inspect the same evidence without replaying the agent's conversation.

### Continue someone else's investigation

A capture can be opened by an editor or queried through MCP after the original process exits. It contains only the state actually captured, with retention, sensitivity, and completeness information. Continuing a live process requires acquiring execution control. Reading a historical capture does not resume or rerun the program.

### Investigate waiting and external operations

For a supported async runtime, the investigator can inspect recorded tasks, suspension locations, cancellation, and wait relationships. Uninstrumented relationships remain unknown. RFC 104 receipts explain authority-bearing operations; RFC 093 telemetry can connect them to the surrounding request or operation. Neither is treated as a complete scheduler history.

## Reference-level explanation

### Ownership and supported capabilities

Oven must own workspace and target selection, artifact acquisition and retention, execution coordination, and investigation lifecycle. Language services must own source meaning, expression semantics, and the mapping of language-level values and locations to the selected executable. Runtime providers must identify the observations they support. Clients must not independently reconstruct those facts from generated-source spellings or terminal output.

Each session must expose a versioned capability description identifying target, architecture, toolchain, debugger engine, language integrations, runtime integrations, optimization/debug profile, and supported operations. Unsupported combinations must be rejected or explicitly degraded before an operation claims success. Pure Rust use must not require authored Incan application code or Incan-specific proof metadata.

### Investigation and evidence records

An investigation must have an identity and schema version. It must reference its initiating failure or question, source snapshot, selected execution configuration, immutable executable payload identity, and relevant debug/source artifacts. It may link RFC 106 context, RFC 105 findings, RFC 111 obligations and assumptions, RFC 104 receipts, and RFC 093 telemetry without changing their owning semantics.

An experiment must record its hypothesis or purpose, reproduction recipe, controlled inputs, uncontrolled dependencies, stop/capture conditions, budgets, execution mode, and outcome. A recipe must identify the test or action, arguments, fixture references, working-directory mapping, relevant environment inputs, and seed when applicable. Secrets must be represented through authorized references or redaction markers rather than copied into recipes by default.

Evidence records must distinguish compiler-established facts, deterministic advisory findings, proof outcomes with assumptions, runtime observations, and investigator inferences. Each observation must name its producing execution and capture or stop. Confidence in a hypothesis must not overwrite the certainty or provenance of its underlying evidence. Observation of one path must not imply completeness over all paths.

### Source, artifact, and value identity

Sessions must bind executable payloads and debug artifacts to their producer and source snapshot. Symbol names alone are insufficient to establish that a source buffer matches the executing artifact. Uncommitted source used for a build must have a capture or digest that distinguishes it from the repository revision alone.

RFC 120 and RFC 106 supply source identity semantics; RFC 124 supplies compiled-unit and payload identity semantics. Cross-build comparisons must use accepted identity mappings and retain both build identities. Where durable declaration identity or a mapping is unavailable, the comparison must report that limit rather than match by name or line number and call it exact.

Debug artifacts and retained captures must have explicit leases, pins, or another accepted retention relationship to the Oven store. Export must enumerate missing dependencies and capture limits. Reopening must verify retained payload identities; an investigation file must not silently resolve to a newer executable.

### Native debugging fidelity

For supported development profiles, breakpoints and stepping must resolve to authored executable source operations. Unbound or relocated breakpoints must expose their actual status and location. Native frames, reconstructed inline frames, and runtime-derived logical async frames must remain distinguishable.

Values must report their source type and availability. Optimized-out, inaccessible, unsupported, redacted, and omitted-for-budget values must not be rendered as ordinary null or empty values. Value expansion must be bounded and paginated where necessary, handle cycles, and avoid silently invoking arbitrary application formatting code. Target memory reads that may have effects, including device memory, must not be treated as passive inspection by default.

The development profile should prioritize predictable stepping and inspectable locals. Optimized profiles must state their weaker guarantees. Transformations such as inlining, tail-recursion lowering, and ownership planning should be explainable through compiler facts; tools must not fabricate physical frames or recoverability to resemble the original source.

### Session lifecycle and shared control

A live session must distinguish launching, running, stopped, exited, detached, cancelled, and failed states, or an equivalent explicit state model. State-changing requests must include an expected session revision or equivalent concurrency guard. At most one client may own execution control at a time; other authorized clients may inspect retained state. Handover must be explicit, and conflicting requests must fail without silently changing the process.

Live value handles must be scoped to a stop generation and become invalid after resume, restart, or termination. Retained captures are immutable observations with separate identities. Long-running operations must expose operation identity, progress, cancellation, and a bounded wait surface. Retrying a request must not duplicate a launch or external effect; the service must provide deduplication or report an uncertain outcome requiring reconciliation.

Launch, attach, detach, terminate, and client-disconnect behavior must be explicit. A disconnect must not silently kill an attached process. Capture failure must not be reported as successful capture, and process cleanup must report failures rather than claim containment that the host cannot provide.

### Experiments, evaluation, and replay

The service must separate passive inspection, expression evaluation that executes target code, resume, restart, rerun with changed inputs, and replay. Execution authority must be checked independently from the ability to read a capture. An expression evaluator must use the selected frame's language, disclose supported constructs, and reject unsupported evaluation rather than reinterpret an expression in another language.

A bounded experiment should combine resume, stop-condition evaluation, and selected capture into one client operation. It must identify why it stopped, including condition met, process exit, timeout, cancellation, budget exhaustion, or provider failure. A failure to reach a condition within a budget is inconclusive, not evidence that the condition is impossible.

Historical inspection reads recorded evidence. Replay reproduces execution using a declared supported recording mechanism. A rerun performs another execution and may repeat side effects. Replaying against changed code is a new experiment, not the original recorded execution. Operation replayability must reuse RFC 104 classifications; whole-session replay support must additionally identify unrecorded nondeterminism and foreign/runtime boundaries. Receipts alone must not establish replay support.

Counterexample reduction must preserve an explicit failure predicate and record the attempted transformations and validation outcomes. The result may be a smaller reproducer; it must not claim global minimality without evidence. A reduction with uncontrolled nondeterminism must expose that limitation. Verification must rerun the original reproduction against the repaired artifact and retain independent regression results; it must not silently weaken the asserted contract to declare success.

### Capture, telemetry, and domain integration

Capture policies must bound time, event count, retained bytes, value depth, and applicable operation budgets. Providers must report sampling, truncation, dropped events, clock domains, and instrumentation settings. A local sequence or timestamp must not imply a total causal order across independent processes.

Runtime providers may expose tasks, waits, cancellation, and bounded historical events. Domain libraries may expose typed operations and inspectors through checked metadata and declaration registries. Domain semantics remain library-owned. Unknown relationships must remain visible as unknown; absence from a partial trace is not proof that an operation never occurred.

RFC 104 reporting modes and receipt-free permissive mode must remain intact. A debugger capture must not manufacture authority receipts for a receipt-free run or imply that raw foreign calls were governed. Telemetry export remains opt-in under RFC 093. Local capture and remote export are distinct choices.

### Secrets and mutation boundaries

Incan-owned typed displays, MCP outputs, and structured bundles must preserve RFC 103 redaction. Raw memory captures and foreign values may contain secrets beyond those guarantees; such captures must expose sensitivity and export restrictions. Redaction may prevent replay and must be recorded as such. Expression evaluation, reruns, tool invocation, and source edits must retain their own authority boundaries under RFC 104, RFC 078, and RFC 076 as applicable.

An investigation service must not turn the read-only RFC 106 context surface into implicit execution authority. Contract modifications must remain distinguishable from implementation repairs under RFC 111. Recorded target output and source text are evidence data, not instructions to the agent client.

### Human and agent interfaces

Editor, CLI, and MCP adapters must consume the same investigation/session contracts and return consistent identities and outcomes. MCP operations should expose task-sized investigations and bounded context packs, with low-level debugger controls available for narrower inspection. Exact tool names and wire mappings must be settled before Planned status.

A response must expose its budget, omissions, freshness, and continuation mechanism where applicable. A human-readable explanation must retain links to structured evidence. MCP transport sessions must not define the lifetime of an investigation or live debuggee. The user-program MCP authoring proposal and the compiler-context MCP service remain distinct integrations.

### Acceptance and release boundary

The v0.7 exit gate must resolve every question in this Draft, accept the necessary owner-contract amendments, and produce focused feasibility evidence before promoting this RFC to Planned. A document review alone does not establish native-debugging feasibility. The gate must identify the supported target/debugger/runtime matrix, measurable latency and overhead budgets, and explicit exclusions.

The v0.8 acceptance corpus must cover pure Rust, pure Incan, and mixed execution in both call directions. It must include: an incorrect result investigated through competing hypotheses and reduced input; a hung or cancelled operation with supported runtime evidence; and a failure reopened from another run using matching artifacts. Each applicable case must connect the observation to a repair and original-reproduction/regression verification.

Negative cases must include stale source, missing symbols, optimized-out state, stale handles, concurrent control requests, cancellation, duplicate requests, redaction, unsupported runtimes, unavailable replay, and truncated captures. Editor and MCP consumers must demonstrate consistent evidence. Debugging tests must drive real native artifacts and debugger sessions; metadata snapshots alone are insufficient.

Measure diagnosis and repair correctness, unsupported causal claims, setup steps, tool calls, token/output volume, elapsed investigation time, capture overhead, and retained size against a shell/log baseline on fixed seeded tasks. Targets and repeated-run methodology belong to the v0.7 design closure. Full deterministic replay, broad foreign-runtime coverage, and rich freestanding debugging are separately qualified extensions rather than universal v0.8 promises.

## Design details

### Existing contracts and prerequisites

RFC 102 joins inspectable project facts; RFC 106 supplies semantic context; RFC 105 supplies advice; RFC 111 supplies proof-aware obligations. This RFC links them to experiments without merging their authorities. Basic debugging must work when optional advice, telemetry, or proof providers are absent.

RFC 118 fixes the command/service boundary. Oven consumes language services and never routes through the Incan CLI. RFC 117 and RFC 119 retain project, target, and provider authority. RFC 121 contributes supported representation facts without becoming an all-or-nothing prerequisite. RFC 123 may support semantic experiments, but an interpreted result must not be presented as native execution evidence.

RFC 019 owns test discovery, fixtures, selection, seeds, and reporting. Investigation recipes must reference that contract instead of defining another runner. RFC 080 contributes AI asset and execution-profile identity; repeating a model request must not be called exact replay without a supported recording mechanism. RFC 094 and RFC 095 supply scope-lifetime and span semantics; those facts do not by themselves supply scheduler history.

Future async closures, stack-safe lowering, algebraic numeric operations, unsafe/layout controls, and additional interop providers should add debugging acceptance cases as their semantics are accepted. Their full implementation is not a prerequisite for this RFC. Existing freestanding and ownership roadmaps retain their independent delivery boundaries.

### Identity and evolution

Every public record family must be versioned. Readers must preserve unknown evidence kinds or refuse unsupported required semantics explicitly. Missing source, unavailable mappings, and stale evidence must remain first-class outcomes. A later capture or explanation may reference earlier evidence but must not rewrite the historical observation.

The exact relationship between durable declarations, compilation-specific locations, native symbols, and store payloads must be settled with RFC 106, RFC 120, and RFC 124 before cross-build comparison is claimed. Source-content identity, declaration continuity, and executable identity serve different purposes.

## Alternatives considered

- **Expose native debugger commands directly through MCP.** Useful as an escape hatch, but insufficient for durable evidence, bounded experiments, identity checks, and shared human/agent control.
- **Build an Incan-only debugger first.** Rejected as the governing architecture because Oven owns Rust and mixed projects as first-class workloads; implementation slices may still target one capability at a time.
- **Make telemetry the debugger.** Telemetry supplies selected events, not complete locals, breakpoint semantics, or native process control.
- **Make receipts the replay engine.** Receipt classification describes what is known about operations; it does not capture every source of nondeterminism.
- **Require universal replay or complete verification before delivery.** This would delay useful native debugging and conflate independently valuable capabilities.
- **Let each client own a separate session model.** This creates races, stale handles, and inconsistent evidence during human/agent handover.

## Drawbacks

Native debugger and runtime integrations create a platform/version support matrix that requires ongoing conformance testing. Capturing values and execution events costs CPU, memory, storage, and sometimes changes timing. Sensitive data can enter raw captures even when typed views redact it. Durable investigations also require retention policy and artifact availability. These costs must be measured and exposed rather than hidden behind a seamless interface.

Evidence can be accurate while a diagnosis remains wrong. A reproduction can pass without covering the original environmental cause. The service can preserve evidence and test outcomes, but it cannot guarantee that an investigator selected the right hypothesis, contract, or repair.

## Implementation architecture

Non-normatively, use an Oven investigation service over existing operational APIs, with native-debugger providers, compiler-owned semantic adapters, runtime observation providers, and immutable evidence storage. CLI, editor/DAP, and MCP adapters should remain consumers. Checked declaration metadata can describe domain inspectors without making a generic plugin ABI a prerequisite. Implementation language and crate boundaries remain governed by existing authoring and Oven ownership rules; this RFC grants no new authored-Rust exception.

## Layers affected

- **Compiler semantics and lowering:** preservation and projection of source locations, scopes, types, transformations, and value availability.
- **Native debug metadata and language services:** source stepping, structured inspection, expression semantics, and foreign-boundary fidelity.
- **Oven operational services and store:** session control, selected artifacts, leases, experiment execution, and evidence retention.
- **Stdlib and runtime integrations:** supported task/wait observations, scoped captures, redaction, receipts, and telemetry correlation.
- **Test runner and typed actions:** reproducible invocation metadata, fixtures, failure predicates, and verification linkage.
- **Editor, CLI, and MCP:** shared session views, bounded operations, handover, and evidence navigation.
- **Documentation and conformance:** support matrix, failure modes, investigation examples, and native execution proof.

## Unresolved questions

- **D1 — Native support matrix:** Which debugger engines, operating systems, architectures, development/optimized profiles, and attach/post-mortem modes form the initial supported matrix, and what executable proof establishes each?
- **D2 — Identity and retention:** What accepted mapping joins durable declarations, compilation locations, native debug information, payload identities, and exported capture retention across source edits?
- **D3 — Service schemas and control:** What exact versioned records, state transitions, concurrency guards, retry semantics, and CLI/editor/MCP operations implement the lifecycle contract?
- **D4 — Evaluation and capture:** Which passive value decoders and expression subset are supported initially, how is target-code execution authorized, and what sensitivity rules govern raw captures?
- **D5 — Runtime history and replay:** Which runtime integration establishes the first task/wait capture, which historical and replay mechanisms are feasible, and how are unsupported boundaries reported?
- **D6 — Experiments and reduction:** What initial recipe, fixture, failure-predicate, comparison, and reduction contracts compose with the test runner and typed actions?
- **D7 — Quantitative acceptance:** Which seeded corpus, baseline, repeated-run methodology, and latency/overhead/storage/diagnosis thresholds define v0.8 acceptance?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
