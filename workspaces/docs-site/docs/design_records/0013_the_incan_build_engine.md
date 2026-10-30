---
id: DD-0013
title: The Incan build engine
status: Accepted
type: design-decision
date: 2026-10-30
review_target: 0.6.0-dev.9
sources:
  - Maintainer approval of the build-engine design v2 (BE-01 to BE-33, stages E1 to E7), 2026-10-30
  - Bakery-on-IncQL-DB storage design (stages S1 to S4), 2026-10-29
  - https://github.com/encero-systems/incan/issues/2481
  - https://github.com/encero-systems/incan/issues/2484
  - RFC 060 (IncQL), RFC 067 (IncQL), RFC 097, RFC 106, RFC 117, RFC 118, RFC 119, RFC 120, RFC 121, RFC 124, RFC 125
  - DD-0004, DD-0007, DD-0008, DD-0012
---

<!-- markdownlint-configure-file {"MD025": {"front_matter_title": ""}} -->

# DD-0013: The Incan build engine

## Context

dev.7 and dev.8 built the direct native route: Incan source goes through the checker and Body IR straight into the pinned rustc, with no Cargo and no emitted Rust, and passes the full contract census. That route is what makes this engine possible. It also gave the first build engine around it a real workload, and measuring that early engine showed where the time and memory go.

The early engine was assembled step by step while the route was being proved. It treats each compile as a fresh, self-contained job: every process loads whole dependency closures, and nothing is shared between processes, projects or commands. On ordinary tasks that costs:

- **Hello world reads the whole standard library.** To emit 11 native functions, a compile reads all standard manifests (12.1 MB) and requests about 400 bodies (#2481).
- **Processes hold what they no longer need.** An idle compiler worker keeps about 2 GiB. The contract runner grows to 10.4 GiB and `incan mcp` to 3.8 GiB.
- **Small questions get whole answers.** Asking about one unit returns the standard library's whole code graph, 64 MB (#2484).
- **Work is repeated.** `incan run` after `incan build` of the same file compiles it again. A cold CI-equivalent run takes 30.5 min and compiles some compiler Loaves 2 to 7 times. A one-function edit to the lowering library costs 271 s.

None of this comes from the language's semantics; the checker, Body IR and lowering stay as they are. It comes from the parts DD-0012 left open. DD-0012 decided what the compiler stores: typed records keyed by canonical identity, a ranked graph of recorded reads, and one bakery per home. It did not decide:
- the process that serves those records;
- how a command becomes work;
- who schedules and claims that work;
- how memory, disk, tests and readers stay bounded.

With the route proved and the costs measured, this record redesigns the engine end to end and names each part by the job it does. It replaces the per-project owner, the ledgers and the whole-closure transport. The maintainer approved the complete design on 2026-10-30. It is the plan of record, not one option among several.

## Governing rules

These rules govern every component below and every review of its implementation.

1. **One home, no split brains.** Each fact, mechanism and decision has exactly one home. No second cache, ledger, freshness check, scheduler, trace channel or storage path exists beside the one named here. The [Code map](#code-map) names that home per component and flags every place where the reused code still has two.
2. **Simple, directed implementations.** Each component does one job in one direction of dependency. Engine policy calls the compiler's producer contracts and the store's substrate contracts; neither calls back into engine policy.
3. **Clear, deliberate APIs.** Each component's API is named, minimal and documented as a contract: inputs, outputs, outcomes and ownership. No component reaches into another's internals. A typed outcome replaces every string-parsed error.
4. **A platform layer that std builds on.** Implementations may be lower level than usual. The platform owns the primitives (store substrate, mapping, durable I/O, IPC, process control, resource probes, tracing). `std` and the engine are peers on top of it. `std` wraps the platform for users and never reimplements it.
5. **The platform wraps, it does not reimplement.** The platform reaches OS and Rust facilities thinly, through Incan interop. Low level means precise and minimal, not homegrown.
6. **Incan only, no Cargo.** Engine policy, planning, scheduling, admission, tests, tracing and reader APIs are Incan. The only Rust authored for the engine and record service is the three bootstrap adapters in [Language and platform boundary](#language-and-platform-boundary). There is no Cargo invocation, including acquisition, and no Python component.

## Decision

An invocation selects an immutable input snapshot, asks for named results, and obtains handles to facts already present or computed once from final inputs. **One resident engine serves one physical bakery.** Homes, projects, test forks and editor overlays are virtual stores over that bakery. Compiler workers evaluate one record family at a time through keyed, mapped reads. The native route remains the pinned rustc route of DD-0004.

### Layers

--8<-- "design_records/0013_layers.svg"

| Layer | Owns | Does not own |
| --- | --- | --- |
| 1. OS and Rust facilities | `mmap`, file and directory sync, atomic rename, locks, clones, sockets and pipes, peer credentials, process spawn and groups, limits, clocks | Anything Incan-specific |
| 2. Platform | Store substrate (IncQL-DB profile), memory mapping with lifetime-safe views, durable I/O and publication ordering, IPC and peer identity, process control and groups, resource probes, tracing | Compiler meaning, package policy, user-facing ergonomics |
| 3. `std` | User-facing wrappers over the platform (`std.fs`, `std.process`, and peers) | A second implementation of any platform primitive |
| 3. Engine and compiler record service | Requests, ranked work, scheduling, claims, selection, readers, run and test | Storage layout, OS access outside the platform |

### System architecture

--8<-- "design_records/0013_system_architecture.svg"

| Process | Owns | Holds while idle | Ends when |
| --- | --- | --- | --- |
| `incan` / `oven` CLI | Arguments, terminal streams, request interest, the launched program | Nothing beyond its command | Its command or child finishes |
| Engine service | Capture, the ranked work graph, scheduling and tokens, claims, publication, leases and pins, interests, traces | A store descriptor, bounded buffers, worker descriptors and observer control state; no decoded program, Loaf or ABI closure | Explicit stop, or 600 s without client, worker, effect or pinned reader |
| Record worker | One compiler activation at a time | Executable code and small protocol state | Unsafe cleanup, crash, incompatible generation, excess idle footprint, or 10 s idle |
| rustc native worker | One native unit through a record-backed `PlanReader` | Nothing semantic | The native invocation ends |
| Program / test process | Its runtime and effects | Its own state | Exit or explicit cancellation |
| LSP / MCP / graph / contract runner | Document versions, snapshot and cursor handles | Handles only, never a decoded compiler world | The session ends |

The engine is singular per `(StoreId, engine protocol and profile generation)`, guarded by a store-level service lock and startup handshake. The Oven executable carries the internal service entry and the compiler driver carries the internal record-worker entry; there is no new executable and no storage daemon. All durable state is in IncQL-DB. Queues, sockets, tokens and in-flight buffers are disposable control state; a crash recovers desired work from captured inputs and committed results.

### Physical bakery and virtual stores

The physical bakery holds immutable records, revision frames, copy-on-write B+tree index pages (4 KiB, authenticated), content-named payloads, staging, leases, claim locks and roots, behind one atomically switched `CURRENT` manifest. It knows no compiler, Loaf or test type; Oven supplies typed families over the generic record contract.

A virtual store is a set of binding roots selected over that bakery:

| Name | Meaning |
| --- | --- |
| `StoreId` | Persistent identity of one physical bakery; never part of compilation identity |
| `ViewKey` | Stable identity of a mutable logical view (a home catalog, a project head) |
| `ViewRevision` | Content-derived binding-root identity; unchanged bindings give the same revision |
| `SnapshotHandle` | Store, manifest, view revision, allowed root set, lease and format profile; a descriptor, not a mapped closure |
| `BuildEpoch` | One captured source and configuration selection plus its private result view |
| `ExecutionHandle` | Exact native unit, payload and loader roots, and an execution pin |

Forking copies a root descriptor; the first differing binding path copies index pages. Reads use the child's root directly and never walk a parent chain. Views publish by comparing their expected prior revision; there is no implicit merge-back. A home finds its bakery through a small `<home>/bakery.view` descriptor (`incql-db.view/1`: store id, locator, home view key) and nothing else.

Three questions have three answers: **available** (bytes present and intact), **admitted** (origin, producer, format and capability closure accepted by policy) and **selected** (this view binds those exact revisions). A digest hit answers only the first. Scoped leases pin a view's logical roots and the index generations needed to reach them, not every other view's payloads. Virtual stores are same-user isolation, not a security boundary.

### Request protocol

--8<-- "design_records/0013_request_lifecycle.svg"

`incan.engine/1` is separate from `incan.compiler-worker/1`. A frame header is 64 bytes, little-endian: magic `INCENG01`, major and minor version, message tag and flags, 128-bit session id, 128-bit request id, per-direction sequence, payload length (at most 65,536 bytes) and reserved zero bytes. Bodies use a versioned tagged binary schema authored as Incan models; decoders check lengths before allocating and never cast bytes to native layouts. Each direction has eight data credits, with a separate control lane for cancellation and acknowledgment. Records never travel on this channel: workers read them from the leased store.

| Message family | Contract |
| --- | --- |
| `Hello` / `Welcome` | Negotiate profile, service and compiler generation, store identity, peer identity and limits before any work |
| `Submit` / `Accepted` / `Captured` | Command kind, view, input selection and requested roots; acceptance precedes capture; `Captured` returns the immutable `BuildEpoch` and initial snapshot |
| `Prepare` / `NeedPrerequisite` | The worker derives compiler-owned selectors, ranks and demands; a missing earlier-rank input is a scheduling fact, not a diagnostic |
| `Evaluate` / `Produced` | A ready batch of records with one activation scope, or one native unit; staged results, read-witness root, diagnostics and counters |
| `Query` / `Page` | Bounded read-only snapshot queries with snapshot-bound cursors |
| `Event` | Progress or trace handles; never program console bytes |
| `Complete` | Caller-specific committed snapshot, admission proof, leased handles and binding outcome |
| `Cancel` / `Cancelled` | Detach one interest; shared work continues while another interest remains |
| `Resolve` | `Committed`, `NotCommitted` or `Unknown` for a transaction after a lost acknowledgment; never reruns user code |
| `Pin` / `Release` | Scoped reader and execution leases |
| `Drain` / `Stop` | Stop accepting, settle or cancel, release resources, transfer service ownership |

Request states run `Accepted → Capturing → Planning → Waiting/Running → Publishing → Complete`, with `Cancelled`, `Failed` or `Superseded` as terminal alternatives. Work states run `Discovered → WaitingInputs → Ready → Claiming → Rechecked → Producing → Staged → Committed`; a hit skips producing.

### Capture and freshness

Capture turns a request into a `BuildEpoch`: the exact editable inputs the request depends on, each checked as cheaply as its kind allows.

| Input | Examples | How capture checks it |
| --- | --- | --- |
| Frozen | Installed toolchain, standard library, admitted Loaves (registry or local), anything stored under its content name | Never. Each is verified once at admission and cannot change after that; capture does not stat, read or hash its files. Record integrity is still checked on every read (S1) |
| Editable, in reach | Root files, the files their recorded lookups resolve to, `loaf.toml`, the lock, the vocabularies `loaf.toml` declares, the toolchain pin | Metadata first; only candidates are read and hashed |
| Editable, out of reach | Files no recorded lookup reaches, such as unrelated sibling fixtures | Never |

**Reach.** A request names its roots: a file for `run`, a test selection for `test`, an open document for the LSP, the whole package for `build` and `bake`. Reach is the code graph: the recorded ancestors of the requested declarations, whatever their language.

- Every import lookup is recorded as a witness, whether it resolved to a file or was absent. Next time only those exact paths are checked again, so a new file matters only when it appears at a path a recorded lookup resolves to. `incan run file0001.incn` in a folder of 1,500 unrelated fixtures checks that file, its imports and the project configuration, and never lists, checks or reads the other 1,499.
- `build` and `bake` of a Loaf root at the package's whole membership: capture lists the package's source directories and checks every member, metadata first. Test fixtures are not members of a Loaf, so they stay out of that listing.
- Incan code has no action at a distance. What a file means comes only from what it imports, its Loaf's `loaf.toml`, and the vocabularies `loaf.toml` declares; Incan's own syntax counts as one of those vocabularies. Any later construct that can affect a file without being imported by it, such as extensions, must be an explicit edge in that file's reach, and the code graph must show it.
- A namespace assembled from a listing is captured once with its listing as a recorded input. The change observer, and the LSP as edits happen, invalidate it when the listing changes.
- Another Loaf, local or registry, is its own entry in the bakery. It is admitted by copying its content into the store, as `pip install <path>` copies rather than links. Once admitted it is frozen for every consumer. Edits to it are checked only by requests rooted in it, and its consumers see them when it is admitted again.
- Loaves developed together belong to one workspace. A workspace is a single root: its member Loaves' sources are all editable sources of that root, checked by reach like any other source, with no re-admission step between them.
- Rust and C source a Loaf carries itself follows the same graph. The compiler's own dependency report (rustc's dep-info, the C compiler's dependency file) only adds reads the graph cannot see, such as `include_str!`, environment variables and headers. A read that neither the graph nor the compiler reports, such as an untracked read inside a procedural macro, must be declared in `loaf.toml`; otherwise it is outside the reach.

**Freshness within reach.** Capture decides freshness from file metadata first, as the dev.7 retained owner did, where unchanged runs took 11–15 ms. A file whose inode, size, nanosecond mtime and ctime, and mode all match the captured record is unchanged and is not read; only files that differ are read and hashed. While an OS change observer's coverage is valid, it narrows the candidates further, so an unchanged request does not even stat the whole reach. Metadata only detects change; content identity stays the hash of the bytes. Unlinking a hard link changes the surviving inode's ctime, so ctime is never used to verify a file that is being moved concurrently. Capture records the exact bytes it admitted, retries a file that changes while it is being read, and reports an unstable read after three attempts. A no-op request borrows the existing view revision and writes nothing. The proof for an unchanged rerun is zero source reads and zero hashes.

### Rust and C interop

- **No crates.** The engine never deals with crates. Rust and C libraries reach a project only as Loaves, frozen like any other.
- **One IR.** Incan and Rust are not two separate worlds: they reach through the same IR, the unified type substrate of RFC 121. A Rust declaration is a declaration record and its signature a signature record, ranked in the same graph as Incan's. Signatures rank before bodies regardless of language, so a type that crosses in both directions forms no cycle.
- **Interop facts.** For Rust these are what Rust inspection extracts: signatures plus the ownership metadata duckborrowing needs. That means whether each receiver and parameter is taken by reference, mutable reference or value, Copy and Clone, Deref targets, and lifetimes in signatures. For C they are its declarations. The checker and the lowering decide borrowing from those facts alone.
- **Early cutoff across languages.** A Rust or C unit is compiled whole. A body edit that leaves its interop facts equal stops at them, so it never re-checks the Incan code that calls it. A change to a borrow shape changes the facts and re-checks every Incan caller.
- **Both directions (RFC 097).** Incan calls Rust through the interop facts. Rust calls Incan through RFC 097's caller facet. Rust inspection records which Incan exports a Rust unit references. The caller facet is a record derived from those references and the exports' signature records, the caller artifact is built from the facet, and the Rust unit is compiled against it. An Incan body edit that leaves an exported signature equal rebuilds neither the facet nor the Rust caller.
- **Compiler-private access.** This record supersedes DD-0004's rule that only the driver Loaf's own units may use rustc's internal interface. Any Loaf may use it, Incan units as well as Rust facets, by declaring it in `loaf.toml` with the keys a Rust facet uses today: `unstable_features`, `toolchain_components` and `sysroot_dependencies`. There is no further gate. The declaration is part of the unit's identity, so a unit built without it never stands in for one built with it, and the driver honors it only from the unit description, after its identity check. An undeclared unit is refused.

### Ranked work graph

--8<-- "design_records/0013_record_graph.svg"

The graph wraps DD-0012's compiler facts with evaluation provenance. `RecordKey` names the question, `RevisionId` the immutable state, `ValueDigest` the canonical semantic answer, and an `Evaluation` how that answer was obtained. Physical location, request ids and home locators are never semantic inputs.

For a new source epoch the engine:

1. captures changed sources, configuration and namespace values and compares their semantic projections;
2. reads reverse edges only for changed projections in the selected view;
3. settles preparation, then processes dirty ready records in rank order once all their dirty predecessors are final;
4. validates the prior evaluation's witnesses and reuses the answer without invoking the producer when they hold;
5. otherwise claims the completed computation key, rechecks, evaluates once and publishes the value with its observed edges;
6. propagates only changed subscribed values; an equal value stops that path;
7. derives native-unit descriptors from the finalized roots and publishes the result binding against the expected view revision.

A value producer that asks for an unfinished supplier returns a typed producer-contract failure. There is no whole-program lower-again loop. Checking scope stays exactly as today, including unreachable bodies; codegen reachability is a separate question.

| Command | Requested roots | Work it never implies |
| --- | --- | --- |
| `build` | Terminal carrier and native unit | Packaging or executing |
| `oven bake` | Loaf-shaped carrier, sealed set, receipt | Running package code during admission |
| `run` | The same program unit as `build`, then an execution action | A second compiled identity for the verb |
| `test` | Discovery and harness units, then fresh case executions | Treating cached outcomes as executed cases |
| `check` | Checks, diagnostics, metadata-only provider units | Code generation or linking |
| `fmt` | Syntax and source records, formatter result | Body IR, native units, unrelated packages |
| Census | A paged collection of the same requests | A second engine or process pool |
| LSP / MCP / graph | Named roots and projections at the captured snapshot | Whole-workspace materialization |

### Compiler producer boundary

The compiler exposes three versioned contracts; the engine never interprets the AST or parses refusal strings.

```text
prepare(stage, input_snapshot, demand_page, producer_profile)
  -> PreparedInputs(required_keys, rank_certificate, selector_witnesses)
   | NeedPrerequisite(keys, earlier_rank) | SemanticRefusal(diagnostics) | CompilerFault(details)

evaluate(ready_work, input_snapshot, prepared_inputs, scoped_reader)
  -> Produced(value_ref, semantic_digest, read_witness_root, diagnostic_root, counters)
   | SemanticRefusal(diagnostics, read_witness_root) | CompilerFault(details)

compile_native(native_descriptor, plan_reader, payload_directory)
  -> NativeOutputs(payload_manifest, receipt, counters) | NativeRefusal(diagnostics) | CompilerFault(details)
```

| Producer | Inputs by key | Output |
| --- | --- | --- |
| Parse / discover | Source revision, syntax profile | Declaration and source-map pages, namespace candidates |
| Header / signature | Declaration source, final imported bindings | One signature, or an explicit compiler-owned group |
| Layout / default | Typed facets and earlier layouts or defaults | One layout or default value |
| Check body | Body source and its final environment | Checked body, diagnostics, read set |
| Body IR | Checked body and required facts | One Body IR body |
| Instance preparation | Canonical arguments and a frozen generic definition | Consumer instance keys and demands |
| Plan function | Body IR or instance, runtime and layout facts | One `PlanFunction` |
| Native interface | Selected native declarations, stable CGU membership, exact externs | Native descriptor and native input roots |
| Doc and position | Source revision of one declaration | Doc record and position map, outside semantic identity |
| Rust inspection | A Rust source unit and the frozen facts it reads | Declaration, signature and ownership records; the Incan exports it references |
| C declarations | A C unit and the headers its compiler reports | Declaration records |
| Caller facet (RFC 097) | Referenced Incan exports and their signature records | One caller facet record |

The `scoped_reader` offers keyed lookup, `read_many`, bounded field access, payload access and exact witness recording, read directly from the mapped store. Every read reports `Found`, `Absent` (with its name slot and selection), `Members` (with range bounds) or `StoreError`; a missing input is never a semantic absence, and damage is never absence.

Native units take their full RFC 124 identity before claiming. Command names, request ids, view ids and temporary paths never enter it. External identity equals unit identity on the stock rustc route. Codegen-unit membership hashes full identities into a fixed partition count that is part of the profile. Rustc incremental stays off until every overridden query has a tracked per-definition fingerprint; when qualified, incremental state is an immutable seed cloned into private scratch, never semantic authority.

Interfaces outlive their implementation language. Wire schemas and durable records carry typed identities and scoped handles, never Rust layouts or pointers.

### Loaves and toolchain bundles

Toolchain bundles, dependency Loaves, local bakes and registry assets enter through one admission path for sealed record sets: verify transport, format and trust without executing package code; verify digests and capability closure; import only missing objects; publish admitted roots atomically; bind in a view only after policy and native-identity checks; release staging. Repeating an admission is a no-op. Admission never moves a selection.

Imported records are frozen. A consumer instantiation of a frozen generic is a new consumer fact. Two views can bind different Loaf versions at once. When two valid bakes of one unit identity produce different payloads, RFC 124 applies: both variants are kept, the unit becomes machine-local and non-exportable, and every receipt pins the payload it linked. A unit id alone never selects a conflicted payload.

### Scheduling and claims

The scheduler holds a bounded frontier of keys, ranks and handles, not a decoded DAG. Work is chosen by readiness, then round-robin across requests within priority classes (four interactive turns per background turn), then canonical key. The engine is the only admission scheduler for its store; census and native jobs take its tokens.

With `C` logical CPUs and effective memory `H`, the working budget is `M = floor(0.75 × H)` unless set explicitly. Unknown record work reserves `min(256 MiB, M)`; an unknown native job reserves `min(2 GiB, M)` and runs alone until measured. Native output-affecting flags never change to fit a slot. These are scheduling budgets, not store size limits.

Claims are physical-store locks keyed by full computation or native identity:

1. finalize prerequisites and complete the work identity;
2. ask the result directory and apply the view's admission policy;
3. acquire the claim without a compiler slot, then recheck a fresh snapshot;
4. reserve resources, or release the claim and requeue;
5. compute into private staging and validate outputs and witnesses;
6. revalidate witnesses and commit the shared immutable result under a protected result lease;
7. release the claim and, per interested caller, admit and select independently;
8. complete each caller with its own outcome; release the shared lease once every caller has its own.

A producer holds at most one claim and never waits on an unfinished prerequisite while holding it. One caller's cancellation, refusal or superseded head never fails another caller's selection.

### Memory and disk discipline

Workers follow `open snapshot → begin read scope → keyed reads → produce → drop views and decoded values → end scope`. After each activation the worker asserts zero live borrows, mapped windows, open scopes and retained decoded semantic bytes, and is retired if its footprint stays more than 16 MiB above baseline for two activations. Mapping the same canonical files lets processes share clean pages; reports show virtual, resident, private and proportional memory, never sums of maxima.

An unchanged build adds zero retained objects and no manifest. Traces and queues live in transient arenas unless a root is requested. Compaction is deterministic: equal retained contents give a byte-identical base. Collection starts from explicit roots and live leases only; there is no age or capacity eviction. `target/` is a view: canonical payloads are used directly where the loader allows, otherwise cloned copy-on-write or explicitly copied and counted; never a writable hardlink.

### Damaged records

This record supersedes one rule of DD-0012, decision 10: "a missing or damaged record is a miss". The store keeps the two apart. A missing record is `Absent`. A damaged record, index page or manifest is a `StoreError` naming the exact object. The store never turns damage into absence and never falls back to an older value.

Recovery belongs to the owner of the damaged object, never to a generic lookup:

- **A derived compiler fact** is quarantined and recomputed from complete, verified inputs in a new, explicit transaction. Without such inputs, the request fails with the store error.
- **A frozen Loaf record or payload** is fetched again, or replaced by a separately baked equivalent through admission. It is never edited in place. If neither is possible, the compile is refused.
- **A damaged `CURRENT` or selected manifest** refuses to open that state. An explicit repair selects a verified committed root; the store never falls back to an empty or older generation on its own.
- **A verify pass** reports damaged objects; repairing them is a separate, explicit step.

DD-0012's verify mode, which recomputes cold and compares byte for byte, is unchanged.

### Run and test

`build` and `run` ask for the same program unit. `run` adds an execution ticket with exact binary and loader roots, arguments, working directory and environment policy, protected by a durable execution pin until the child's termination is observed. A launch is attempted at most once and never replayed. The canonical assessed executable inode is reused where behavior permits; no assessment or security policy is bypassed.

Compatible test cases compile into one harness with a deterministic entry map; each case runs in a fresh process of that binary. There is no five-second compile split. `TestDefinition`, `TestObservation` and `TestEvidence` are separate records; replay reports `REPLAYED` and names the original observation, and the default `test` and the census always execute afresh.

### Readers

The LSP, MCP, graph and contract runner use the same snapshot and read contract. The LSP works through an overlay view. MCP and graph requests name scope, projection, ordering, rows and bytes; default pages are at most 256 records and 1 MiB. Traversal follows paged adjacency with a direction, depth and work budget. The runner consumes typed pages and never parses a whole-tree answer for a one-unit question. Readers never trigger compilation implicitly.

### Built-in profiling

Every logical operation emits structured spans and counters from the start: capture, reads, preparation, checking, Body IR, lowering, native work, admission, claims, mapping, publication, materialization, launch, query, cancellation and cleanup. `EngineSpan` and `ResourceSample` travel on a separate trace channel and never reach compared stdout or stderr. Default retention is request-scoped; `--record-engine-run` keeps a `RunObservation` root. `oven inspect run <id> --proofs` and `--engine-report` print each proof as `proved`, `failed` or `unmeasured`, with counts and evidence handles. Observation overhead is held at or below 5% of the uninstrumented run.

### Source positions, comments and docs

Comments, docstrings and source positions are separate records beside each declaration. They never enter its signature, layout or body identity, so editing them never re-checks a declaration or anything that reads it.

| Record | Holds | Read by | Identity effect |
| --- | --- | --- | --- |
| Declaration text | Exact source text of the declaration, comments included | Formatter, LSP hover on source | Seeds the declaration; the comment-free semantic value is equal, so propagation stops there |
| Doc record | Docstring and doc attributes, per declaration | Docs generation, LSP hover, `incan doc` | None on semantics; docs generation reads doc and signature records together |
| Position map | Span table from each semantic node to file, line and column, keyed by the declaration's semantic record and source revision | Diagnostics, native line tables | None on semantics; diagnostics and native units bind to it |

- **Diagnostics** are stored against semantic node identities, not lines. A diagnostic is rendered by joining it with the current position map, so moving or annotating code shows the right line without re-checking.
- **Stack traces** need line tables in native code. A native unit therefore reads the position maps of its members. A comment or blank-line edit changes those maps, so it re-emits only the codegen units whose members moved. It never re-runs checking, Body IR or planning. Native units in release and debug profiles behave the same way, so a panic in either points at the current source.
- **Docs generation** is a reader like the LSP: it queries doc and signature records by key through the engine's snapshot surface. It does not compile anything.
- **Loaves** ship their doc records and position maps with their frozen semantic records. Diagnostics and stack traces inside a dependency then point at that Loaf's source, and its API docs come from the same records.

## Key decisions

Each ID (BE for "build engine") names one decision in the full design, which records it with the alternative it rejected.

| ID | Decision |
| --- | --- |
| BE-01 | Virtual stores select copy-on-write binding roots over one shared physical bakery |
| BE-02 | Availability, admission and selection are distinct |
| BE-03 | One resident engine per physical store; native workers run one invocation at a time |
| BE-04 | All durable engine facts live in IncQL-DB; volatile scheduling state is disposable |
| BE-05 | A request pins captured inputs and explicit result roots |
| BE-06 | A versioned, bounded, correlated control protocol; local record reads do not use RPC |
| BE-07 | Source identity, semantic value, evaluation provenance, native identity and physical location stay separate |
| BE-08 | DD-0012's ranked graph with a finalized-input frontier and one value evaluation per dirty record |
| BE-09 | Positive, negative and set-valued reads are recorded with their exact scope |
| BE-10 | Semantic checking scope stays as today, independent of codegen reachability |
| BE-11 | The compiler exposes typed per-record producers and read witnesses |
| BE-12 | A typed missing input is neither a semantic absence nor a corrupt-store miss |
| BE-13 | Native units form from finalized record roots; build and run share the program unit |
| BE-14 | CGU membership is stable by compiler identity and a fixed profile partition |
| BE-15 | Native incremental reuse requires tracked compiler inputs; until proven it is off |
| BE-16 | Toolchain bundles, Loaves and local results share one admission path |
| BE-17 | Claims belong to the physical store and cover completed derivation and native identities |
| BE-18 | Fair, byte-accounted admission and CPU tokens, including nested native work |
| BE-19 | Cancellation detaches an interest; user effects are never replayed automatically |
| BE-20 | Request arenas and mappings are scoped; workers that cannot release them are recycled |
| BE-21 | Explicit roots and leases, never size or age eviction |
| BE-22 | Canonical immutable payloads execute where the loader allows; writable views are clones or copies |
| BE-23 | Compatible tests compile together; every case runs in a fresh process |
| BE-24 | Historical observations, reusable evidence and fresh execution are separate |
| BE-25 | All readers use bounded snapshot queries with typed cursors |
| BE-26 | Every logical operation is instrumented on a separate trace channel; retention is deliberate |
| BE-27 | One invocation-wide engine selector during migration; no fallback after selection |
| BE-28 | Comparisons use the same frozen census and inputs against a pinned integration tree |
| BE-29 | The first stage proves the complete architecture on hello, without a bulk-checker shortcut |
| BE-30 | Language and platform gaps block their stage; no private Rust services |
| BE-31 | RFC 124 equal-unit, unequal-payload conflicts keep variants, mark the unit machine-local and pin exact payloads |
| BE-32 | Compiler interfaces outlive their implementation language; present Rust adapters are bootstrap work |
| BE-33 | A shared result is published once, then admitted and selected independently per caller |
| BE-34 | Comments, docs and positions are records outside semantic identity; diagnostics bind to node identities, native line tables to position maps |

Migration uses `INCAN_ENGINE=legacy|records`, inherited by child tools and recorded in the run. In records mode an unsupported capability or defect is an explicit failure, never a fall-through to legacy.

## Stages and order

--8<-- "design_records/0013_stage_graph.svg"

Stages S1 to S4 build the store (the bakery on IncQL-DB); stages E1 to E7 build the engine on top of it. The letters keep the two tracks apart, so finishing a store stage never counts as finishing an engine stage.

| Order | Stage | Delivers | Depends on |
| --- | --- | --- | --- |
| 1 | Compiler defects blocking E1 | External native constructors (for example `Mmap.map`) reach lowering as typed constructors, not an unknown namespace value; a byte constant passes to a read-only `bytes` parameter without the "unsupported Body IR non-list borrow" refusal; slicing a Rust value that derefs to `[u8]` types as `bytes`. Reused and already landed or in review: #2491, #2482, #2483, #2493, the keyed standard-manifest reads | — |
| 2 | S1 Durable record store | Full-key collision checks, typed absence, error and tombstone outcomes, frame integrity, validated transactions, durable manifest publication, source and snapshot leases | Fixes |
| 3 | S2 subset | Copy-on-write directory, scoped views and leases, lifetime-safe mapped views, sealed-set admission, deterministic compaction and root-aware collection | S1 |
| 4 | E1a Engine mechanics with unit records | Service, one worker, bounded protocol, one virtual project view, capture with frozen-content skip and metadata-first freshness, one producer that answers a whole compiled unit as one unit record keyed by its RFC 124 identity, native descriptor, claim, publication, execution pin, proof report | S1, S2 subset |
| 5 | E2 producer map | A sub-design naming every producer, its keys, rank, read kinds and witnesses, its group membership where joint, and the prepared-input contract | — |
| 6 | E1b Hello record by record | The producer map's first slice for hello replaces the unit-record producer: per-declaration signature, checked-body, Body IR and plan records; the engine is unchanged | E1a, E2 producer map |
| 7 | S3 Compiler reads record by record | Record-by-record compiler reads, persisted checked body, IR, instance and plan facts with witnessed dependencies, bounded graph consumers | S1, S2 |
| 8 | E2 Ranked semantic graph | All semantic producers, doc records and position maps, demand reach with lookup and listing witnesses, compiler-owned preparation, negative and range witnesses, typed refusals, diagnostic projections, result-view transactions | E1b, S3 |
| 9 | E3 Loaf admission and shared native units | Bundle, Loaf and local-bake admission (local Loaves by copy), workspaces as one root, Rust and C units in a Loaf, Rust inspection and C declaration producers, RFC 097 caller facets, version diffs, exact externs, payload variants, native output views, and the toolchain's own release as an `oven bake` of its Loaf graph: the candidate driver is a unit the runtime and std units depend on, so they are compiled by the compiler they ship with | E2, S2 sealed sets |
| 10 | E4 Scheduler, memory, lifecycle | Antichain scheduling, CPU and memory tokens, fairness, shared-interest cancellation, worker recycling, crash matrices | E3, S2 leases and collection |
| 11 | E5 Test, run and readers | Harness partitioning, fresh per-case execution, explicit replay, LSP overlays, MCP and graph pages, contract runner | E4 |
| 12 | S4 Distribution qualification | Same sets from local bake, bundle and registry; trust refusals, upgrades, platforms and filesystems, Baseline-comparable workloads | E5 |
| 13 | E6 Native incremental proof | Override audit, tracked per-definition inputs, qualified seeds, fixed CGUs, edited and cold oracle comparisons | E2 to E5 |
| 14 | E7 Default switch and removal | Default engine, full census and CI phases, removal of obsolete authority in a separate cleanup, including the release script (`scripts/release-dev8.sh`) | E6, S4, the acceptance report |

E1 is split in two so the engine's own proofs do not wait for the compiler's per-declaration producers. E1a proves the engine's mechanics with one record per compiled unit: the compiler already works in units, and RFC 124 gives each unit an identity from its compiled inputs. E1b swaps that one producer for the per-declaration producers of the producer map's first slice; the engine does not change. The rule that a compiler adapter must not repackage whole-program results as per-declaration records still holds: E1a's records are unit records and are named so. Where the [build engine design](../contributing/explanation/build_engine_design.md)'s E1 exit says otherwise, this record governs.

## Proof by stage

Every stage exits on measured proofs, not on attractive timings. Each table lists the efficiency and correctness proofs the stage must show and the baseline it is compared with. The **Measured**, **Commit** and **Evidence** columns are measurement slots: they hold the result of the stage's qualifying run, the immutable commit it ran on, and the path of its evidence.

Every proof row is measured twice: in a cold home with an empty bakery, and in a warm home that already holds the results. A row passes only when both hold. Warm is the normal state once the engine runs, and reuse defects show only there.

Baselines come from the baseline measurement of 2026-10-29 (#2481, bundle 0cb030e9c, two jobs, fresh home), the regression measurement of the same day, the inventory of the compiler's whole-closure reads, and the dev.8 reference CI run 40926646431 (#2471): **Linux 46 min, macOS 64 min**.

### Compiler defects blocking E1

| Proof | Kind | Baseline | Measured | Commit | Evidence |
| --- | --- | --- | --- | --- | --- |
| `Mmap.map` and other external native constructors lower as constructors | Correctness | Lowering refuses them as an unknown namespace value | | | |
| A byte constant passed to a read-only `bytes` parameter compiles | Correctness | "unsupported Body IR non-list borrow" | | | |
| Slicing a Rust value that derefs to `[u8]` types as `bytes` | Correctness | Does not type as `bytes` | | | |
| Cold hello `lower_program` (reused: #2491) | Efficiency | 80.1 ms median (47.3 ms after #2491) | | | |
| Census verdict parity | Correctness | 1214 PASS / 0 FAIL / 1 PEND, 0 lost PASS keys | | | |
| Compiles through the resident worker from a deep home | Efficiency | 651 compiles on the cold path, about +100 ms each | | | |
| One `incan_frontend` artifact per profile | Efficiency | One artifact per feature variant | | | |
| A second checkout reuses unit identities | Efficiency | Eight duplicate rlibs, 148.28 s of second compilation | | | |
| Standard manifests read for hello | Efficiency | All 12 (12.1 MB), about 45 ms | | | |
| Timing never reaches compared stderr | Correctness | 705 batch splits and 843 s census under timing | | | |

### S1 Durable record store

| Proof | Kind | Baseline | Measured | Commit | Evidence |
| --- | --- | --- | --- | --- | --- |
| Disjoint and conflicting concurrent writers | Correctness | No validated transactions | | | |
| Crash at every staging and publication boundary shows old-complete or new-complete | Correctness | Not covered | | | |
| Injected 128-bit collision resolves by full key | Correctness | Prefix-only lookup | | | |
| Damage reports `StoreError`, never absence or an older value | Correctness | Damaged unit read as a miss | | | |
| Repeated unchanged commit writes nothing | Efficiency | Not measured | | | |

### S2 subset

| Proof | Kind | Baseline | Measured | Commit | Evidence |
| --- | --- | --- | --- | --- | --- |
| Selected record read independent of unrelated segments (store ×10, ×100) | Efficiency | Flat 48-byte index, about 20 probes per segment | | | |
| Mapped views copy zero payload bytes | Efficiency | `MappedFile.read()` returns a copy | | | |
| Two Loaves and a bundle share contents; repeat import is a no-op | Efficiency | Not measured | | | |
| Fork costs root and page deltas, no payload copies | Efficiency | Full store per test home | | | |
| Collection keeps leased snapshots and late lazy reads valid | Correctness | Not covered | | | |
| Byte-identical compacted base across insertion orders | Correctness | Not covered | | | |
| 1, 2, 4, 8 readers share clean pages (PSS or footprint) | Efficiency | 192.1 GB sum of maxima, 1678 MB concurrent upper bound | | | |

### E1a Engine mechanics with unit records

| Proof | Kind | Baseline | Measured | Commit | Evidence |
| --- | --- | --- | --- | --- | --- |
| Same output as legacy for `incan run hello.incn` | Correctness | Legacy output | | | |
| Hello compile to launch | Efficiency | 163 ms warm worker; manifests 35 ms, checking 10 ms, lowering 80 ms, rustc 75 ms | | | |
| Repeat: zero evaluations and native bakes, program still launched | Efficiency | 47 ms unchanged rerun; 16.0 ms edit-loop unchanged median | | | |
| An unchanged request starts no process except the program | Efficiency | Two `rustc` probes and a `--rust-imports` driver on every unchanged run, 62 ms | | | |
| A frozen toolchain is admitted once per installation, not per command | Efficiency | 1,180 file reads and 1,169 JSON decodes on every command | | | |
| `run` after `build` bakes nothing | Efficiency | Second compile, 365 ms | | | |
| Two virtual homes, one claimed bake | Efficiency | One bake per home | | | |
| A superseded, cancelled or refusing caller does not fail the other caller | Correctness | Not covered | | | |
| Collection between publication and selection, lost acknowledgment resolved without rebake | Correctness | Not covered | | | |
| Worker killed before publication recovers safely | Correctness | Not covered | | | |
| Idle worker holds no semantic state | Efficiency | Tiny compile peak 250–267 MB; idle worker about 2 GiB | | | |
| Interface audit: no Rust representation in messages or records | Correctness | Not covered | | | |
| Unchanged rerun: zero source reads and zero hashes | Efficiency | About 1M file reads per census (24.3 s cold) plus hashing (22.4 s cold) | | | |
| Frozen inputs untouched: zero stats, reads and hashes on toolchain, std and Loaf files | Efficiency | Re-proved by every command | | | |

### E1b Hello record by record

| Proof | Kind | Baseline | Measured | Commit | Evidence |
| --- | --- | --- | --- | --- | --- |
| Records and bytes read against records needed | Efficiency | 362 bindings, about 400 bodies, 3.15 MB Body IR for 11 emitted functions | | | |
| One-body change touches only its records | Efficiency | Whole-unit recompile | | | |
| Per-declaration plans byte-identical to the whole-program plan | Correctness | Not covered | | | |
| The engine is unchanged from E1a | Correctness | Not covered | | | |

### E2 producer map and E2 Ranked semantic graph

| Proof | Kind | Baseline | Measured | Commit | Evidence |
| --- | --- | --- | --- | --- | --- |
| Producer map covers every producer, key, rank and read kind | Correctness | String refusal and lower-again retry | | | |
| Cold-oracle equivalence for body, signature, layout, default, namespace insert and delete, absent import, candidate set, feature, toolchain and Loaf edits | Correctness | Not covered | | | |
| Checking scope equals legacy, unreachable bodies included | Correctness | Legacy scope | | | |
| One-record edit reuses sibling checks, Body IR and plans | Efficiency | One lowering-body edit 270.79 s; edited unit 252.02 s | | | |
| No evaluation starts before its inputs are final; no restart | Correctness | Retry loop bounded by distinct facades | | | |
| Exact producer counts per edit | Efficiency | Not measured | | | |
| Hello edit loop | Efficiency | 316.9 ms median | | | |
| `run` of one fixture among 1,500 unrelated ones touches only its reach | Efficiency | Not measured | | | |
| A new file at a recorded absent lookup path is picked up; a new unrelated sibling is not touched | Correctness | Not covered | | | |
| Comment-only edit: zero checks, Body IR and plans; only codegen units whose lines moved are re-emitted | Efficiency | Not measured | | | |
| Diagnostics and stack traces show current lines after code moves | Correctness | Not covered | | | |
| Docs generation compiles nothing | Efficiency | Not measured | | | |

### S3 Compiler reads record by record

| Proof | Kind | Baseline | Measured | Commit | Evidence |
| --- | --- | --- | --- | --- | --- |
| Std manifest and fact reads replaced by records | Efficiency | About 35 ms; 7–16 MB | | | |
| Lowering reads only demanded runtime facades | Efficiency | `project_context` joins the full std closure | | | |
| Unchanged work adds no facts | Efficiency | Not measured | | | |
| Bounded codegraph answer for one unit | Efficiency | 64,211,355 bytes, 60,612 records | | | |
| `whole` and `keyed` plans byte-identical on the census | Correctness | Not covered | | | |

### E3 Loaf admission and shared native units

| Proof | Kind | Baseline | Measured | Commit | Evidence |
| --- | --- | --- | --- | --- | --- |
| Shared contents imported once | Efficiency | Per-consumer extraction | | | |
| Two homes bake identical units once | Efficiency | Compiler Loaves compiled 2–7× in a cold run (30.5 min) | | | |
| Different features or native requirements stay distinct | Correctness | Not covered | | | |
| Frozen definitions never recomputed | Efficiency | Not measured | | | |
| Two valid unequal payloads: both kept, unit machine-local, receipts pinned, unit-only reuse refused | Correctness | Not covered | | | |
| Damaged or incomplete capabilities fail closed | Correctness | Not covered | | | |
| Edits to a local Loaf reach consumers only after re-admission; workspace members need none | Correctness | Not covered | | | |
| Rust body edit with equal interop facts re-checks no Incan caller | Efficiency | Not measured | | | |
| Borrow-shape change re-checks every Incan caller and re-decides borrowing | Correctness | Not covered | | | |
| Two-way Incan and Rust interop in one Loaf (RFC 097) builds with no cycle | Correctness | Not covered | | | |
| Undeclared untracked read inside a procedural macro is outside the reach and reported | Correctness | Not covered | | | |

### E4 Scheduler, memory and lifecycle

| Proof | Kind | Baseline | Measured | Commit | Evidence |
| --- | --- | --- | --- | --- | --- |
| No duplicate successful producer under contention | Correctness | Not covered | | | |
| No deadlock across homes | Correctness | Not covered | | | |
| Idle workers hold no semantic heap or mappings | Efficiency | Idle worker about 2 GiB | | | |
| Reservations against actual peaks, CPU-token utilization, wait reasons | Efficiency | 1624 driver processes; p50 / p90 / p99 45 / 300 / 672 MB | | | |
| Running children keep their pins through collection | Correctness | Not covered | | | |
| Crash matrices at every boundary | Correctness | Not covered | | | |

### E5 Test, run and readers

| Proof | Kind | Baseline | Measured | Commit | Evidence |
| --- | --- | --- | --- | --- | --- |
| Full frozen census, zero lost PASS | Correctness | 1214 PASS / 0 FAIL / 1 PEND | | | |
| Console bytes unchanged with tracing on | Correctness | Timing JSON broke stderr comparisons | | | |
| Fresh binaries per eligible batch, case isolation kept | Efficiency | 245 isolated launches, 145–203 ms first exec each | | | |
| No batch deadline kills | Efficiency | About 17 × 5 s ≈ 85 slot-s discarded | | | |
| Graph family warm | Efficiency | 123.2 s (`graph/incan/vocabulary` 76.0 s) | | | |
| Runner and MCP memory | Efficiency | Runner 10.4 GiB; `incan mcp` 3.8 GiB | | | |
| No implicit test replay | Correctness | Not covered | | | |

### S4 Distribution qualification

| Proof | Kind | Baseline | Measured | Commit | Evidence |
| --- | --- | --- | --- | --- | --- |
| Same admitted sets from local bake, bundle and registry | Efficiency | Not measured | | | |
| Trust and profile refusals, upgrades and removal | Correctness | Not covered | | | |
| Durability per platform and filesystem, including power-loss order | Correctness | Process-kill tests only | | | |
| Baseline-comparable census | Efficiency | 322 s (two jobs, fresh home); 284.38 s fresh home at twelve jobs | | | |
| First compile in a fresh home | Efficiency | 13.5 s | | | |

### E6 Native incremental proof

| Proof | Kind | Baseline | Measured | Commit | Evidence |
| --- | --- | --- | --- | --- | --- |
| Every override has a tracked input | Correctness | Untracked global `Plan` | | | |
| Edited incremental output equals cold output and behavior | Correctness | Not covered | | | |
| CGUs actually regenerated per edit | Efficiency | Whole native unit | | | |
| Head-to-head rows: hello cold, unchanged, private-body edit, signature edit, two homes, unrelated packages ×1/×2/×4, demanded records ×1/×2/×4, graph and LSP, census, idle workers | Efficiency | Pinned integration tree under BE-28 | | | |
| Observation overhead | Efficiency | At most 5% | | | |

### E7 Default switch and removal

| Proof | Kind | Baseline | Measured | Commit | Evidence |
| --- | --- | --- | --- | --- | --- |
| Full census with the default selector, zero lost PASS | Correctness | Frozen integration PASS set | | | |
| CI wall time, Linux | Efficiency | 46 min (run 40926646431) | | | |
| CI wall time, macOS | Efficiency | 64 min (run 40926646431) | | | |
| Every deleted authority names its replacement | Correctness | Owner caches, stamp ledgers, whole-closure transport | | | |
| No fallback-specific code remains | Correctness | Legacy route present | | | |

## Code map

One home per component. **Reused** means existing code that the component builds on, with its branch and commit; **to be built** means no implementation exists yet. The last column flags every place where the design or the reused code has two homes for one thing; each flag is resolved by the stage named.

### Platform

The platform is one Loaf, `incan_platform` in `loaves/platform`, below `std`. Oven, the store, the compiler worker host and `std` depend on it. A primitive stays at the home the State column names until it moves into that Loaf.

| Component | One home | State | API contract | Two-home flags |
| --- | --- | --- | --- | --- |
| Store substrate (IncQL-DB profile) | Platform store module; first implementation `workspaces/oven/src/segment_store.incn` | Reused: `dev9/bakery` 65c314587 (`ab0fbc970` typed outcomes, validated transactions, synced publication) | `snapshot`, `lookup → Found / Deleted / Absent / StoreError`, `begin`, `put`, `delete`, `commit` | Lives in Oven while its home is the platform; moves under S2. Read and lookup paths `get` (`Option[bytes]`) and `lookup` coexist; `get` goes |
| Bakery families and claims | `workspaces/oven/src/bakery.incn`, `unit_publication.incn` | Reused: `dev9/bakery` 65c314587 | Unit records, claim-once, views | Claim locks belong to the substrate, the claim protocol to the engine; today both live here (resolved in E1) |
| Sealed record sets | `workspaces/oven/src/sealed_set.incn` | Reused: two versions, `dev9/s2-sealed-sets` f00035a66 and `dev9/store-s2-prep` 0b1899ede | Seal, verify, admit once | **Two implementations of one mechanism**; one is kept in S2 |
| Memory mapping | Platform mapping primitive with lifetime-safe views: the shape of `loaves/stdlib/system/src/fs/mapping.incn` and `loaves/stdlib/system/rust/src/mapping.rs` at 296b28e4b (`dev9/store-s2-prep`) | Reused | Map read-only, borrow bounded views, scalar reads, owner-anchored lifetime | `std.fs` mapping becomes a thin wrapper over the platform primitive; the copying `MappedFile.read()` of c9d9d70fa / 65c314587 is a second mapping API and goes |
| Durable I/O and publication ordering | Platform durable-I/O module | To be built (publication sync inside `segment_store.incn` today) | Sync file, sync directory, atomic replace, persistent locks | Sync logic embedded in the store; extracted to the platform in S1 |
| IPC and peer identity | Platform IPC module | Reused in part: endpoint and pooling in `workspaces/oven/src/owner_endpoint.incn`, `compiler_worker.incn` (#2483) | Bounded framed transport, peer credentials, credits | Two protocols today (owner and compiler worker); `incan.engine/1` and `incan.compiler-worker/1` stay distinct by design, framing has one home |
| Process control and groups | Platform process module | To be built | Spawn, process group, birth evidence, cancel with grace | — |
| Resource probes | Platform probes module | To be built | CPU limit, effective memory, footprint, observer coverage | — |
| Tracing | Platform tracing module | Reused in part: `workspaces/oven/src/jec.incn`, work spans (`6a70e9f94`) | Spans, counters, separate channel | **Four channels today**: `jec`, `OVEN_TRACE`, `INCAN_TRACE_DIR`, `INCAN_NATIVE_TIMING`; one home in E1, the rest become adapters |

### Compiler record service

| Component | One home | State | API contract | Two-home flags |
| --- | --- | --- | --- | --- |
| Unit identity | `workspaces/oven/src/invocation_identity.incn`, `source_identity.incn`, `compiler_facts.incn` | Reused: [#2493](https://github.com/encero-systems/incan/pull/2493), `dev9/b2a-identity-final` 18ce40d88 | Identity from compiled inputs, no paths or mtimes | `unit_freshness.incn` stamps remain a second freshness authority until evaluation records replace them (E2) |
| Record read API | `loaves/compiler/incan_mir_lowering/src/records.incn` | Reused: `dev9/b4-read-path` 411329faa | `RecordKey`, `RecordRead = Found / Absent / Failed`, `ReadWitness` | `loaves/compiler/incan_frontend/src/record_read.rs` repeats the type in Rust as a bootstrap adapter; its `Failed(str)` and string-refusal retry become typed outcomes (E2) |
| Keyed manifest reads | `loaves/compiler/incan_frontend/src/loaf_index.rs` | Reused: `dev9/b4-read-path` a436df28b | Read a standard manifest only when reached | Process-global `ARTIFACTS` / `ABI` stores in `loaf_index/retained.rs` duplicate the store; removed in S3 |
| Lowering context | `loaves/compiler/incan_mir_lowering/src/project.incn`, `lowering.incn` | Reused: [#2491](https://github.com/encero-systems/incan/pull/2491) | Borrowed project context | Joins every std module per compile; per-record plan functions replace it (E2) |
| Producers (`prepare`, `evaluate`) | Compiler record-service module | To be built | The three contracts above | — |
| Rust inspection | The Rust facts extractor that already serves duckborrowing | Reused: present extractor | Declaration, signature and ownership records | Must become one producer over the shared IR, not a second facts store; named in the E2 producer map |
| Doc and position records | Compiler record-service module | To be built | Doc record, position map | — |
| `PlanReader` and native tracking | Pinned rustc adapter, `workspaces/toolchain/incan-rustc-driver/src/adapter.rs` | Reused in part | Record-backed query answers with tracked fingerprints | Whole `Plan` in process-global state; replaced in E6 |

### Engine

| Component | One home | State | API contract | Two-home flags |
| --- | --- | --- | --- | --- |
| Service and lifecycle | Oven engine service | To be built | Store lock, handshake, drain and stop | Per-project owners (`owner_client`, `owner_service`, `owner_state`) retire at E7 |
| Request capture and freshness | Engine capture | Reused in part: dev.7 retained-owner source reconciliation (#2192, #2194–#2196, #2202, #2203) | `Submit → Accepted → Captured`; reach, metadata-first checks, observer | `unit_freshness.incn` per-call freshness reads and stamps (see Unit identity) retire at E2 |
| Ranked work graph | Engine graph | To be built | Frontier, dirty set, witness validation | — |
| Scheduler | Engine scheduler | To be built | Tokens, fairness, batches | Runner-side group deadlines in `workspaces/contracts/src/batch.incn` retire in E5 |
| Claims and publication | Engine claims over substrate locks | Reused in part: `bakery.incn` claims | Claim, recheck, publish shared, select per caller | See bakery flag |
| Leases and pins | Engine over platform leases | To be built | `Pin`, `Release`, execution pins | — |
| Run and test | Engine execution actions | Reused in part: `workspaces/oven/src/testing.incn` | Execution ticket, harness partitions, fresh cases | Runner and Oven both decide batching today; one home in E5 |
| Readers | Engine query surface | Reused in part: `workspaces/oven/src/graph_context_index.incn`, `workspaces/contracts/src/graph.incn` (#2484) | `Query` / `Page`, typed cursors | In-memory node and link structures duplicate the store; replaced in S3 |
| Profiling and proofs | Engine over platform tracing | To be built | `oven inspect run <id> --proofs`, `--engine-report` | See tracing flag |
| Engine selector | Engine entry in `workspaces/oven/src/main.incn` | To be built | `INCAN_ENGINE=legacy\|records` | Temporary by design; removed at E7 |
| Toolchain release | `oven bake` of the toolchain's Loaf graph | To be built (today `scripts/release-dev8.sh`) | Bake the driver, runtime and std as units in dependency order; pack the admitted set | The script orders compilation by hand and compiles std with the previous toolchain's driver; it retires at E7 |

## Language and platform boundary

Each prerequisite is verified on the selected toolchain at runtime, not by a type check alone: mapped byte views, durable publication and locking, scoped views and leases, typed IPC and process control, resource and observer probes, the compiler record service, native input tracking, and deterministic schemas and hashes. A missing capability blocks its stage and is tracked as a language or platform gap.

**Bootstrap seeds.** A release compiles the standard library and Oven with the previous toolchain, the seed. Engine and platform code therefore cannot use a compiler fix or a standard API that the same release introduces. Each qualified release, installed fresh with `incan toolchain install`, becomes the next seed; a copied installed home is not a seed, because it keeps paths into the run that built it. A stage that adds a standard or platform API schedules that seed step before the code that uses the API. Once the toolchain release is an `oven bake` (E3), the candidate compiler builds the runtime and standard library it ships with, but source still bootstraps from the seed.

The only authored Rust is bootstrap work, each piece with a named boundary and the contract that removes it:

1. the existing language-core adapter that exposes checker and native compiler objects as record-scoped handles;
2. the pinned rustc query, AST and MIR adapter that connects record handles to the native route with tracked inputs;
3. the minimum executable entry that calls the Incan-authored record-worker host.

A bespoke Rust service, a side database, a generated Cargo project or a Python layer is outside this decision.

## Consequences

- A command is a request for roots; the engine computes each missing fact once per physical store and every later request finds it.
- Memory follows the work running now: no process holds a dependency closure between requests.
- The store grows only by new facts; unchanged builds write nothing.
- Readers answer bounded questions with bounded work.
- The checker, Body IR and lowering become per-record producers; that restructuring is the main cost, as DD-0012 stated.
- The current owner, ledger, stamp and whole-closure paths are removed once E7's acceptance holds.

## Non-goals

- A distributed cache, cross-user authorization or a networked store.
- A relational planner, Substrait, Protobuf or DataFusion inside the engine.
- New user-facing syntax or semantics.
- Bypassing platform security assessment of new executables.

## Revisit condition

Revisit if a producer needs whole-program state that per-record reads cannot express, if a proof row stays `failed` after its stage's work is complete, or if the pinned rustc cannot supply tracked native inputs.

## Provenance

The design (engine design v2 with decision register BE-01 to BE-33 and stages E1 to E7) and the storage design it builds on (stages S1 to S4) were approved by the maintainer on 2026-10-30. This record summarizes them. The full texts are versioned beside it as the [build engine design](../contributing/explanation/build_engine_design.md) and the [bakery storage design](../contributing/explanation/bakery_storage_design.md), the detailed contract for each decision. Where this record adds to or differs from them, this record governs: capture and freshness, Rust and C interop, comments, docs and source positions, and damaged records. It refines DD-0012 in one respect, one engine per physical bakery with homes as virtual stores over it, and supersedes one DD-0012 rule, that a damaged record is a miss (see [Damaged records](#damaged-records)), and one DD-0004 rule, that only the driver Loaf's units may use rustc's internal interface (see [Rust and C interop](#rust-and-c-interop)). The maintainer added BE-34 (comments, docs and source positions) during review of this record.

## References

- DD-0004, DD-0007, DD-0008, DD-0012
- RFC 097, RFC 106, RFC 117, RFC 118, RFC 119, RFC 120, RFC 121, RFC 124, RFC 125; IncQL RFC 060 and RFC 067
- #2481, #2484, #2471, #2482, #2483, #2491, #2493
