# Build engine design

This page is the detailed contract behind [DD-0013: The Incan build engine](../../design_records/0013_the_incan_build_engine.md), which the maintainer accepted on 2026-10-30. DD-0013 summarizes it and governs where the two differ. DD-0013 also adds rules this page does not repeat: capture and freshness, Rust and C interop, comments, docs and source positions, and damaged records. The storage contracts the engine relies on are in the [bakery storage design](bakery_storage_design.md).

**Scope:** the engine, its compiler-service boundary and its clients. The checker, Body IR semantics and the native rustc route remain authoritative.

## 0. The end state

An invocation selects an immutable input snapshot, asks for named results, and obtains handles to facts already present or computed once from final inputs. Neither the CLI nor a worker constructs an independent package closure, cache, or freshness system. IncQL-DB holds the semantic records, their observed dependencies, native payload references, and the bindings that make a particular build meaningful.

There is one physical bakery for a sharing group and any number of **virtual stores** over it. A home, project configuration, editor overlay, or test fork selects its own bindings and retention roots. Identical admitted contents occupy the same physical objects. A virtual store is not a union of directories searched in priority order, and sharing bytes does not confer authority to select them.

```text
incan / oven / LSP / MCP / contract runner
                    |
              typed requests
                    v
       one engine for a physical bakery
       bounded scheduler; no retained program closure
          |                         |
    compiler workers          virtual-store snapshots
    keyed reads                     |
          +-------------------------+
                    v
           IncQL-DB substrate
     immutable records, indexes, payloads,
     scoped leases, claims, atomic bindings
                    ^
          verified sealed-set admission
       toolchain / Loaf / local bake / registry
```

The engine is Incan-authored. Its compiler adapters invoke the existing language core, not a second checker. Native artifacts still come from the pinned rustc route. There is no Cargo invocation, including acquisition, and no Python production component.

The engine runs **one service per physical bakery, with homes as logical contexts**. This provides a single resource budget and claim coordinator across sharing homes without adding a separate broker. Independent physical stores remain independent engines and do not promise cross-store bake-once. This refines DD-0012: homes become virtual stores over a shared physical bakery.

### 0.1 Decision register

The sections referenced here contain the contracts, not just preferences. An implementation may change representation details that preserve them; changing a decision requires review rather than a silent implementation choice.

| ID | Decision | Rejected alternative and reason |
| --- | --- | --- |
| BE-01 | Virtual stores select COW binding roots over a shared physical bakery (§1). | A full store per test/home duplicates contents and prevents common claims; a global mutable namespace mixes configurations. |
| BE-02 | Availability, admission, and selection are distinct (§1, §6). | Reusing anything found by digest ignores provenance, capability completeness, and the caller's trust policy. |
| BE-03 | One resident engine per physical store; native workers are single-invocation at a time (§2). | One heavyweight owner per project retains closures; one service per virtual home requires another global resource broker. |
| BE-04 | All durable engine facts live in IncQL-DB; volatile scheduling is disposable (§2). | Per-project JSON ledgers and stamp files create a second authority and recovery model. |
| BE-05 | A request pins captured inputs and explicit result roots (§3). | Reading ambient latest during compilation mixes input revisions; mtimes alone are not content authority. |
| BE-06 | Use a versioned, bounded, correlated control protocol; local record reads do not use RPC (§3). | Closure-sized requests and one RPC per compiler record defeat bounded activation. |
| BE-07 | Keep source identity, semantic value, evaluation provenance, native identity, and physical location separate (§4). | One hash either over-invalidates on provenance changes or incorrectly equates different native compilations. |
| BE-08 | Use DD-0012's ranked graph with a finalized-input frontier and one value evaluation per dirty record (§4). | Whole-unit retries, unconstrained recursive demand, and generic fixpoint sweeps conceal repeated work. |
| BE-09 | Record positive, negative, and set-valued reads with their exact scope (§4). | Positive-edge-only dependency tracking misses new declarations, overloads, and feature-selected names. |
| BE-10 | Preserve existing semantic checking scope, independently of codegen reachability (§4). | Checking only functions reachable from main can accept invalid source rejected by today's compiler. |
| BE-11 | The compiler exposes typed per-record producers and read witnesses (§5). | Engine-side AST interpretation, string-parsed missing-record errors, and a second semantic model are prohibited. |
| BE-12 | A typed missing input is not a semantic absence and not a corrupt-store miss (§5). | Conflating them yields wrong diagnostics or silent fallback after damage. |
| BE-13 | Form native units from finalized record roots; build and run share the same program unit (§5). | Putting the CLI verb or target-directory path into unit identity causes duplicate compilation. |
| BE-14 | Keep CGU membership stable by compiler identity and a fixed profile partition (§5). | Repartitioning as program size changes invalidates unrelated code. |
| BE-15 | Native incremental reuse requires tracked compiler inputs; until proven it is disabled (§5). | Enabling rustc incremental around an untracked global Plan can reuse stale MIR. |
| BE-16 | Use the same admission path for toolchain bundles, Loaves, and local results (§6). | Special std injection, whole-pack extraction per consumer, and guessed native compatibility recreate the old closure problem. |
| BE-17 | Claims belong to the physical store and cover completed derivation/native identities (§7). | Per-home locks duplicate bakes; holding a claim while waiting for prerequisites can deadlock. |
| BE-18 | Use fair, byte-accounted admission and CPU tokens, including nested native work (§7). | Counting worker processes alone permits oversubscription and hides retained compiler memory. |
| BE-19 | Cancellation detaches an interest, not a shared computation; user effects are never automatically replayed (§7, §9). | Killing every disconnected request's shared bake harms other callers; retrying a run can duplicate side effects. |
| BE-20 | Keep request arenas and mappings scoped; recycle workers that cannot release them (§8). | A global decoded std/ABI cache violates the intended memory model even when its hits are fast. |
| BE-21 | Use explicit roots and leases, not cache size or age eviction (§8). | Quotas do not prove efficient storage and can destroy still-needed facts. |
| BE-22 | Execute canonical immutable payloads where loader layout permits; writable views are COW/copies (§8, §9). | Writable hardlinks endanger canonical artifacts; rewriting an assessed executable in place is not an optimization. |
| BE-23 | Batch compatible test compilation, but execute each case in a fresh process (§9). | One new binary per compatible case wastes first-launch cost; one shared process changes isolation. |
| BE-24 | Separate historical test observations, reusable evidence, and fresh test execution (§9). | A cached PASS is not evidence that a new acceptance run executed. |
| BE-25 | All readers use bounded snapshot queries with typed cursors (§10). | Whole-graph JSON and process-global semantic indexes turn small questions into whole-world loads. |
| BE-26 | Instrument every logical operation, use a separate trace channel, and retain observations only deliberately (§11). | Console instrumentation changes contract outcomes; always persisting timestamped traces makes unchanged work grow the store. |
| BE-27 | Migrate with an explicit, invocation-wide engine selector; never fall back after selection (§12). | Automatic partial fallback makes correctness and performance evidence uninterpretable. |
| BE-28 | Compare the same frozen census and input closures against a pinned integration tree (§13). | Comparing different fixtures, bundles, hints, or cache states attributes unrelated improvements to this engine. |
| BE-29 | First stage proves the complete architecture on hello, without a bulk-checker shortcut (§14). | A new scheduler around today's whole-unit compile would not test the design's central boundary. |
| BE-30 | Language/platform gaps block their stage; track them rather than introduce private Rust services (§15). | Convenience bridges can silently move engine semantics out of Incan. |
| BE-31 | Follow RFC 124 for equal-unit, unequal-payload conflicts: retain variants, mark the unit machine-local/non-exportable, pin exact payloads (§6). | Silently substituting variants breaks consumers; discarding the second valid payload contradicts the governing RFC. |
| BE-32 | Compiler interfaces outlive their implementation language; present Rust adaptations are bootstrap work (§5.6, §15). | Making Rust ownership part of the engine protocol would obstruct the Incan-authored compiler direction. |
| BE-33 | Publish a shared immutable result once, then admit and select it independently for each caller (§7.3). | One caller's cancellation, rejection or superseded head must not fail another caller's valid selection. |

### 0.2 Evidence is not a delivery claim

The measurements cited here describe the engine as it stood when this design was written. None of them proves a delivered indexed, record-oriented engine. The keyed read path's lowering improvement for hello world, from 38 ms to 3 ms, is a reported result without a reproducible measurement for that exact comparison: treat 3 ms as a target to re-establish, not a verified baseline. Every measurement records its own immutable commits.

RFC 117 still describes explicit Cargo compatibility, RFC 118 says there is no Cargo mode, and RFC 125 allows a temporary Cargo download helper. This design follows the Cargo-free contract of DD-0005: it uses verified direct acquisition or already admitted bytes, and implements none of those compatibility routes.

## 1. Physical bakery and virtual stores

### 1.1 Objects and bindings

The physical store owns immutable records, revision frames, index pages, payloads, claims, and publication coordination. The generic IncQL-DB layer owns the transaction and lifetime semantics. Oven supplies typed families and policies; the physical store does not recognize a compiler, Loaf, or test.

The logical identities are:

| Name | Meaning and lifetime |
| --- | --- |
| `StoreId` | Persistent identity of one physical bakery. Not part of compilation identity. |
| `ViewKey` | Stable identity of a mutable logical view, such as a home catalog or project head. This is not the storage design's content-derived logical view identity. |
| `ViewRevision` | Content-derived binding-root identity. Corresponds to the storage design's logical `ViewId`; unchanged bindings have the same revision. |
| `SnapshotHandle` | Exact store, physical manifest, logical view revision, allowed root set, lease, and format profile. A descriptor, not a mapped closure. |
| `BuildEpoch` | One captured source/configuration selection and its private derived-result view. Sources do not change as results are added. |
| `ExecutionHandle` | Exact native unit, payload/loader roots, and an execution pin. It never resolves a name through latest. |

These names define the protocol and model; they do not claim that the APIs already exist.

A home stores a small binding to a physical store and a home `ViewKey`. Its home catalog selects toolchain and admitted package capabilities. A project view selects a source snapshot, lock/feature/profile/target selection, and result roots. A test or editor view forks a selected project revision. Forking copies the root descriptor; the first differing binding path copies index pages. Reads use the child's effective COW root directly: **no lookup walks a chain of parent overlays**. The parent can advance without changing the fork.

An operation that changes a view compares its expected prior revision. Different views can publish in one physical store without sharing a mutable namespace. There is no implicit merge-back. Explicit merge is a transaction over specified keys with conflict checking; test/editor views never merge automatically.

`INCAN_HOME` continues to select home configuration. A new home's default remains its own physical bakery unless it is explicitly provisioned to join a sharing group. The home binding identifies the shared store by `StoreId` plus locator. A locator change does not change build identity. This avoids silently pooling previously isolated homes while permitting CI to create cheap virtual forks over one prefilled bakery.

Use a small `<home>/bakery.view` bootstrap descriptor with schema `incql-db.view/1`, the full `StoreId`, a locator, and the home `ViewKey`. It contains no package selections, source stamps, result cache, or mutable head digest. Those live in the store. Provision the durable view root before atomically publishing this descriptor; an interrupted provisioning operation is recoverable by its transaction id and must not silently choose another store. This descriptor is configuration needed to find the database, not a second database. Moving or copying a descriptor does not fork its view: an explicit `CreateView` operation creates a distinct `ViewKey`.

### 1.2 Authority and reuse

Three questions have separate answers:

1. **Available:** are the exact objects physically present and intact?
2. **Admitted:** does the current policy accept their origin, producer, receipt, format, and complete capability closure?
3. **Selected:** does this view's plan bind those exact revisions/units?

A global object-directory hit answers only the first. A second home's identical native unit may be reused after admission and exact identity comparison, subject to RFC 124's payload-conflict rule in §6. A conflicted unit additionally requires exact payload selection; another home's private namespace bindings are never inherited by accident. The same package version with different features or native inputs remains a different unit. Different versions with truly identical compiler-observed inputs may share the RFC 124 unit, while their publication provenance remains distinct.

Virtual stores are **same-user logical isolation**, not security boundaries. The user who can read the backing files can read their contents. Mutually untrusted users need separate physical stores and OS permissions; this design does not add cross-user authorization or a distributed cache.

### 1.3 Scoped snapshot leases

A lease pins all facts that the selected view promises it can read later, not just the files it has opened. A test fork can therefore drop mappings without losing access to its old snapshot during compaction or collection.

Add **scoped leases** to the generic substrate contract: a lease pins its logical binding roots, admitted dependency roots, and the physical index generations needed to locate them. It need not retain every other virtual store's payloads merely because the physical manifest also indexes them. Historical location-index pages may remain pinned; unrelated entries in those pages do not become semantic reachability roots. Reads outside the leased logical root set are refused. A raw by-digest debug reader must request a broader explicit lease.

This distinction is necessary for inexpensive test forks and LSP snapshots. A long-lived editor snapshot must not accidentally retain all artifacts produced by every other home. It is an IncQL-DB lifetime capability, not an Oven exception.

## 2. Processes, ownership, and lifetime

| Process | Owns | May retain while idle | Ends when |
| --- | --- | --- | --- |
| `incan` or `oven` CLI | Argument validation, terminal streams, request interest, run child where applicable | Nothing beyond its command | Command or child finishes |
| Engine service | Scheduling, active interests, resource tokens, publication coordination, snapshot/claim handles | Store descriptor, bounded protocol buffers, worker descriptors, filesystem-observer control state; no decoded program/Loaf/ABI closure | Explicit stop or 600 s with no client, worker, effect, or pinned-reader interest |
| Compiler worker | One serialized compiler activation at a time; per-record compiler calls and one rustc session at a time | Executable/runtime code and small protocol state only | Retired on unsafe cleanup, crash, incompatible generation, excess idle footprint, or 10 s idle |
| Program/test process | Program runtime and its own effects | Its own application state | Program exits or explicit cancellation |
| LSP/MCP client | Document versions and small snapshot/cursor descriptors | Its document state and requested snapshot handles | Client/session ends |

Use existing installed executable roles: the Oven executable has an internal service entry; the compiler driver has an internal record-worker entry. There is no new executable per task and no new standalone storage daemon. Internal dispatch is not the public Incan CLI: Oven calls its engine and compiler APIs directly, respecting RFC 118's one-way delegation.

The record-worker control host and orchestration are Incan-authored. The existing Rust driver bootstrap enters that host through the compiler's existing native linkage mechanism. The permitted native adaptations are enumerated in §15; moving the scheduler or store into the Rust driver is not permitted.

The engine is singular per `(StoreId, engine protocol/profile generation)`. A store-level service lock and startup handshake prevent duplicate launch. An incompatible running generation is drained explicitly before replacement; clients receive a named generation mismatch, not a silent route to a different compiler. Existing requests can finish under their pinned generation. Claims and store publication remain correct even when an old service is finishing and a new one is starting, but they do not create independent full worker budgets: the service lock transfers scheduling authority only after the old service has released its workers.

A worker executes one compiler activation at a time because the current native adapter has process-global compilation state. Parallelism is across workers and within a native job's explicitly reserved CPU allowance, not concurrent invocations through that global state.

Service generation and compiler generation are different identities. One compatible service can dispatch to several pinned compiler generations without changing a global toolchain selection. Worker pools are partitioned by exact compiler/host ABI, protocol profile, and process-environment contract, while sharing the service's resource budget. An incompatible compiler must provide the required record-service capability or is refused; its presence does not select the legacy engine. Workers and the service itself retain execution leases for their executable/loader closures, just as user programs do.

All durable state is in IncQL-DB. Queue entries, sockets, watchdogs, OS observer descriptors, in-flight buffers, and resource tokens are volatile control state, not another database. A crash reconstructs desired work from the request's captured roots and committed results. It does not restore a serialized compiler heap or unfinished rustc invocation.

### 2.1 Endpoint and startup protocol

Reuse the existing short, private runtime-directory approach. On Unix, prefer an owner-verified `XDG_RUNTIME_DIR`; otherwise use an owner-verified mode-0700 `/tmp/incan-<uid>` directory. Refuse symlinks and wrong ownership. Endpoint names derive from full store and service-generation identities, with a short name used only for discovery. The handshake checks the full identities so a shortened-name collision cannot select another store.

The existing deep-home fix is a prerequisite to reuse, not a new performance saving claimed by this design. A failed endpoint does not select a cold legacy compiler. It produces an actionable startup error before the command has been accepted.

On Windows, use an owner-restricted named pipe with the same protocol. Platform transport and filesystem durability qualification remain separate exits; no portability claim follows from macOS tests alone.

## 3. Request capture and control protocol

### 3.1 Capturing inputs

The engine captures the selected manifest/lock/configuration, source contents, namespace membership, declared tool/provider products, and compiler-observed environment inputs into immutable input roots. A command operates on those roots even if an editor changes a source file afterwards. Derived results extend a private build view; they do not replace its captured sources.

Each phase dispatch pins a new snapshot of that build view containing its finalized prerequisite results. Every such snapshot must retain the same captured source/configuration roots and `BuildEpoch`. Workers never upgrade their own snapshot by consulting the project's latest head. Old workers finish against their assigned inputs; completed results can be admitted into a later compatible build snapshot only through normal witness validation.

Capture does not claim a global atomic filesystem snapshot. It records the exact bytes actually admitted. Files changing during read are retried; unstable reads or inconsistent directory enumeration are reported after three capture attempts. A coherent external filesystem snapshot can be imported when supplied. Without one, a source tree being edited concurrently may require another capture; it cannot be made globally transactional by naming a manifest `Snapshot`.

An OS change observer can cheaply identify candidate changed paths while its coverage and generation remain valid. Overflow, restart, uncovered paths, environment changes, or tool replacement invalidates that proof and forces content validation. Mtime/size/inode tuples are change detectors, not semantic identity. Without a valid observer, capture checks the metadata of every file in reach and hashes only the files whose metadata differs (DD-0013). It reports this work rather than claiming an O(1) unchanged build.

The resident service keeps observer control state, not an in-memory dictionary of all declarations or previous file contents. Source indexes and prior observations are paged records in the store. An unchanged capture can reuse the prior command-result binding without opening declaration graph pages. Descriptor reads, observation checks, artifact verification, and launching the program still count as work.

A no-op request borrows the existing view revision. It does not create a durable fork or change a last-access timestamp. A private writable build view is allocated only when a new fact or selection must be published; request ids and transient reader leases alone do not require a new result manifest.

Editor buffers enter through a captured overlay view with explicit document versions. No unsaved edit becomes the disk project's latest binding automatically.

### 3.2 Handle contracts

All control messages carry handles or bounded scalar options. Large argument lists, membership sets, source text, diagnostics, and result sets use typed paged records or immutable blob references. A worker is given the exact captured input roots and work descriptor, not `UnitDescription` with a complete decoded dependency closure.

| Handle | Required fields |
| --- | --- |
| Snapshot | `StoreId`, physical manifest digest, `ViewKey`, `ViewRevision`, allowed-root-set digest, lease id, format/profile versions |
| Record | Full `RecordKey`, selected revision, semantic value digest, codec/profile; physical location is resolved within the snapshot |
| Work | Request id, work id, producer/profile id, record key or native unit id, input snapshot, prepared-input-root, resource reservation, cancellation generation |
| Cursor | Snapshot identity, query digest, last full ordered key, direction, rows/bytes remaining; authenticated by the issuing session |
| Artifact | Unit id, exact selected payload manifest digest and linked external payload selections, loader-layout root, caller-specific admission proof, execution/materialization lease |

A handle from another store or expired lease is an error. A cursor never follows a moving head. To share the same records through a new view, the caller obtains a new handle through admission/selection, rather than editing handle fields.

### 3.3 Wire framing

Define `incan.engine/1`, separate from the existing `incan.compiler-worker/1`. There is no guessed compatibility between them. The frame header is 64 bytes, little-endian, with these fields in order:

| Bytes | Field |
| --- | --- |
| 0..7 | Eight-byte protocol magic `INCENG01` |
| 8..9, 10..11 | Major and minor protocol versions (`u16`) |
| 12..13, 14..15 | Message tag and flags (`u16`); unknown required flags are refused |
| 16..31 | Session id (128 bits; zero only for initial hello) |
| 32..47 | Request id (128 bits) |
| 48..55 | Per-direction connection sequence (`u64`) |
| 56..59 | Payload length (`u32`, maximum 65,536 bytes) |
| 60..63 | Reserved zero bytes |

Bodies use a versioned tagged binary schema authored as Incan models: fixed-width little-endian scalars, length-prefixed UTF-8/bytes, explicitly tagged enums, and bounded list counts. Unknown required variants fail closed. Optional extension fields may be skipped only when the negotiated schema says so. The decoder validates lengths before allocation. It does not cast bytes to native struct layouts.

The hello exchanges supported profiles, engine/compiler generation, store identity, peer identity, and resource limits. Unix peer credentials or the named-pipe security identity must match the service user. Session ids, request ids, and monotonically increasing sequences reject stale, duplicate, or unsolicited frames. A protocol violation retires the offending connection/worker; it never consumes another request's reply.

Each direction has eight outstanding data-frame credits. Control messages have a separate bounded lane so cancellation and acknowledgment cannot deadlock behind output. Large records are not fragmented onto this channel; they are read from the leased store. Per-client admitted queue length is at most twice the current CPU-token count, and the service's admitted ready window is eight times that count. Additional requests receive `Busy` with a retryable status or remain unaccepted under back-pressure. These are working-memory limits, not store quotas.

### 3.4 Messages and state

| Message | Contract |
| --- | --- |
| `Hello` / `Welcome` | Negotiate profile and exact service/store identity before any work. |
| `Submit` | Command kind, home/project view, input-selection descriptor, requested roots, run/test options, observation policy. No closures. |
| `Accepted` | Request id, capture token, and interest token. Acceptance precedes capture and is not completion. |
| `Captured` | Immutable input epoch and initial snapshot, after capture has succeeded. |
| `Prepare` | Worker derives compiler-owned selectors/rank/demand descriptors for a stage. It does not run a second checker. |
| `Evaluate` | Worker receives a ready batch of record keys sharing an activation scope, or one native-unit descriptor. |
| `Produced` | Staged result/root handles, read-witness root, diagnostics root, counters. Results are not selected until publication succeeds. |
| `NeedPrerequisite` | Typed scheduling input discovered in preparation; not a diagnostic and not an absence witness. |
| `Query` / `Page` | Bounded read-only snapshot query and cursor. |
| `Event` | Progress/trace handle or bounded status, never program console bytes. |
| `Complete` | Caller-specific committed result snapshot, admission proof and leased result handles, binding outcome, structured status, observation handle where retained. Shared computation completion alone does not complete every caller. |
| `Cancel` / `Cancelled` | Detach an interest and acknowledge effects that have or have not stopped. |
| `Resolve` | Return `Committed`, `NotCommitted`, or `Unknown` for an identifiable transaction/request after loss of acknowledgment. Never rerun user code. |
| `Pin` / `Release` | Manage scoped reader/effect leases. |
| `Drain` / `Stop` | Stop accepting work, settle or cancel explicitly, release resources, then transfer service ownership. |

Request states are `Accepted -> Capturing -> Planning -> Waiting/Running -> Publishing -> Complete`, with `Cancelled`, `Failed`, or `Superseded` terminal alternatives. Work states are `Discovered -> WaitingInputs -> Ready -> Claiming -> Rechecked -> Producing -> Staged -> Committed`; a hit skips producing. Shared work completion and request completion are separate: `Committed` work can have selected, rejected, cancelled or superseded request bindings. Deterministic compiler refusal is a typed result, not a worker crash. Transport failure after acceptance yields an unknown request outcome resolved by id; only side-effect-free compilation work may be resubmitted under its exact identities.

A no-op reuse request need not create a durable transaction merely to remember its request id. If the service dies after answering it, `Resolve` may return `Unknown`; the client can repeat the read/compilation ask against the captured inputs. An execution ticket has different durable pin/outcome handling and must never use this compilation retry rule.

The current protocol's console-path capture remains a compatible transport adapter during migration. New code uses separate bounded streams/spools for stdout/stderr and a separate structured trace stream. Console bytes remain exact; neither profiling nor protocol framing may be inserted into them.

## 4. Work graph and invalidation

### 4.1 Record families

Use compiler-owned canonical identities from DD-0012/RFC 106, not source positions or guessed fully qualified strings. The engine's graph wraps compiler facts with evaluation provenance; it does not redefine the facts.

| Family | Logical key | Value and granularity |
| --- | --- | --- |
| `SourceRevision` | Controlled source origin and logical path | Admitted bytes digest and source-map reference; one source revision |
| `Namespace` / `NameSlot` | Namespace; namespace + name + selection context | Membership root; present/absent declaration binding |
| `DeclarationSource` | Canonical declaration identity | Compiler-normalized declaration input; coordinates are a separate projection |
| `Signature`, `Layout`, `Default` | Declaration identity + semantic facet | One checked signature, layout, or default expression |
| `CheckedBody` / `BodyIR` | Callable identity and relevant instantiation context | One checked body / one Body IR body |
| `Instance` | Generic identity + canonical type/const arguments + required context | Consumer-owned instantiated facts; frozen provider remains unchanged |
| `PlanFunction` | Native instance identity + lowering profile | One lowered function plus its exact native-input projection |
| `CguMembership` | Native namespace + fixed partition identity | Ordered member identity/digest root, not a copied list of all bodies |
| `NativeUnit` | Full RFC 124 unit identity | Native payload variants and conflict status; each selection receipt pins an exact payload manifest and linked external payloads, toolchain/profile |
| `Evaluation` | Producer profile + logical output key + completed input evidence | Output revision, semantic digest, read witnesses, rank, refusal/success status |
| `Selection` / `ResultBinding` | View-local command target/configuration | Exact source, package, record, native, and diagnostic roots selected |
| `RunObservation` / `TestEvidence` | Invocation or explicitly reusable evidence identity | Actual effects/results and replay eligibility, never semantic build authority |

Collections, adjacency, and membership lists are paged. A record may reference a large body payload, but reading its identity, digest, rank, or read-set root does not decode that body.

### 4.2 Identities and early cutoff

`RecordKey` says which question is being answered. `RevisionId` identifies immutable stored state. `ValueDigest` hashes the compiler-canonical semantic answer. An evaluation records how that answer was obtained. Physical location, compression, compaction generation, request id, and home locator are not semantic inputs.

A reusable computation key includes the producer/profile and **observed input values and selectors**, including absence/enumeration values and relevant declared environment. Its evidence record also binds the exact revisions and snapshot scopes observed. Rebinding identical values from different revisions can therefore reuse the computation while producing new provenance; it cannot mutate the old evidence in place. Native units retain their stricter RFC 124 identity rules.

The first evaluation does not yet have an observed read set. Its **claim key** is therefore distinct from the subsequent minimal reuse evidence: it includes the producer/output key, normalized declaration input, and compiler-prepared final input/scope roots. A persistent namespace or candidate-set root can conservatively cover dynamic selectors without decoding every candidate. Do not include an entire physical manifest, home id, or unrelated output/trace roots. Preparation certifies that this envelope covers every value the producer can read. Once an evaluation exists, validate its narrower observed witnesses before deciding whether any computation is needed. A changed conservative envelope does not force recomputation when the old witnesses still validate.

Bake-once applies to equal completed claim keys. Two previously unseen, different conservative envelopes can independently produce the same semantic value; the store deduplicates that value, but the engine must report both computations rather than claim that it predicted their equivalence. Native units have their complete RFC 124 key before execution and need no such discovery qualification.

Changed provenance or a changed dependency read set with an equal semantic value must publish the new evaluation/read witnesses, but does not dirty semantic consumers of that value. Consumers of provenance, coordinates, or membership subscribe to those distinct projections and are invalidated when those projections change. This is not permission to discard source information from debug output or diagnostics.

The forward and reverse indexes publish with the evaluation. Reverse lookup is by logical key and read kind, filtered to reader evaluations selected in the target view. A global reverse index must not propagate work into another project's unselected revisions.

### 4.3 One ranked sweep

The ordering follows DD-0012: imported namespaces first, then compiler phases. Within a phase, any required sub-order is supplied by the compiler's dependency/rank descriptor, not inferred by Oven from source text. Legal recursion follows declaration/signature references rather than a body-to-body evaluation cycle. Indirected recursive layouts retain the language core's rules.

There are two different activities:

1. **Discovery/preparation:** parse changed source, establish namespace membership and compiler-owned stage demands, and finalize earlier-phase inputs. This does not claim to be free and is counted separately.
2. **Value evaluation:** run a dirty producer once, after all of its possible input suppliers for that stage are final, and publish its observed read set with the result.

For checking, namespace/header/layout stages establish the final source-unit environment before body checking. The checker may discover which final signature or name slot it actually reads while evaluating a body. It must not cause that supplier to be recomputed afterwards. Frozen Loaf suppliers are already final and are loaded by key. For lowering, a compiler-owned walk of checked Body IR establishes required runtime facades and instance/layout demands before the function is evaluated; it does not lower the whole program speculatively to discover misses.

Dynamic selectors can expand preparation in earlier ranks. Their descriptors record the selector facts they read. The value producer is not marked ready until that preparation is settled. A value evaluation unexpectedly requesting an unfinished supplier returns a typed producer-contract failure: its output is not published, and the run cannot claim one-pass success. There is **no automatic whole-program lower-again loop** in this engine. This deliberately makes the compiler's preparation contract a stage prerequisite rather than hiding incomplete integration behind retries.

For a new source epoch:

1. Capture changed source/configuration/namespace values and compare semantic projections.
2. Read reverse edges only for changed projections in the selected view; union direct dirtiness with already pending predecessors.
3. Settle preparation and process dirty ready records in rank order. A record is ready only after all dirty predecessors that can affect its stage are final.
4. Validate the prior evaluation's witnesses against final inputs. If all observed values still match, reuse the answer and rebind evidence where needed without invoking the value producer.
5. Otherwise claim the completed computation key, recheck for an admitted result, evaluate once, and publish the result and observed edges together.
6. Propagate only changed subscribed values. Equal values stop that path; another changed predecessor can still require the consumer.
7. Derive native-unit descriptors from the finalized selected record roots. Publish the terminal result binding against the expected project view revision.

Scheduling can parallelize an antichain. Publication order between independent results may differ, but canonical records and final selected roots must be deterministic. The single-writer manifest section is short; neither checking nor rustc runs under it.

### 4.4 Reads and graph correctness

Every logical read reports one of:

- `Found(key, revision, value_digest, selected_projection, scope)`;
- `Absent(name_slot_or_exact_key, namespace_selection, absence_witness)`;
- `Members(range_or_set_selector, membership_digest, cursor_bounds, scope)`;
- `StoreError(kind, object_identity)`.

An absent import records the actual namespace and selection context, not a global missing string. Overload selection, trait candidate discovery, and enumeration depend on candidate-set membership even when the selected candidate remains the same. A bounded query also records the range boundary and ordering facts needed to detect an insertion that would change its answer.

Witnesses are canonicalized and deduplicated per evaluation; large sets spill into paged staging records. Failed speculative reads that can affect the result remain witnesses. Engine scheduling metadata is not substituted for actual compiler reads.

### 4.5 Commands become asks

| Command | Requested roots | Work it must not imply |
| --- | --- | --- |
| `build` / `oven build` | Selected terminal carrier and native unit; prerequisite semantic records | Packaging a `.loaf` or executing the result |
| `oven bake` | Selected loaf-shaped carrier, sealed set, receipt | Running package code during resolution/admission |
| `run` | The same program unit as build, then a separate execution action | A distinct compiled identity merely because the verb is run |
| `test` | Selected test discovery/harness units, then fresh case executions by default | Treating cached test outcomes as executed cases |
| `check` | Compiler checks/diagnostics and required metadata-only provider units | Code generation/linking unless an explicitly selected host semantic capability requires its own executable provider |
| `fmt` | Selected syntax/source records and formatter result | Loading Body IR, native units, or unrelated packages; write-back is a separate conditional source edit |
| Census | A paged collection of the same build/check/test/format/query requests | A separate compilation engine or a second process pool outside the resource budget |
| LSP/MCP/graph | Specified semantic roots/projections at the captured snapshot | Whole-workspace materialization when the question names one unit/record |

The checker still validates every declaration that today's selected compilation scope validates, including invalid but unreachable function bodies. Codegen reachability is a later, different question. A one-body edit can reuse prior checks of other bodies; a cold check may legitimately visit all selected authored bodies. Imported frozen libraries are not rechecked from source.

## 5. Compiler service and the native boundary

### 5.1 Service contract

The following is an interface specification, not syntax claimed to compile today:

```text
prepare(stage, input_snapshot, demand_page, producer_profile)
  -> PreparedInputs(required_keys, rank_certificate, selector_witnesses)
     | NeedPrerequisite(keys, earlier_rank)
     | SemanticRefusal(diagnostics)
     | CompilerFault(details)

evaluate(ready_work, input_snapshot, prepared_inputs, scoped_reader)
  -> Produced(value_ref, semantic_digest, read_witness_root,
              diagnostic_root, counters)
     | SemanticRefusal(diagnostics, read_witness_root)
     | CompilerFault(details)

compile_native(native_descriptor, plan_reader, payload_directory)
  -> NativeOutputs(payload_manifest, receipt, counters)
     | NativeRefusal(diagnostics)
     | CompilerFault(details)
```

A `scoped_reader` implements the keyed read path's intent with stronger outcomes: keyed lookup, `read_many`, bounded field/view access, payload access, and exact witness recording. `read_many` groups known keys by page/segment; it is not permission to load a whole Loaf. Reads happen directly through the same mapped store in the worker, not through one engine RPC per key. The engine grants snapshot and staging handles, not deserialized records.

The current keyed read surface returns table positions and boolean found witnesses for runtime facades. The next adapter must return revision-bound handles and distinguish `Absent` from `NeedPrerequisite`. Its existing string refusal/retry path remains a Path A compatibility implementation; it is not the protocol above and does not qualify as this stage's completion.

### 5.2 Compiler-owned producers

| Producer | Inputs by key | Output | Invocation boundary |
| --- | --- | --- | --- |
| Parse/discover | Source revision and compiler syntax profile | Declaration/source-map pages, namespace candidates | One changed source; one parse, no reparse per declaration |
| Header/signature | Declaration source, final imported bindings, required type/default declarations | One signature or compiler-owned declaration group with explicit membership | Existing checker operation; not engine interpretation |
| Layout/default | Typed declaration facets and earlier layout/default prerequisites | One layout/default value | Recursion rules remain compiler-owned |
| Check body | Body source and final signature/layout/default/name-slot environment | Checked body, diagnostics, read set | One body; context is keyed/read-only |
| Body IR | Checked body and required semantic facts | One Body IR body and supporting typed projections | Existing lowering semantics |
| Instance preparation | Canonical generic arguments and frozen generic definition | Consumer instance keys/demands | No mutation or rebake of the provider |
| Plan function | Body IR/instance plus selected runtime/layout facts | One `PlanFunction` | Does not join all runtime modules |
| Native interface | Selected native declarations, stable CGU membership, exact externs | Native descriptor and native input roots | Traverses the selected native unit, not its complete decoded dependency closure |

A compiler-owned group is allowed only where the existing semantics require a joint operation; its membership and reason are explicit, bounded, and visible in counters. It must not become an excuse to label a whole project one record. Before engine replacement, the edit fixtures in §13 must demonstrate per-body checking/Body IR/plan evaluation. Any unavoidable larger dependency group is part of the affected closure and is reported, not omitted from the accounting.

Parsing, language checks, MIR construction, refusal wording, and diagnostic order remain the current compiler's authority. Diagnostic templates reference stable source anchors plus source-map revisions; rendering resolves current captured coordinates. Reusing a semantic answer cannot reuse stale line/column positions.

### 5.3 Rustc units and codegen membership

The engine completes the full RFC 124 native unit identity before claiming compilation: producer/toolchain/SDK identity, target/host domain, normalized profile and compiler flags, selected semantic/native input projections, exact dependency native identities, features, and applicable declared producer products. Paths that are only locators are excluded; paths or environment values observable by compilation are retained according to the compiler's identity contract.

Command names, request ids, home/view ids, temporary output paths, and package version strings with no observed compilation effect do not enter the key. Program arguments and test case selection affect execution, not compilation unless they change a harness or compiled feature selection.

For native dependencies passing through the pinned stock rustc route, **external identity equals unit identity** until the compiler provides a proven narrower compatibility contract. RFC 124 explicitly requires this fallback. Semantic signature stability alone cannot authorize relinking a changed rlib into an unchanged dependent. Identical units across projects still share fully.

The compiler owns stable native definition and instance identities. For a selected codegen profile with `N` partitions, group identities map deterministically to partitions using a versioned full-identity hash modulo fixed `N`. `N` is part of the profile and never changes automatically with source size or worker count. Required co-location groups are supplied by the compiler. Membership pages are sorted by full identity. A new function changes its partition membership, not every partition; a profile change legitimately creates a different partitioning and native unit.

`CguMembership` is not a promise that the engine can link arbitrary per-function rustc outputs. Native compilation remains one valid rustc unit with its ordinary metadata and linkage rules. Cross-unit ABI changes still invalidate dependent native units under the conservative external-identity rule.

### 5.4 Tracked native inputs and incremental state

The current adapter passes an entire `Plan` to rustc and exposes it through process-global state while overriding native queries. A record-backed `PlanReader` replaces that ownership pattern: callbacks obtain the exact selected definition/layout/function records. The reader lives for the native invocation; rustc's own typed arenas may retain the data they necessarily construct until that invocation ends.

**Do not enable rustc incremental merely because plan functions have hashes.** Every overridden query must have a tracked dependency that changes whenever its answer can change.

The selected bridge design is a compiler-owned per-definition input fingerprint embedded into the synthetic native definition's tracked input representation before rustc hashes it. The fingerprint includes the exact plan/body, signature/layout/native attributes, and other external records consumed by that definition's overridden queries. Each override reads that tracked input through the pinned rustc query interface, not directly from an untracked global lookup alone. Non-definition-wide inputs get an explicit tracked unit input. The adapter maintains an audited table of every override and its tracked input. A query with no established tracking contract disables incremental reuse for that unit.

The exact native API belongs to the pinned rustc adapter; this design does not invent a stable public rustc method for it. The architectural requirement is testable: changing any injected answer must invalidate its native consumer; changing one unrelated body must not invalidate all native bodies. If the existing toolchain cannot satisfy it, native incremental remains off and stage E6 is blocked, not silently weakened.

When qualified, rustc incremental state is an **optional derived seed**, not semantic authority. Store an immutable seed payload tied to the exact toolchain/incremental-format, native unit lineage, flags, and exact extern compatibility envelope. A job clones it into private writable scratch. Canonical seed files are never mutated. Rustc validates its own dependency graph against the new tracked inputs; a missing or unusable seed means an ordinary native compile. A seed selected from an incompatible envelope is refused before invocation.

Seeds are local, non-distribution capabilities initially. They participate in roots/GC as derived payloads and do not change unit identity. If using a seed produces different payload bytes for an equal unit identity, apply the §6 conflict rule; never treat those variants as interchangeable. Seed selection also pins the exact external payload compatibility envelope. Native metadata/codegen is never replayed solely because an engine record was unchanged.

### 5.5 Source coordinates and debug fidelity

Comment-only and coordinate-only edits should leave semantic checking and lowering values unchanged. That does not automatically prove an identical debug artifact: the native route enables debug information. If the selected native profile observes changed source coordinates, its debug/native input changes and the engine must preserve that effect. A compiler-proven separate debug companion can narrow future invalidation; this design does not assume it exists or strip debug information to improve a benchmark.

Report semantic reuse and native/debug reuse separately. The strongest RFC 124 source-relocation claims remain gated on its compiler-owned identity/coordinate model, not an engine heuristic that deletes spans.

### 5.6 Compiler interfaces and implementation language

The engine calls versioned compiler-owned producer and reader contracts. Their observable identities, outcomes, ownership/lifetime rules and read witnesses remain independent of whether their implementation is currently Rust or later Incan. Wire schemas and durable records carry typed identities, schema-defined values and scoped handles, not Rust heap layouts, process pointers or Rust trait-object representations. An in-process adapter may use native types privately under the pinned compiler generation; those types must not become the engine's durable protocol.

The current checker, Body IR semantics and rustc route remain the bootstrap semantic authority. Replacing an implementation with Incan does not authorize different checking scope, diagnostics, ownership or native behavior. It supplies a new exact producer/compiler identity and must meet the same contracts and cold-oracle tests; the old implementation can remain a comparison oracle until the replacement is qualified. Cross-generation result reuse still needs explicit identity/admission evidence, not merely a matching interface name.

The intended low-level Incan authoring surface is `std.compiler`. Its exact types and callable spellings remain subject to the compiler-authoring spike; the protocol above does not claim those APIs already exist. Compiler policy and producers should become ordinary Incan consumers of that surface. The pinned rustc layer supplies only the primitives needed to express its native types, callbacks, scoped values and queries correctly. The engine must not depend on a permanent authored-Rust implementation of those primitives.

E1 checks the boundary now: service messages and persisted schemas expose no Rust-specific representation, the engine calls the producer interface rather than inspecting compiler internals, and the native-work inventory names each temporary adaptation and its tracked expressiveness gap. E1 does not require the entire compiler rewrite to be finished.

## 6. How Loaves and toolchain bundles contribute data

All sources of reusable facts use the sealed record-set admission contract in storage-note §6. A toolchain bundle is a pinned collection of those sets, not a private initialization route.

Admission proceeds as follows:

1. Verify transport digest, format/profile, publisher/builder/trust evidence, and the manifest's declared source/native capabilities in private staging. Acquisition and verification execute no package code.
2. Verify record and payload digests, exact exported roots, external-set references, and capability closure. Reject an incomplete semantic capability even if a native rlib is present.
3. Compare full immutable identities with the physical directory. Import only missing objects; normalize partially overlapping packs to avoid retaining duplicate canonical records merely for transport convenience.
4. Publish the admitted-set roots and provenance atomically. Admission does not move a project's package/toolchain selection.
5. Resolve a selected plan against the new set. Bind exact namespace/signature/body/native capabilities in that view only after policy and native-unit identity checks.
6. Release staging. Repeating the same admission under the same policy and bindings is a no-op.

A dependency's generic definition may be frozen in an admitted set while a consumer instantiation is a new local fact. A selected Loaf cannot cause the engine to recheck or edit that frozen definition. Swapping Loaf versions diffs namespace/member values and invalidates subscribers, including absent name slots, rather than flattening and rescanning both complete Loaves.

Two views may bind different Loaf versions at the same time. Generic object deduplication does not replace those bindings with a global std pointer. An already imported but unselected DataFusion capability has no effect on a selected lightweight core/db build closure.

Local bake, registry asset, and bundle import converge on the same immutable object identities. Follow the referenced RFC 124 payload rule: if two otherwise valid bakes with equal unit identity produce different payload digests, retain both immutable payload variants under that identity, record the conflict, and mark the unit identity machine-local and ineligible for export. Every plan/selection receipt pins the payload digest it actually linked, including the exact external payload choices used to build its dependents. Existing receipts are never rewritten to select the newer variant. Physical bytes remain content-addressed and deduplicated by their actual digest; there is no longer a unique interchangeable payload for that unit identity.

A conflicted-unit lookup must resolve an exact payload selection from its receipt or explicit validated selection. A unit id alone is insufficient and produces a typed ambiguous-payload diagnostic. Neither newest, first-found nor another caller's choice is a default. Reuse and claim-joining must preserve exact linked external payload selections; requests with different selections cannot join merely because their RFC unit ids coincide. This qualifies claim/reuse compatibility without silently redefining RFC 124's unit identity. Native seeds use the same exact payload envelope.

Preserving a variant does not admit corrupt or malformed output: those candidates are still quarantined as faults. A payload conflict is visible in diagnostics and evidence, prevents interchangeable/portable reuse, and blocks export; it does not delete the second otherwise valid local result. Retention follows explicit variant/diagnostic roots and leases, while the unit's conflict status remains until an explicit policy resolves it. The conflict rule must not be erased merely because one variant is later collected. A stricter reject-the-second-bake policy would require a deliberate RFC 124 amendment and is not adopted here.

Assets are exact matches or misses. Missing assets can be baked from the selected source/producer capabilities; missing required capabilities or failed integrity checks are not approximate-match opportunities. Host-machine-specific native requirements stay machine-local and cannot become exportable merely because their bytes were stored.

## 7. Scheduling, claims, back-pressure, and failure

### 7.1 Ready work and fairness

The scheduler holds a bounded frontier of keys, ranks, priorities, and handles, not a decoded DAG. Larger ready sets, reverse edges, and preparation descriptors remain paged. Work is selected by dependency readiness, then round-robin across active requests within priority classes, then canonical key. Interactive requests receive four scheduling opportunities for every one background opportunity; background work therefore cannot starve. A running native unit is not preempted merely because an interactive request arrives.

An activation batch contains ready records with the same compiler generation, target/domain, producer phase, input scope, and compatible resource class. Its descriptor is a paged list of keys. Batch size is limited by reserved bytes and the ready window, not by concatenating an entire closure. Batching amortizes worker dispatch and shared small context; it does not turn multiple records into one uninspectable cache entry.

The engine is the sole admission scheduler for this physical store. Census jobs and native parallel work acquire its tokens rather than creating nested independent pools. An explicit `--jobs` remains a caller upper bound; it does not multiply the global budget for every caller.

### 7.2 Resource model

Let `C` be the OS-allowed logical CPU count, further reduced by an explicit engine jobs setting. Let `H` be the effective physical/cgroup memory limit. The initial service working-memory budget is `M = floor(0.75 * H)`, bounded downward by an explicit working-memory setting and current memory pressure. If `H` cannot be determined, require an explicit budget instead of guessing an effectively unbounded worker count. These are scheduling budgets, not artifact-store size caps or statements of optimal compiler memory. Independent physical stores do not share this scheduler; their budgets must be configured under the host/OS limit rather than each being assumed to own the machine.

Each job reserves at least one CPU token and a predicted working set. Predictions are keyed by producer/toolchain/profile and input-size class, using measured high-water observations where available. Unknown lightweight record work starts with a `min(256 MiB, M)` reservation; an unknown native job starts with `min(2 GiB, M)` and runs alone until measured. Reservations include the worker's baseline and read/decode windows, not just the expected new allocation. A known job larger than `M` is not silently overcommitted: run it alone only with an explicit larger budget, otherwise report the required budget. A smaller machine is not promised to compile every project.

Record work uses one CPU token per worker. A native job reserves its configured LLVM/codegen parallelism; the adapter passes the corresponding bounded parallelism to rustc where supported. When a toolchain cannot bound its native threads, that job reserves the whole CPU budget and runs alone. CPU accounting includes child linker/tool processes.

Do not change codegen-unit count or other output-observable flags to fit an available slot. Native parallelism settings that affect compiler output are fixed in the native recipe and identity before claiming; a scheduling-only permit mechanism may vary execution concurrency only after its output-independence is qualified. An unbounded native tool running exclusively is a containment fallback, not proof that the CPU-budget gate passed. The initial qualified native profile can use one native worker thread, with cross-unit parallelism supplied by the engine, until broader native token sharing is proven.

Admission requires that running reservations, live mapped/decode windows, and engine control buffers fit the working budget. Observe physical pressure and actual process footprints while jobs run. Stop admitting new jobs when reservations are exceeded; cancel with a resource error rather than silently OOMing the service if the configured hard limit is reached. Completed observations improve estimates but never change semantic or native identity.

Working budgets may evict unborrowed mappings, postpone work, or require a larger explicit allocation. They must not delete reachable store objects. The efficiency report shows reservations versus actual peaks, concurrency, CPU utilization, and wait reasons.

### 7.3 Claim-once protocol

Claims use physical-store locks keyed by full completed computation/native identity. Claim filenames may be bucketed, but hash prefixes are not identity authority. Locks have persistent identities and are never unlinked while contenders may hold them.

The order is:

1. Finalize prerequisites and complete the work identity.
2. Ask the global result directory, then apply the requesting view's admission policy.
3. Acquire the claim without occupying a compiler slot; re-read a fresh snapshot and recheck after acquisition.
4. Reserve worker resources. If resources are unavailable, release the claim and requeue; do not hold claims while waiting indefinitely for memory/CPU.
5. Compute into private staging, validate all outputs and read witnesses, and enter the short publication transaction.
6. Revalidate computation read/write/absence/range witnesses against its finalized captured inputs and commit the shared immutable result and evidence under a protected result lease. Rebase disjoint physical-store changes. A caller's mutable latest head is not a precondition for this shared-result transaction. Apply §6 if a native payload conflicts.
7. Release the computation claim. For each remaining interested caller, independently validate admission and exact payload compatibility, then compare and update that caller's expected view revision in a selection transaction. Issue handles bound to that caller's snapshot and lease. A failed or superseded selection does not roll back the shared result or another caller's selection.
8. Complete each caller with its own selection outcome. Release the shared result lease only after every interested caller has either acquired its own protected roots/lease or reached a terminal outcome. Durable unresolved publication/selection transactions retain recoverable pins until resolved; they do not require replaying the computation.

A producer holds at most one claim and never waits on an unfinished prerequisite while holding it. Preparation discovers prerequisites first. Native units wait for all exact externs before claiming. This avoids dependency lock cycles and makes bake-once meaningful across virtual homes.

Healthy, identical inputs in one physical store produce at most one successful computation for a completed identity. A crash can waste an unfinished attempt; the report distinguishes attempts, completed computations, and committed distinct results. It does not claim exactly-once execution through arbitrary process failure.

### 7.3.1 One computation with independent callers

Consider homes A and B selecting the same admitted compiler, native recipe and inputs. Their requests join one claim. While that bake runs, A advances its project view; B does not. The completed immutable result remains valid for the captured inputs. A's head update returns `Superseded` without overwriting its newer selection. B independently admits and selects the same result successfully. A may inspect its old result while its request lease is valid or retain it through an explicit pin; there is no implicit permanent history root.

The same separation applies if A cancels or its admission policy refuses the result: B can still complete. If B's acknowledgment is lost after its selection commits, resolving B's selection transaction returns its committed handles; it does not rebake or reselect A's view. Use separate transaction identities for shared-result publication and each caller's selection, correlated by the shared work id and caller request id.

A lease protects the result through the gap between shared publication and caller selection. GC or compaction during that gap cannot remove objects that a remaining caller may still bind. Each successful caller receives its own scoped lease before the shared protection is released. Publication may optimize compatible transactions into one physical commit only if their independent outcomes and recovery identities remain observable; no all-callers-must-succeed precondition is allowed.

### 7.4 Publication and recovery

The store note owns staging, file and directory synchronization, atomic manifest replacement, leases, and transaction outcomes. The engine never calls a convenient non-atomic file replacement as a substitute. Expensive compilation happens outside the writer lock.

| Failure | Required behavior |
| --- | --- |
| Worker dies before staging completes | Release resources/claim through OS ownership; discard only its proven-abandoned staging; retry side-effect-free compilation under the same inputs if still requested. |
| Worker returns malformed or mismatched output | Quarantine output; retire worker; compiler/toolchain fault, not a cache miss. |
| Publisher dies before commit | No partial result becomes visible. Recovery follows the store transaction state. |
| Commit succeeds but reply is lost | Resolve the shared-result or individual selection transaction id, as appropriate. Return caller-scoped handles only after its selection/admission resolves; do not rebake or relaunch a program blindly. |
| Another request advances one caller's project head | Keep the result for captured inputs under its result/request lease; report `Superseded` only for that caller's head selection. Other callers independently select it. Never overwrite the newer head or roll back the shared result. |
| Snapshot object is corrupt | Return `StoreError`. A compiler-owned repair policy may quarantine and recompute from verified source, in a new explicit transaction; generic lookup does not turn corruption into absence. |
| Native unit id collides with different output | Determinism conflict as in §6; never last-writer-wins. |
| Engine crashes | Preserve committed roots and unresolved effect pins; recover desired work from captured inputs, not saved process heaps. |

### 7.5 Cancellation

A request holds interest in shared work. Disconnect or cancellation removes that interest. If another request still needs the same bake, computation continues. With no interests, record work stops at the next record boundary; a running native job receives cancellation and a two-second shutdown grace before its process group is killed. Its incomplete staging is never selected.

Publication already in its atomic commit section is allowed to finish; cancellation then means the caller detached from an existing fact. This avoids leaving ambiguous partially committed state for a cosmetic prompt deadline.

Execution actions are different. A user program/test is an effect, not a shared compilation. It is not automatically retried, coalesced with another invocation, or resumed after lost acknowledgment. User-requested timeouts apply to the intended case/action; the scheduler does not impose the old five-second batch-compilation split deadline.

## 8. Memory, disk, and views

### 8.1 Read-scope ownership

Workers follow `open snapshot -> begin read scope -> keyed reads -> produce -> drop borrowed views/decoded values -> end scope`. A mapped view owns or borrows the mapping for its full lifetime. Bounds and scalar access are checked; copying a full payload to simulate a borrowed view does not qualify as zero-copy.

The service's idle state contains no checked bodies, ABI maps, deserialized manifests, full key catalogs, or global `Plan`. A worker's request arena owns its decoded inputs, compiler contexts, and mapping guards. Returning a result cannot leave those values in a static cache. The existing B4 mapping/keyed adapters are a starting point, not proof that every current global cache has disappeared.

After each activation, assert zero live record borrows, zero request-owned mapped windows, zero unfinished read scopes, and zero retained decoded semantic bytes. Release allocator pages where the platform supports it. Measure post-request private/physical footprint against the worker's initialized baseline. If logical cleanup fails, terminate the worker immediately. If physical footprint remains more than 16 MiB above baseline for two completed activations, retire it instead of retaining its high-water heap. The threshold is a proposed recycling control, not a claim that 16 MiB is intrinsic or a store limit; its cost is reported in the acceptance run.

If the compiler/runtime cannot demonstrate safe reuse, use the same fixed worker executable as a disposable activation process until that lifecycle defect is fixed. This preserves one compilation route and correctness; it does not pass the warm-worker efficiency gate or justify claiming the intended memory result. No special cold semantic path is introduced.

Large body records may need decoding into the compiler's existing owned types. Record that copy and its necessary lifetime. The zero-copy claim applies only to mapped fields/views actually consumed without copying, not to all checking or rustc's internal arenas.

### 8.2 Physical memory accounting

Same-inode immutable mappings can share clean pages through the OS. A copied file or a filesystem COW clone does not prove the same page-cache sharing. Report virtual mapped bytes, resident mapped bytes, private anonymous bytes, RSS/physical footprint, and proportional sharing where the platform exposes it. Never add lifetime process maxima and call that simultaneous physical memory.

Use a synchronized sample of the process tree plus per-request counters. When an OS cannot attribute shared pages precisely, report the limitation and bounds. Do not replace missing measurement with an assumed division by worker count.

### 8.3 Retained disk growth

An unchanged build with the same selected facts and no explicit observation retention adds **zero retained objects and no new logical result manifest**. It may perform control I/O and verification. New source versions, changed read evidence, executed tests and runs, and explicitly recorded traces are new facts; their costs must not be hidden as ordinary unchanged-build growth.

Default traces and request queues use bounded transient arenas/spools under the substrate's request staging. They are discarded after delivery. This is not a second persistent store. Recording a trace or run observation is an explicit root-producing action. An ordinary successful reuse does not append a timestamped receipt just to say it was reused.

Compaction uses the storage design's deterministic record ordering and profile: equal retained logical contents produce a byte-identical compacted base. Different telemetry histories or retained observations are different input sets, not determinism failures. Physical compaction does not change semantic or native identities.

Collection starts from explicit home/project/toolchain/package/result/evidence roots and live scoped leases. There is no age-based or capacity-based deletion. Dropping a test fork removes its root; it does not immediately unlink bytes that another view or running process still needs. Current project results remain roots; old results survive only if reachable from an explicit history/evidence/pin policy. That policy is visible, not an unbounded hidden list of every prior build.

### 8.4 `target/` and test scratch

`target/` is an output view, not the compilation cache's authority. It carries handles/manifests and requested materializations. Prefer direct canonical paths for immutable linker inputs and executable payloads when the native loader layout permits. Where stable names or writable outputs are required, create a COW clone; where unsupported, make an explicit copy and count the copied bytes. Never expose a writable hardlink to a canonical artifact.

Removing `target/` does not invalidate an otherwise admitted native unit. Recreating it materializes the same selected artifact. A relocated path is normalized only if the compiler says that path is unobserved; debug, `argv[0]`, resource lookup, and loader-relative paths retain their actual behavior.

A test scratch directory contains mutable fixtures and view descriptors, not a duplicate std/Loaf store. Tests that intentionally mutate toolchain/store state receive an isolated virtual view and controlled writable projections. Tests of physical corruption or store initialization use separate physical stores deliberately; they cannot share the object under destructive test.

## 9. Run, test, and effect evidence

### 9.1 Launching a program

`build` and `run` ask for the same program unit. `run` then requests an execution ticket with exact binary/loader roots, arguments, cwd, environment policy, and input snapshot. The CLI launches the native program directly; no generated launcher or new executable format is introduced. The ticket pins the needed payload/loader closure for the entire execution, including libraries loaded after startup.

Before launch, persist a small recoverable execution pin. The launcher records the child identity/process birth evidence and process group, then waits and releases the pin only after observing termination. The pin is deliberately more conservative than a read lease owned solely by the CLI: if the CLI is killed while its child survives, the child must not lose future lazy-loaded files to GC. An ambiguous launch window keeps its pin until recovery proves the child is gone or an explicit operator action resolves it. A PID alone is not proof because it can be reused.

This gives at-most-one automatic launch attempt, not exactly-once external effects. On disconnect the engine can cancel a known child group; if its state is uncertain, it reports unknown and retains the pin rather than retrying. Normal completion removes transient launch metadata; recording execution history is opt-in. These transient durable pins are necessary effect-lifetime bookkeeping, not a new semantic build manifest on every run.

Use the canonical assessed executable inode on repeat runs where observable behavior permits. Do not overwrite that inode with different bytes to evade platform assessment. Do not disable security policy, strip provenance attributes, or assume a fixed host `dlopen` avoids assessment: the local first-launch experiment measured a fresh dylib paying it too. A legitimately new executable can still pay the OS cost; the engine measures it separately from compilation.

### 9.2 Test compilation and execution

Test discovery produces typed case records with stable case ids, entry handles, fixture roots, expected behavior, execution envelope, and batching eligibility. Compile together only cases sharing target/domain/toolchain, native features/profile, compatible initialization, and namespace/entry semantics. Existing compiler eligibility and refusal rules remain authoritative; the engine must not approve batching by a superficial text scan.

A batch harness has a deterministic ordered entry map and native identity. Additions change its membership and therefore its identity. Each case launches a **fresh process** of that shared binary with its selected entry and its own scratch/cwd/env/timeout. Static state, crashes, panic hooks, and output remain isolated per case. Cases observing incompatible process/entry behavior stay separate; negative compilation tests retain their own diagnostic contexts.

Partition descriptors are reusable planning facts, not cached results. Their key includes compiler eligibility profile and exact fixture/entry projections. A deterministic incompatibility splits the group at the named boundary; a compiler crash, resource error, or transport failure is not treated as proof of semantic incompatibility. There is no automatic five-second compile timeout that repeatedly bisects valid groups.

The minimum executable count is the number of distinct compatible native harness/program identities, not the number of source files or cases. Fresh processes of an already assessed binary are expected to be cheap relative to fresh binaries, but actual platform timing remains measured.

### 9.3 Test facts and replay

Separate three records:

| Record | Meaning |
| --- | --- |
| `TestDefinition` | Immutable case/fixture/expected-result/runner-policy identity |
| `TestObservation` | A particular execution attempt: exact binary, input envelope, stdout/stderr digests, exit/signal, duration, environment evidence, outcome |
| `TestEvidence` | An observation qualified for a stated reuse policy, with complete controlled-input and tool/runner identities |

Replay is allowed only for explicitly replayable cases whose relevant inputs and effects are controlled: exact code, fixture, case, runner, target, toolchain, selected environment, clock/randomness policy, and external interaction policy. An undeclared network, ambient clock, uncontrolled filesystem, or unknown environment dependency makes the case non-replayable. Saving a digest of unobserved inputs does not prove control.

The default `test` and the acceptance census execute afresh. An explicit evidence-replay mode returns `REPLAYED`, never `PASS (executed)`, and names the original observation. A user can inspect historical outcomes without treating them as current validation. Compiler deterministic refusals may be reused as semantic results with their full read witnesses, but acceptance exercises the requested fresh compiler path when measuring it.

Actual executions create new observations. If retention is not requested, stream them to the caller and discard the transient view after completion. A retained evidence root deliberately keeps its supporting snapshot and artifacts reachable; the report shows that storage cost.

## 10. LSP, MCP, graph, and contract readers

All consumers use the same snapshot/read contract. The LSP creates an overlay view for unsaved buffers, asks for bounded semantic products, and discards responses whose document versions no longer match. Completion depends on namespace/candidate-set witnesses; diagnostics depend on checked-body and coordinate projections. It keeps handles, not an independent decoded compiler world.

MCP and graph requests must specify scope, projection, ordering, maximum rows, and maximum bytes. Default pages are at most 256 records and 1 MiB, whichever is reached first; a single oversized record requires an explicit field/payload request. These are response budgets, not limits on database contents. Relationship traversal follows paged adjacency with an explicit direction/depth/work budget and a continuation cursor. No separate graph language or whole-graph JSON export is required for this engine.

The contract runner consumes typed pages for the precise assertions it needs. A bounded streaming compatibility renderer can still produce the existing public JSON contract where required; the runner must not parse a 64 MB whole-tree answer to answer a single-unit question. Fixtures expecting an existing complete export keep that explicit request and are measured as such, not silently weakened.

Read-only queries do not materialize native artifacts or trigger arbitrary compilation. A reader explicitly requests semantic work when a required record is not yet produced. `Absent`, `NotProduced`, `ExpiredSnapshot`, and corruption remain different outcomes. A reader can hold only a snapshot handle between queries and still preserve its promised future reads through the scoped lease.

Operational trace queries belong to Oven inspection; semantic codegraph queries remain Incan/compiler products. A future IncQL adapter can expose these same typed relations. The bootstrap engine does not pull in a relational planner, Substrait, Protobuf, or DataFusion to inspect its own records.

## 11. Built-in profiling and direct efficiency proofs

### 11.1 Trace schema and transport

Every logical operation emits structured spans/counters from the start: capture, source read/hash/parse, namespace lookup, record read/batch/decode, preparation, checking, Body IR, plan-function lowering, native codegen/link, admission, claim/recheck/wait, mapping, publication, materialization, launch, query, cancellation, and cleanup.

`EngineSpan` includes run/request/work ids, parent span, producer/operation, key or digest, input snapshot, monotonic start/end, process/thread, CPU time where available, outcome, and byte/record counters. `ResourceSample` records synchronized process-tree observations and phase boundaries. `ReadWitness` is semantic evidence, not a substitute for timing. Sensitive environment values and source contents are not copied into traces; their permitted identities and redacted locators suffice.

Per-scalar field loads use counters inside the enclosing record-read span; they do not allocate an event object per byte or field. Batched reads have a batch span plus keyed read evidence. Every completed logical operation is attributable; events are not silently sampled away. A bounded producer buffer drains to a transient paged trace view and applies back-pressure if necessary. This overhead is itself timed.

Clock durations are monotonic within one run. Cross-process traces share the host monotonic clock contract, and elapsed durations are not reconstructed from wall-clock timestamps. No trace ever enters compared program stdout/stderr. Enabling observation must not change fixture success or diagnostic text.

Default trace retention is request-scoped. Explicit `--record-engine-run` retains a `RunObservation` root. Operational surfaces are `oven inspect run <id>`, `oven inspect run <id> --spans ... --limit ...`, and `oven inspect run <id> --proofs`. A command's `--engine-report` prints the proof summary on a dedicated requested report destination, never implicitly appending it to the program console. Existing `OVEN_TRACE` can export a compatibility stream from these records. These are proposed extensions to Oven's inspection surface, not shipped command claims.

### 11.2 Proof rows

The run report prints `proved`, `failed`, or `unmeasured` for each property, with counts and evidence handles. Empty counters are not proof.

| Property | Required evidence |
| --- | --- |
| Computed once | By completed computation key: attempts, actual producer calls, commits, hits, and crash retries; healthy duplicate successful producers = 0 |
| Stored once | Unique full canonical identities versus retained copies/bytes; transport duplicates and writable views reported separately |
| Selected work only | Requested roots, prepared/visited/evaluated record counts, dirty closure, and reasons for every native unit |
| Early cutoff | Changed input/read sets, equal-value stops, consumers not evaluated, and a cold recomputation oracle |
| No whole-closure reads | Bytes and records read per family/package, unrelated selected-but-unused Loaf reads, maximum decoded/mapped live set |
| Nothing semantic held idle | Zero owned semantic records/borrows/mappings after release plus measured private footprint and worker retirements |
| Shared physical pages | Same canonical file identities plus supported synchronized sharing measurements; not sums of RSS |
| No-change disk growth | Before/after object and binding roots and retained byte counts, with observations/effect pins separately classified |
| Cheap virtual forks | Root/page deltas, no copied payloads at fork, independence after divergent updates |
| Parallel efficiency | Runnable backlog, CPU-token utilization, claim/memory/CPU/publication waits, critical path versus wall time |
| Native reuse correctness | Tracked-input coverage and edited-vs-cold result comparisons; CGUs actually regenerated, not merely engine plan hits |
| Console integrity | Byte-for-byte outputs with profiling off/on and correlated protocol replies |

Example output shape, with no invented measurement:

```text
Engine run <id>  source <digest>  view <revision>
Semantic evaluation: <actual> / <dirty-required>; duplicate producers <count>
Native units: <hit> reused, <baked> baked, <waited> joined
Read/decode/map: <bytes> / <bytes> / <peak-live-bytes>
Idle retained semantic records: <count>; private-footprint delta: <bytes>
Retained build-store delta: <bytes>; observation delta: <bytes>
Proofs: <proved> proved, <failed> failed, <unmeasured> unmeasured
```

A structured query API filters spans by operation/key/parent, follows read evidence, and pages critical-path and reuse reasons. It uses indexed record families in IncQL-DB, not a new analytics database. Full trace retention is an explicit new fact and can be collected by dropping its root.

### 11.3 Cost model and targets

For a request, account for:

```text
T = capture + selection + prepare + indexed_reads + necessary_compute
  + claim/resource_wait + native_work + publication + materialization
  + launch_assessment + program_time + observation_overhead
```

Warm record reads should be dominated by already resident index/frame pages and typed access, not milliseconds of RPC setup per record. The earlier 6-16 ms figure is an unverified activation-level envelope, not a record-read budget. Opening ten thousand records must not incur ten thousand service handshakes.

The 2026-10-29 baseline figures are diagnostic anchors, not a current-head baseline: approximately 35 ms loading std facts, about 80 ms traced lowering in that setup, tiny-process peaks around 250 MB, and first-executable launches around 145-203 ms. The first-launch experiment also found a fresh clang program median near 140 ms and a much cheaper repeat launch. Those are local observations, not universal platform constants.

The new engine must remove unnecessary work by counts first: hello loads only its requested runtime/semantic records, an unchanged build invokes no semantic producer/native compiler, and a private-body edit evaluates only its affected semantic closure. It must then beat or match the pinned integration tree under §13. The brief's 3 ms hello lowering and the historical 11-15 ms retained-owner unchanged path are useful comparison targets; missing them requires a measured cause, not an invented success threshold.

## 12. Migration and ownership boundaries

Use a single development selector, `INCAN_ENGINE=legacy|records`, inherited by child tools and recorded in the run. It chooses the complete invocation before work starts. Default remains legacy until acceptance and approval. This keeps existing command arguments and CI scripts intact; the harness selects an engine by environment. The selector is temporary, not a second permanent backend product.

In records mode, unsupported capability or a store/compiler defect is an explicit failure. It never falls through to legacy. Existing valid fixtures remain frozen. A failure is fixed or blocks promotion; it is not removed from the census or hidden by routing that case through the old engine.

During coexistence, legacy stores/ledgers remain legacy authority only for legacy runs. Records mode imports verified source/Loaf capabilities through normal admission, not by trusting old stamp files. Use separate virtual views and explicit physical-store isolation when measuring cold behavior. No in-place format upgrade of Path A's active store is required.

| Existing area | Replacement boundary |
| --- | --- |
| `main.incn` command parsing/selection | Preserve public behavior; call one engine request API |
| `owner_client`, `owner_endpoint`, `owner_service`, `owner_state` | Short endpoint/startup concepts retained; per-project held `UnitDescription`, ledger, and closure state removed after acceptance |
| `compiler_worker.incn` / driver worker loop | Bounded correlated record protocol and request-lifetime workers; retain proven endpoint/console fixes |
| `unit*.incn` / `rust_units.incn` | Keep project/provider semantics; replace whole-descriptor freshness/compilation orchestration with typed selection/native descriptor records |
| Per-project observations/stamps | Immutable evaluation/read evidence and result bindings in the shared store |
| `bakery.incn` / `segment_store.incn` | The engine calls the agreed substrate and admission API; additions to the store go through that API |
| `jec.incn` / timing sinks | Adapter over built-in structured observation, not a competing logger |
| `testing.incn` / contract batch runner | Typed case/harness requests and one resource scheduler; unchanged expected semantics |
| Driver `frontend.rs` / `adapter.rs` | Retain language/native authority; expose record-scoped inputs and audited native query tracking |

Do not delete the old engine while any acceptance row still needs it as the oracle. Once approved as default, run the full census again with the default selector, then remove obsolete owner caches, stamp-ledger authority, whole-closure request transport, implicit cold fallback, and duplicate scheduler paths. Preserve verification tools and compatibility output adapters still required by public contracts. A separate cleanup commit identifies every deleted authority and its replacement.

## 13. Acceptance and head-to-head comparison

### 13.1 Freeze the experiment

Before measuring, record the integration SHA, candidate SHA, compiler/toolchain/SDK and bundle digests, census manifest and PASS baseline digest, exact selected fixtures, source edit scripts, jobs/memory settings, target/profile/features, OS/hardware, group hints, tracing mode, and input-store roots. The integration snapshot inspected for this note is `69efea3e0f803667cd1431a9e49be051e38a9bd7`; it is not a substitute for pinning the actual comparison tree when the candidate is ready.

Run both engines against the same language-core revision and admitted toolchain/Loaf contents. The current engine may improve during development; final acceptance compares against the then-agreed pinned integration tree, not a deliberately slower old baseline. Never attribute a language-core fix or different fixture census to the engine.

Define four states separately:

| State | Definition |
| --- | --- |
| Empty store | No admitted build facts; import/toolchain acquisition costs stated separately |
| Prefilled cold engine | Same admitted bundle roots, no project results, service/workers stopped; OS page-cache state explicitly stated |
| Warm unchanged | Same captured inputs/results, admitted service generation; no timed-result replay |
| Warm edit | One specified source/configuration mutation, with prior roots pinned for comparison |

Do not call a fresh virtual fork a cold store: its backing objects may already be warm and computed. Do not claim a cold OS cache merely because the process was restarted.

Use at least five alternating paired full-census runs per compared state and at least twenty edit-loop samples after separately reported setup. Keep both two-job CI and machine-capacity runs. Report raw samples, medians, p95 where sample count supports it, and dispersion. Rerun noisy comparisons on a quiet host; do not average away an unrelated competing build. Sampling profiles and resource measurements must not alter expected stdout/stderr.

### 13.2 Semantic and durability gates

These are mandatory, not performance tradeoffs:

1. **Zero lost PASS** against the exact frozen integration PASS set; no new FAIL/PEND, disabled case, weakened assertion, or automatic legacy fallback. Existing baseline exceptions are named and unchanged.
2. Exact diagnostics, codegraph assertions, formatting, runtime output, exit/signal behavior, and test isolation remain correct, including profiling enabled.
3. Body, signature, layout, default, namespace insertion/deletion, absent import, candidate-set change, feature, toolchain, and Loaf-version edits match a fresh cold recomputation oracle.
4. One-record edit proves unchanged sibling checks/Body IR/plan functions are reused; changed read sets with equal values update witnesses without inappropriate propagation.
5. Concurrent identical requests in two virtual homes produce one successful result computation and, for deterministic input, one payload, with caller-specific admission/selection intact. Advance or cancel A while B remains interested: A is superseded/cancelled and B completes from the same computation. Force GC between shared publication and B's selection, and lose B's selection acknowledgment: leases preserve the result and transaction resolution recovers B without a rebake.
6. Crash injection at every staging/publication/ack boundary yields old-complete or new-complete snapshots, never partial visibility; retries do not replay program effects.
7. Fork/drop/GC/compaction preserve leased readers and running loader closures; corruption is not misreported as absence.
8. Repeat sealed-set import is a no-op and overlapping Loaves deduplicate. Incomplete capabilities and corrupt outputs fail closed. Inject two valid unequal payloads for one unit: both are retained, the identity becomes machine-local/non-exportable, existing receipts retain their exact payload, ambiguous unit-only reuse is refused, and dependents never silently switch external payload variants.
9. Cancellation of one shared interest does not kill another; cancellation of an unshared effect is reported accurately, including ambiguous launch recovery.
10. Edited incremental native results agree with non-incremental cold outputs/behavior and show complete tracked-input coverage before native incremental is enabled.

### 13.3 Efficiency gates

Every efficiency-ledger row needs necessity, equivalent-work rate, CPU utilization, and top-self-time evidence. A low total wall time does not excuse repeated work hidden behind parallelism.

| Scenario | Required structural result | Comparative result |
| --- | --- | --- |
| Hello cold, prefilled | No full std/ABI/runtime join; selected records and native units only | Lower activation/read/lowering cost than pinned integration; native/OS-launch floor reported separately |
| Unchanged build | Zero value evaluations and native bakes; no retained build-store growth | Median and tail no worse than integration within measured noise |
| One private-body edit | One affected body check/Body IR/plan evaluation, plus justified compiler dependencies | Improved affected-stage time; native regeneration matches tracked dependency evidence |
| Signature/layout/default edit | Exact affected closure, no stale absent/member witnesses | No unexplained whole-workspace recomputation |
| Two sharing homes | One physical result, one successful healthy bake per identity | Claim waits replace duplicate compilation; no duplicate retained closure memory |
| 1x/2x/4x unrelated admitted packages | Constant demanded-record set and no decoded unrelated closure | No growth proportional to unrelated package contents in warm query/hello work |
| 1x/2x/4x demanded records | Indexed/paged work scales with demanded records/bytes | Explain index/log factors and any superlinear compiler pass; do not call it constant-time |
| Contract graph/LSP | Bounded unit/field pages | No 64 MB whole-tree answer for a bounded question; no whole-tree runner parse |
| Cold/warm census and CI phases | No compiler Loaf rebaked under the same identity; no timer-induced batch explosion | Each phase no worse outside paired noise; total improves |
| Idle workers and forked views | Zero retained semantic heap/mappings, cheap COW roots | Measured post-request footprint/page sharing and copied bytes meet the stated ownership model |
| Instrumented versus uninstrumented | Identical outcomes, complete correlated evidence | Aim for <=5% default observation overhead; above that blocks efficiency acceptance until explained and approved |

The 5% observation figure and recycling/scheduling defaults are engineering budgets, not measured results. The acceptance report must show whether they hold. Store content itself has no imposed size cap.

### 13.4 Replacement condition

The new engine becomes default only when all semantic/durability gates pass, every mandatory efficiency proof is measured rather than assumed, no comparison row shows an unexplained regression, and the maintainer approves the report. A compiler-native limitation can block an efficiency stage; it cannot be hidden by changing the test or labeling untracked incremental reuse safe. Any deliberate scope reduction or unachievable gate requires an explicit design amendment before promotion.

## 14. Implementation stages and exits

The substrate S1-S4 stages keep their names. The engine stages below are E1-E7 so the two tracks do not accidentally claim one another's completion.

### E1. Hello through the actual architecture

**Dependencies:** S1 publication/claims/errors; the S2 subset for keyed indexed reads, scoped views/leases, mapped-view lifetime, and admitted semantic/native std roots. If these are unavailable, implement their generic substrate prerequisite first, not a private engine cache.

Deliver the service, one worker, bounded protocol, one virtual project view, keyed compiler adapters for the minimal hello declaration/body/runtime set, native descriptor/claim/publication, execution pin, and built-in proof report. One compiler worker and one native unit are sufficient. No general registry or parallel scheduler is necessary yet.

Run `incan run hello.incn` in records mode from a prefilled store. It prints the same output as legacy. Repeat it and prove zero semantic evaluations/native bakes while still launching the program. Change one body and show only the relevant records change. Submit the same request from two virtual homes and show one claimed bake. Advance A's view while the bake runs: report A as superseded and let B select the result without rebaking. Repeat with cancellation and caller-specific admission rejection. Collect between shared publication and B's selection, then simulate a lost selection acknowledgment; result leases and transaction resolution must preserve B's correct outcome. Retain a snapshot handle, release all record mappings, and read it again. Kill the worker once before publication and prove safe recovery.

**Exit:** These outcomes plus record/read/byte/claim/idle counters. The compiler adapter must not call the whole-program check/lower path and merely repackage its result as records. Until the minimal per-record service exists, E1 is not complete. The §5.6 interface audit also passes: no Rust representation enters service messages or durable records, and each bootstrap adaptation has a named boundary and tracked gap. This is a small end-to-end architectural proof, not a delivery claim for arbitrary programs.

### E2. Ranked semantic graph and general compiler producers

**Dependencies:** E1 and S3's record/edge API. Complete signatures/layouts/defaults/bodies/instances, compiler-owned preparation, negative/range witnesses, typed refusals, diagnostic coordinate projections, and result-view transactions. Replace B4's string/retry discovery on the new route with prepared keyed reads.

**Exit:** Cold-oracle equivalence for the semantic edit matrix, exact producer counts, same checking scope as legacy, no unfinished-input evaluation/restart on the selected acceptance corpus. Legal language cases that do not fit the rank contract block this stage and require a compiler/design finding, not a newly invented language refusal.

### E3. Loaf capability admission and shared native units

**Dependencies:** S2 sealed sets and indexes, E2. Complete bundle/Loaf/local-bake admission, version-selection diffs, exact extern identity, metadata-only/host/target unit handling, and native output views. Use no Cargo acquisition or fallback.

**Exit:** Shared contents imported once, two homes' identical units baked once, different features/native requirements kept distinct, frozen definitions not recomputed, build followed by run causes no new native bake, damaged/missing capability cases fail correctly, and valid equal-unit payload conflicts retain and select variants under §6 without export or silent substitution.

### E4. Scheduler, memory discipline, and lifecycle

**Dependencies:** E3, substrate scoped leases/GC coordination, completed mapped-view API. Add antichain scheduling, bounded ready pages, CPU/memory tokens, fairness, shared-interest cancellation, worker cleanup/recycling, and crash matrices. The S2/S4 owner integrates any generic view/lease additions; the engine does not fork the store implementation.

**Exit:** Parallel and multi-home contention tests, no deadlock or duplicate successful producer, no retained semantic closures idle, correct running-child pins, measured memory/page sharing, COW forks, and byte-identical compaction under equal retained facts.

### E5. Test/run orchestration and bounded readers

**Dependencies:** E4. Integrate harness eligibility/partitioning, fresh per-case execution, explicit evidence replay, LSP overlays, MCP/graph pages, and the contract runner over the same engine API. Preserve existing public output formats through streaming adapters where needed.

**Exit:** The full frozen census with zero lost PASS, unchanged console behavior with tracing, bounded graph assertions, no nested uncontrolled pools, no implicit test-result replay, and fewer new executables for eligible batches without changing case isolation.

### E6. Native incremental proof and performance qualification

**Dependencies:** E2-E5 and the native adapter tracking work in §5.4. Audit every override, introduce tracked per-definition/unit inputs, qualify immutable incremental seeds and fixed CGU membership, and run edited/cold oracle comparisons. Keep native incremental off until these pass.

**Exit:** Safe per-definition native invalidation, observed CGU work, conservative extern compatibility, all head-to-head scenarios and efficiency rows measured. If the pinned compiler cannot supply this, report the exact boundary and keep this stage blocked; do not claim the engine's native edit-loop north star delivered.

### E7. Default switch and removal

**Dependencies:** E6, the acceptance report, and the maintainer's explicit approval. Switch the default, rerun the full acceptance census and CI phases, then remove obsolete authority paths in a separate reviewed cleanup. Keep a recoverable tagged legacy comparison tree and evidence, not two permanent runtime engines.

**Exit:** One default engine, one native route, one durable store authority, unchanged commands/project model, documented supported platforms and failure modes, and no fallback-specific code masking gaps.

## 15. Language/platform prerequisites and bootstrap native work

This design cannot be implemented faithfully by assuming every needed primitive already exists. Verify each prerequisite against the selected development toolchain; source definitions or a type check alone are not native runtime proof.

| Capability | Required contract | Ownership and handling |
| --- | --- | --- |
| Mapped byte views | Lifetime-safe borrowed/owner-anchored slices, bounds/scalar reads, no full copy | Generic `std.fs`/system Loaf work and substrate S2; the prior preparation work is not claimed fully qualified here |
| Durable publication/locking | File and directory durability, atomic manifest replacement, stable process lock identities | Existing std/substrate contract, qualified per platform; no engine-specific replacement helper |
| Scoped virtual views/leases | COW roots, optimistic view publication, view-scoped reachability, future lazy-read protection | IncQL-DB substrate capability; not compiler-aware storage code |
| Typed IPC/process control | Bounded transport, peer identity, cancellation, process groups/birth evidence, exact byte streams | Incan plus existing `rust::` standard/native APIs; any missing language binding is tracked before a workaround |
| Resource/observer probes | CPU limits, effective memory, process footprint, reliable invalidation of observer coverage | Narrow platform probes through existing APIs; unavailable measurements are marked unavailable |
| Compiler record service | Existing checker/Body IR operations exposed at record boundaries, explicit readers/witnesses/preparation | Language-core owners; semantics unchanged, no Oven-side checker |
| Native input tracking | Every injected rustc query answer has a tracked fingerprint and complete dependencies | Pinned native driver adapter; correctness gate for incremental |
| Deterministic schemas/hashes | Canonical typed encoding, full identity checks, bounded decoding | Incan-authored store/engine models using existing standard hashing/encoding |

New engine policy, planning, selection, dependency capture, scheduling, admission orchestration, tests, trace/report logic, and reader APIs are Incan. Generated Rust from Incan is an output, not a hand-authored implementation layer.

The following authored-Rust allowances apply only to the current bootstrap implementation. They are not permanent ownership of a compiler layer by Rust, nor a requirement that Incan 1.0 keep hand-authored Rust. Each adaptation must name its tracked language/interop limitation and the contract that allows its eventual removal. Existing Rust libraries should be consumed through Incan interop wherever it can express that contract. The engine's own policy remains Incan throughout.

The permitted bootstrap adaptations are:

1. **Existing Rust language-core/FFI adapter:** expose the current checker/native compiler objects through safe record-scoped handles/read callbacks where their Rust ownership cannot cross the Incan boundary directly. This is an adapter to existing semantics, not new semantic rules or engine logic. Any missing Incan expressiveness needed for this boundary must be reduced and tracked before introducing a workaround.
2. **Pinned rustc query/AST/MIR adapter:** connect record handles to the existing native route, establish tracked synthetic inputs, retain correct compiler lifetimes, and expose native counters. These use pinned rustc-private types and remain behind the compiler-owned interface, with no scheduler/store policy there. Their current Rust implementation may be replaced by Incan using the qualified low-level authoring surface; native lifetime and query-tracking obligations do not change.
3. **Existing executable bootstrap/linkage:** the minimum entry that calls the Incan-authored record-worker host. Protocol decoding and worker lifecycle policy remain Incan. Retain a tracked replacement boundary for any authored-Rust entry still required; once Incan can express it, the engine must not depend on its former implementation language.

A fourth bespoke Rust service, SQLite side database, generated Cargo project, Python orchestration layer, or untracked compiler workaround is outside this design. If a native adaptation cannot be justified by one of these boundaries, stop and record the limitation for review.

## 16. References

- [DD-0013: The Incan build engine](../../design_records/0013_the_incan_build_engine.md): the accepted summary, stage order and per-stage proofs.
- [DD-0012: Incremental compilation over a persistent semantic store](../../design_records/0012_incremental_compilation_over_a_semantic_store.md): the ranked record graph, frozen Loaves and early cutoff this engine serves.
- [Bakery storage design](bakery_storage_design.md): substrate decisions and the S1 to S4 boundaries.
- DD-0004 (native route through the pinned rustc), DD-0005 (Cargo-free development), DD-0007 (Loaf artifacts), DD-0008 (one Oven in Incan).
- RFC 097, RFC 106, RFC 117, RFC 118, RFC 119, RFC 120, RFC 121, RFC 124, RFC 125.
- Issues #2481 (the bakery) and #2484 (bounded code-graph answers), and the dev.8 reference CI run for #2471.
