# Compiler record contracts

`internal_models` holds the compiler's shared record contracts: the values a compiler producer publishes and the scoped reads it makes. A consumer that only names a compiler fact depends on this Loaf, not on the lowering, Oven or the worker lifecycle. It depends on no native compiler object; the frontend's native adapters translate to and from these values outside it.

**Versioning:** Release-train cadence, with the driver that links it.

| Module | Holds |
| --- | --- |
| `record_scope` | Revision-bound reads over one pinned snapshot: full record keys, selections, read evidence, witnesses, faults and the one complete receipt. Sources own bytes; the scope owns read evidence. |
| `records` | The public read boundary over `record_scope`. |
| `record_service` | The DD-0013 compiler producer interface over those reads: stages, ranks, preparation and evaluation. |
| `checker_records` | The canonical checker record schema, its bounded field codec and its semantic digest, for the initial free-function profile. |

## Contract

- **Reads.** `record_service` reuses `records` for keyed reads. Preparation distinguishes final inputs, earlier prerequisites, semantic diagnostics and compiler faults; evaluation distinguishes staged results, semantic diagnostics and compiler faults. An unfinished supplier during evaluation is a typed contract fault.
- **Contracts only.** This Loaf does not implement record-scoped checking or plan production. The current reader's table positions and boolean witnesses cannot certify revision-bound read roots. Producers require the existing checker and Body IR operations to consume final keyed environments before they can implement this interface; whole-program checking or lowering followed by record slicing does not satisfy it.
- **Canonical values.** A checker record is a profile, a checked stable owner, a position-free semantic root and ordered fields. Native objects never enter the codec, and an unsupported capability is refused, never silently omitted.
- **Compatibility.** `incan_mir_lowering` re-exports the `records` and `record_service` names from its facade.

The [record-scope cases](./tests/test_record_scope.incn) and [canonical controls](./tests/canonical_record_controls.incn) cover these contracts. The canonical controls compare against the frontend's native adapter, which stays out of this Loaf's manifest, so installed CI's Oven test step, `scripts/oven_tests.incn`, runs them in an isolated project that adds it.

## Ownership record profile

`incan.checker-free-functions/11` and `incan.checker-fields/9` distinguish declaration ownership annotations from body-owned access facts. These versions describe checker records; profile 11 additionally binds the selected final Body IR owner frame, whose internal layout remains independently versioned by the executable representation.

Graph-selected Body IR values are opaque kernel executable-owner frames at representation version 55. The record graph authenticates and transports those bytes, while `incan_semantics_core` alone encodes and interprets the native `Body`. `body_ir_declarations.incn` remains a transitional adapter test surface and is not a durable Body IR authority.

| Field family | Retained information |
| --- | --- |
| `ownership` | Declaration modes, parameter/result components and an explicitly named result source. |
| `input-copy` | The callable owner's exact checked Copy decisions by resolved value type, independent of graph event tags or ABI adaptation heuristics. |
| `input-call` | The exact callable owner and callee, explicit-versus-ordinary declaration classification, instantiated parameter contracts and receiver mode. |
| `input-local` | A canonical local declaration, its callable owner, selected access mode, checked type, write permission and annotation anchor. |
| `input-flow` | Complete storage permissions, place projections, loans and parent relationships, local loan destinations, owned destinations with exact initializer/source-event relationships, operation completion and ordered operand dependencies, invocation argument bindings, closures bound to locals with their captures, loan and capture reads, graph nodes and control-flow edges. |
| `binding-type` | The checked type of a canonical body binding, or a canonical parameter requirement retaining Some(type) versus an explicit None conflict. |
| `mutation` | Completed changed-parameter inventories and reassigned local declarations. Empty changed-owner inventories remain distinct from absent ones. |

The Incan codec validates primitive shapes, counts, indices and canonical ordering. Ownership map keys use length-framed UTF-8 tokens; declaration identities and source anchors remain opaque handles. The native bootstrap adapter resolves those handles against the current owner and source revision, then recomputes graph analysis. It does not recover discarded semantic payloads from the current checker. A checked graph can retain source-program conflicts; an explicitly invalid graph must reproduce its recorded infrastructure refusal and cannot become a checked graph. Unsupported source projections retain their anchored refusal.

This is still a bounded free-function profile. Pending mutation decisions, forwarding edges, callable-value bindings, loop-view state, active mutation frames and other unrepresented checker capabilities refuse projection. They never become empty successful facts. Profiles through 7 are not accepted as profile 8. Each invocation retains explicit evaluation boundaries for its receiver and arguments, the exact source Read/Move or loan-creation node, a completion node, and ordered final loan uses. Restored argument order and complete nested evaluation intervals must agree with the selected source call; neither a surviving signature nor recomputed liveness can replace a missing transfer event. A non-Copy operand can retain its source until the enclosing operation consumes it, after later operands have been evaluated. The record therefore retains the operation, its completion node and every exact operand/place/loan/final-read relationship. Structural graph checks authenticate these relationships; execution admission also requires complete coverage against the checked source operation. Reanalyzing a graph with an omitted consumption event cannot recreate this authority. Owned initializer relationships are retained explicitly; neither source-span searches nor current checker payloads reconstruct a missing relationship. The existing 64 KiB record and 4,096-field limits are unchanged.

The canonical codec tests execute Incan encode/decode round trips and compare the complete restored selected owner with an independent fresh check after source positions move. They also remove ownership facts and corrupt versions, map order, counts and graph references. Passing the primitive Rust adapters alone is insufficient evidence for this boundary.

Invocation controls additionally test exact protocol-field restoration and synthetic Incan codec records, including receiver intervals. Complete owner projection now retains call-related `ExpressionArtifacts.body_ir_parameter_requirements`, including conflicts. Complete-owner round trips cover the supported ordinary fixed-call shapes; synthetic receiver controls do not claim executable method support.

Copy decisions use the checker's existing derive relation while the source scope is active. Restored source-access validation requires the exact owner and type entry; missing evidence cannot turn into an inferred Copy result. The codec preserves these decisions and canonical type ordering without re-running derive resolution or reconstructing decisions from Read/Move events.

Ordinary source-call facts are retained only when their caller independently requires explicit-input analysis. Their synchronous concrete header and parameter inventory must match the selected canonical declaration. The bounded ordinary route admits immutable checked Copy inputs; non-Copy inferred access, mutable adaptation, generic parameter substitution and nominal result dependencies remain refused. Classification is not an input access mode and does not change the ordinary declaration ABI. Changing a stored classification cannot replace an explicit contract.
