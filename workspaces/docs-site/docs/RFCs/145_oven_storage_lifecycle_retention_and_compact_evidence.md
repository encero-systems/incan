# RFC 145: Oven storage lifecycle, retention and compact evidence

- **Status:** Draft
- **Created:** 2026-10-15
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 020 (offline, locked and reproducible builds)
    - RFC 061 (stdlib compression)
    - RFC 090 (standard CLI exit categories)
    - RFC 112 (crash-safe publication and file coordination)
    - RFC 117 (Loaf project and local-store model)
    - RFC 118 (Incan and Oven command-line surfaces)
    - RFC 119 (native build units and Rust facets)
    - RFC 124 (unit identity and cross-plan artifact sharing)
    - RFC 125 (Loaf registry and baked asset distribution)
    - RFC 126 (durable investigations and evidence availability)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Oven should keep large projects predictable in storage as well as in build time. This RFC defines one user-wide artifact lifecycle across ordinary build, run, test and package workflows: compatible immutable units share storage, verified downloads remain available until deliberately pruned, disposable development state expires under a coordinated capacity policy, and retained evidence stays compact through references, deduplication and lossless compression. Cold content can remain compressed without changing its identity, and restoration is a bounded, verified operation. Users can inspect what occupies space, why it remains, and what an explicit cleanup would remove without managing compiler-specific directories.

## Core model

The governing policy is: **store as little as possible, recover cheaply.** Just Enough Compilation (JEC) bounds the work that produces development state: select the required Loaves, reuse compatible baked Loaves, perform only the compilation and linking that the requested result requires, and materialize only the content it needs. Keep the minimum content needed for the promised availability and recover disposable state through that same selective path.

1. **Share by the existing unit contract.** RFC 124 determines interchangeability. Projects, worktrees, SDK preparation, host tools and tests consume compatible units directly from the shared store. Downloaded Loafs are not copied from that store into project or secondary caches; a consumer-local reference may be a symlink.
2. **Retention is a relationship.** A download, live project receipt, execution lease, investigation or explicit pin can retain a closure. The bytes have one identity; several independent reasons may keep them available.
3. **History does not imply permanent executable retention.** A historical receipt records what happened. A live retention registration separately records which payloads must remain usable. Expiring the latter does not rewrite the former.
4. **Downloads survive automatic maintenance.** Automatic expiry and pressure handling may reclaim disposable state and hot expansions, but must preserve the verified backing content and closure of retained downloads.
5. **Coordinate the whole operation.** Retained content, in-progress writes, compression, restoration and materialization contribute to observable accounting. Concurrent commands cannot independently spend the same available disk space.
6. **Representation does not change meaning.** Compression changes how bytes are stored, not the unit identity, canonical payload digest, archive identity or attestation.
7. **Evidence is a small account of work.** Results and recipes reference exact inputs and selected artifacts. Keeping every executable, expanded dependency and build directory is an explicit retention choice, not the default consequence of recording a result.

## Motivation

This RFC addresses a current delivery gap in Oven: the intended JEC reuse and bounded-storage experience is not yet delivered at the scale of Incan compiler and toolchain development. Compatible prepared closures can be retained in separate stores, repeated work can produce additional generations, and historical build state can outlive its useful role. The solution must make ordinary Oven workflows do less compilation, reuse baked Loaves directly and retain less derived state. A one-time cleanup or additional storage capacity does not establish that behavior.

A large project may contain application libraries, host generators, native dependencies, tests and several target configurations. Rebuilding after small edits, switching profiles and running concurrent commands can create many generations of individually valid artifacts. A single large workspace can encounter this pressure; it does not require dozens of compiler checkouts.

Incan compiler and toolchain development is a substantial real-world case study of this workload. It combines repeated compiler edits, SDK and native-driver preparation, dependency reuse, concurrent test batches and retained failure evidence. It should expose where compatible content is duplicated, where genuine compiler changes require new units, and where historical executable generations outlive their useful development role. Those findings must inform ordinary consumer behavior through the same Oven services.

Unit sharing addresses repeated compatible compilation, but it does not by itself decide when a superseded executable, failed staging directory or old capture should disappear. Independent store limits can also multiply: a bounded cache in every test environment is an unbounded collection of caches from the user's perspective. A cleanup that removes useful downloaded dependencies solves the space symptom by creating avoidable network and compilation work later.

Retained evidence needs a separate answer. A command, result, selected identities, source patch and diagnostic may explain an experiment without retaining its complete compiler and dependency closure. Repeated logs may compress well, while some compiled content offers much smaller savings. Retention, sharing and compression must therefore work together; no compression ratio substitutes for deciding whether bytes deserve to remain.

The desired experience is ordinary development without preparation or cleanup rituals, with deliberate control over persistent downloads and explicit outputs. The same behavior must serve small projects and users working at toolchain or operating-system-sized graph scale.

## Goals

- Make compatible immutable storage reusable across ordinary Oven workflows and independent projects.
- Define persistent downloads, current project roots, leases, pins, disposable generations and compact historical evidence.
- Coordinate automatic expiry, byte accounting, temporary-space reservations and pressure diagnostics.
- Preserve offline operation for retained downloads and protect actively used or explicitly pinned content.
- Make deduplication and lossless compression part of retention, with bounded restoration and selective evidence inspection.
- Expose inspectable, previewable cleanup that names affected roots and availability consequences.
- Give humans and AI agents the same structured pressure facts, available actions, and consequences so they can make deliberate storage decisions.
- Define consumer acceptance for large graphs, repeated edits, concurrency, failure recovery and several filesystems.

## Non-Goals

- Changing RFC 124's unit compatibility rules, weakening trust admission, or equating packages solely by name and version.
- Adding a new dependency resolver, registry, remote build-cache protocol or compiler ABI equivalence model.
- Automatically managing authored source, user models, system backups, requested output images, or explicitly exported packages and reports.
- Guaranteeing arbitrary compression ratios, a universal artifact-size ceiling, or identical physical allocation reporting on every filesystem.
- Changing language semantics, Rust code generation, default optimization/debugging contracts or supported compilation targets.
- Requiring a resident background service or periodic job installed by the user to make ordinary cleanup correct.

## Guide-level explanation

### Build the next project without another copy of its foundations

A developer builds an application, a command-line utility and tests that share a compatible dependency graph. Oven keeps their project outputs separate and selects common native dependencies, SDK units and toolchain owners from the same store. A second checkout changes source selection; it does not automatically create another immutable dependency cache.

Changing target, features or profile may require different units. Oven reports the difference instead of describing every miss as a cold cache. The following commands illustrate the proposed canonical Oven experience; they are acceptance examples, not claims that the complete surface is already shipped:

```text
$ oven build
Dependencies: 38 reused, 2 compiled
Project outputs: 1 linked

$ oven build --release
Dependencies: 24 reused, 16 compiled
Variant changes: profile differs for 16 units
```

The counts are illustrative. Reuse and storage facts come from the actual selected graph and execution, not from the wording of a progress message.

### Keep downloaded work available

A developer downloads verified assets, works offline for several days, and repeatedly edits a local application. Automatic maintenance retires superseded local results while leaving the downloaded assets and their required closure available. Some backing content may be cold and compressed; the next use restores only the required units without downloading them again.

Running out of room is visible before an expensive write fails. If protected content prevents a new operation from fitting, Oven explains the shortfall and offers inspection, deliberate pruning or a larger configured allowance. It does not silently delete downloads and hope they can be obtained again later. A warm operation that needs no new allocation remains usable.

### See what would be removed

RFC 118 owns the command families. This RFC proposes the following lifecycle operations within those families:

```text
oven store status
oven inspect cache
oven store gc --dry-run
oven store gc
oven store prune --downloads --asset sha256:... --dry-run
oven store prune --downloads --asset sha256:...
oven store prune --project <project-id> --dry-run
oven store pin sha256:... --reason "release reproduction"
oven store unpin <pin-id>
```

`gc` performs policy-governed collection of eligible derived state. Explicit download pruning names assets and reports which persistent retention roots it would remove. Removing one root does not remove shared content retained by another root. Pins remain separate: pruning a download does not silently unpin a reproduction that needs the same bytes.

Status distinguishes retained downloads, live project content, pinned content, idle derived content, compressed backing, temporary reservations and evidence history. These relationships may overlap; the unique total does not add the same shared payload once for every owner. A dry run reports an estimate and its uncertainty without making changes.

### Keep a failure report without keeping its build directory

After a failed test, Oven retains a summary, exact selected identities, the reproduction command, relevant source changes and compressed diagnostics. Opening the summary does not unpack a dependency closure. A user can inspect an individual diagnostic chunk, export a report, or explicitly pin the executable closure when a future binary-level reproduction is needed.

If payloads later expire, the report still says which ones were used and whether they remain available. An expired executable is not represented as retained, and an incomplete capture is not represented as complete. Durable investigations use their own retention roots rather than depending on the incidental age of a test-output directory.

## Reference-level explanation

### Shared store and selection

Ordinary Oven operations for one user must resolve a shared immutable store independently of the source checkout, project scratch location and test working directory. Build, run, test, SDK/provider preparation and package consumption must use the same selection and admission rules. An explicitly isolated store is permitted, but its isolation and additional storage must be observable; it must not become an undocumented per-command default.

The effective store root must be discoverable through inspection and consistently selectable across command families. A mutable project home must not implicitly require an independent copy of all reusable artifacts. The pub download cache is the shared Loaf store described by RFC 125, not an upstream cache from which another Oven cache receives copies. Admission registers and verifies downloaded units in that shared owner. Read-only packaged owners and verified local mirrors may satisfy selections through the same identity space; their placement must not create another resolver or weaken trust.

Normal consumption of a downloaded Loaf must reference its admitted shared location directly. If a consumer-local filesystem entry is needed, it may be a symlink to that location; normal consumption must not copy, reflink or hard-link the Loaf into a project, fixture home, checkout or secondary cache. A different filesystem or unavailable symlink support must not trigger a copy fallback: direct shared paths remain the consumption mechanism. The same compatible downloaded unit must not acquire another persistent payload because another project or command selects it.

Selection must bind the reference to the verified unit/payload identity and hold a lease on its canonical owner through use. A symlink is a placement reference, not authority or ownership. The physical operation must use the admitted canonical location so that retargeting a consumer-local symlink cannot substitute unverified content. Collection must honor the live canonical lease, and removal of a reference must not imply deletion of its shared target.

Units may be shared only under RFC 124's identity and payload rules. Storage representation, receipt history and retention origin must not make incompatible units interchangeable. Payload conflicts under one unit identity must preserve the existing conflict and provenance requirements.

The store must retain one canonical backing payload for an interchangeable unit, subject to RFC 124's conflict handling. Filesystem aliases, temporary staging, user-requested output publication and explicitly bounded hot materializations may create additional placements. Each placement must have an owner and lifecycle and must contribute honestly to accounting. A new compatible project must not require copying its entire reusable closure into a new private store. A user-writable output must not expose a writable alias to admitted immutable backing; materialization must preserve the backing content even when the user modifies or replaces the output.

### Retention roots and availability

A retention root must have an identity, kind, owner, creation time, reachable artifact set or closure reference, and explicit expiry or persistence. The required kinds are downloaded asset, current project receipt, active operation, explicit pin, and retained investigation/capture. A historical result is not itself an additional persistent executable-retention root.

Availability must distinguish content available directly, content recoverable from retained local compressed backing, content retained externally but requiring acquisition, and content unavailable. A historical identity or a source recipe alone must not be reported as an available executable. Offline availability means the complete content required by the selected operation is retained locally and can be materialized without network access.

Successful admission of a downloaded asset must atomically establish a persistent root for the admitted asset, its trust/provenance records and every local content dependency needed to use it under its recorded facts. If some members already exist from a local bake, those members gain the same protection. Automatic maintenance must not delete the asset or any member whose deletion would break that retained closure.

A source Loaf and a baked asset retain different capabilities. Keeping source does not establish that a prepared executable variant is available. Inspection must identify which variants and acquisition requirements remain. The retention contract does not promise that previously unselected profiles, targets or features are already present.

The current project receipt registration retains selected content needed by the current published result and its declared active development use. This protection must renew with successful use and expire automatically after the configured development inactivity period, initially the same seven-day idle TTL as reusable derived state. Build, run or test use of the registered result renews it; inspecting history, listing the store or merely leaving a project directory present must not. An active operation lease independently protects content through the operation.

Replacing, expiring or explicitly retiring that registration may make old local units eligible; it must not mutate the immutable historical receipt. Registration expiry must not restart an artifact's idle TTL or grant it another full retention interval. A downloaded root, explicit pin, retained investigation or another live project root continues to protect shared content independently. Deleting a project directory must not be necessary to retire abandoned development protection. Ownership reconciliation must distinguish an absent owner, an idle owner and a live process before making collection decisions.

Returning to an expired project may require rebuilding its local units. Recovery must select the compatible units still available and perform only the work required by the requested operation; expiry must not itself trigger a complete dependency download, SDK preparation or unrelated workspace rebuild. Requested outputs, source and lockfiles remain user-owned. If necessary inputs are unavailable under the active offline or locked policy, Oven must report that limitation rather than inventing a recoverable executable.

Explicit pins must identify the selected unit/asset/execution and the closure they protect. A pin must have a stable id, owner and reason and may have an explicitly requested expiry. Unpinning removes that retention relationship only. Pins must not be silently removed by download pruning, budget pressure or ordinary diagnostic-history expiry.

An active operation lease must protect the actual selected representation and closure through compilation, linking, execution or restoration. Process ids alone are insufficient proof of current ownership because ids may be reused and child processes may outlive a launcher. Owner recovery must use the existing cooperative lifetime and crash-recovery contract.

### Automatic maintenance and TTL

Automatic maintenance must collect only content no live root or lease requires and which the declared derived-state policy makes eligible. A payload reachable from several roots is retained until every protecting relationship permits its removal. Locally built versus downloaded origin is provenance, not a mutually exclusive ownership classification.

The proposed default idle TTL for reusable local derived units is seven days from their last successful selection; eligible idle content may be reclaimed sooner under the declared byte-retention policy. Superseded private attempts have a maximum retention of 24 hours after a confirmed end of ownership. Successful replacement should retire unnecessary intermediate state promptly. Last filesystem modification time must not be treated as last use, and elapsed wall time alone must not prove owner death.

Time comparisons must handle restarts and clock changes without prematurely expiring retained content. A backwards clock change must not manufacture age. The effective TTL, last successful use, expiry basis and retention reason must be inspectable.

Normal completion and cancellation must release their temporary reservations and private state when their owned work has ended. Recovery after a crash must leave earlier committed artifacts usable. Unknown ownership must be reported and must not authorize deletion. A stale directory name or absent top-level process alone is not sufficient evidence.

Maintenance must not require an independently installed scheduler for correctness. It may run at ordinary lifecycle boundaries or through an explicitly requested command. Warm selection must not enumerate, rehash or refresh the timestamps of an unrelated transitive store simply to enforce retention; selected-identity indexes and bounded maintenance must preserve ordinary-command latency.

### Capacity and pressure

The policy must distinguish an idle derived-retention target from an allocation admission bound. The retention target guides which eligible idle data is kept. Admission governs whether new staging, output, compression and restoration can safely proceed. Active or persistent data may exceed the idle-retention target; such a condition must be visible rather than resolved by deleting protected content.

A change of project, checkout, command or fixture directory must not create a new implicit budget allowance. Managed isolated stores and scratch must participate in the user-wide accounting and temporary-admission scope by default. An explicitly independent policy scope is permitted only as a discoverable user/host choice, with its additional retained allocation visible.

All operations in a shared store must coordinate reservations. Each reservation must record the owning operation, intended work, reserved bytes, observed allocation and lifecycle state. Admission must account for existing concurrent reservations and a configured free-space headroom. The implementation may use estimates before work starts but must monitor actual allocation and handle overruns through bounded additional admission, backpressure or cancellation-safe refusal.

Before refusing a new allocation, Oven should reclaim eligible derived state according to policy. It must not evict active leases, remove persistent download roots, unpin content or delete user-owned outputs. If protected content prevents admission, the diagnostic must report the operation's required space, observed available space, relevant reservations and protected totals, and the action that can change the outcome. It must not wait indefinitely without progress or a cancellation path.

Storage-pressure diagnostics must support deliberate decisions by humans and AI agents. They must expose versioned structured facts alongside the readable explanation: the requested operation, effective policy and accounting scope, required space or its estimated range, observed available allocation, configured free-space headroom, concurrent reservations, protected content and its retaining roots, eligible reclaimable content, and the resulting shortfall. Shared allocation must not be counted repeatedly. Estimates, unknowns, and the observation time must be explicit. A diagnostic must not require an agent to parse terminal prose or inspect compiler-internal directories to recover those facts.

Where an action can change the outcome, the diagnostic must identify the supported action and exact selectors or configuration controls, its applicable preconditions, and the known or estimated consequences. Choices may include collecting eligible derived state, deliberately pruning selected downloaded or project roots, changing an allowance, or reducing or waiting for concurrent allocation. The action description must distinguish changing retention from immediately reclaiming bytes, identify shared or pinned content that remains protected, and expose any loss of offline availability or need for acquisition, restoration, or rebuilding. Only actions supported by current policy and observations may be advertised. A preview is evidence for a decision, not a guarantee that eligibility will remain unchanged; execution must recheck the live roots, leases, reservations, and available space.

When proceeding requires a new caller decision, interactive human-facing execution may explain the concrete proposed action and its consequences and ask whether to proceed. A confirmation must identify the selected action, affected roots or allowance, and relevant availability consequences; a generic confirmation must not conceal those effects. Machine-readable or noninteractive execution must not wait for terminal input. It must exit with a nonzero status and a versioned structured decision-required outcome containing the pressure facts, supported actions, preconditions, and consequences. An AI agent can then choose a precise follow-up invocation or decline. Invocation mode must be explicitly selectable; having a terminal alone must not override a request for noninteractive behavior.

A decision-required outcome must remain distinguishable from successful completion and from an unrecoverable operation failure. Work already completed or admitted must be reported, and returning the outcome must release reclaimable temporary state after owned work ends. A later invocation must recheck current admission and retention facts rather than treat the earlier confirmation or report as a guarantee of unchanged state. Policy-authorized automatic maintenance remains automatic; the interaction boundary applies when an additional choice is required. The RFC 118 `--format json` mode must be noninteractive and must return the structured outcome without prompting, even when a terminal is present. A decision-required outcome must exit with code 3; RFC 090's existing success, application-failure, and usage-error categories remain 0, 1, and 2 respectively. The status reports the outcome of the operation rather than replacing the structured facts and actions. RFC 118 records these shared CLI semantics.

A selection that requires no new allocation must not fail solely because idle-retention accounting is over target. A cold restore or a new output may require space and must obey admission. No policy can guarantee operation when the underlying filesystem cannot satisfy the requested write; that failure must preserve previously admitted content.

### Physical accounting

Inspection must distinguish logical payload bytes, encoded backing bytes, uniquely allocated retained bytes, hot materialization bytes, evidence bytes, active temporary bytes and reserved but not yet allocated bytes. Counts must state their scope and overlap: active and persistent root totals are not disjoint merely because their labels differ.

Where supported, accounting must avoid charging a physical allocation repeatedly for hard links or copy-on-write aliases. Cross-filesystem copies must be charged as additional allocation. If exact extent sharing or actual reclaimability cannot be measured, inspection must expose an estimate or lower bound and its limitation. The implementation must not promise that deleting a snapshot-retained extent immediately increases free space.

Status must expose the effective policy and explain why content is retained, collectable, or blocked. A completed collection must report removed roots, removed content and observed or estimated reclaimed allocation. Source and requested-output storage outside managed ownership must be reported separately when measured; it must not be silently swept into cache deletion.

### Compact evidence

Default operation evidence must retain the result, producing operation, selected source/lock/compiler/provider/unit identities, command and relevant configuration, work performed, meaningful diagnostics, timings and capture completeness. A source patch or fixture required to understand the result must be retained or have its availability explicitly recorded. Identity-only references must not imply that source or an executable remains retrievable.

Default evidence must not retain complete dependency closures, compiler snapshots, expanded source checkouts or target trees. Such payload retention requires an explicit root and visible cost. Existing immutable content must be referenced rather than copied into each report. Repeated diagnostic content should be deduplicated before compression.

The default automatic diagnostic-history policy retains at most 256 MiB of encoded history per user and expires eligible history 24 hours after the producing operation ends, or immediately when a replacement supersedes it, whichever comes first. The byte ceiling may retire older eligible history sooner; 24 hours is a maximum automatic history window, not a minimum retention guarantee. Inspecting a report must not restart that window. This policy must apply across projects rather than multiply into a separate full allowance for each project or operation. Explicitly retained investigations, pins and user-requested exports are separate roots and are not subject to this rolling history policy. Their allocation remains visible.

Supersession must identify the earlier result actually replaced for the same logical project, target or test selection and applicable build variant. It must not be inferred solely from completion time: concurrent operations can finish out of order. An unrelated passing operation, an incomplete replacement or an inspection must not supersede a failure report. Once a replacement is committed, superseded automatic history must release its retention immediately, without waiting for the 24-hour window. Unreferenced bytes must be reclaimed promptly when active readers and independently protecting roots no longer require them; shared payloads and deliberately retained evidence remain protected.

Evidence summaries and indexes must permit bounded lookup and selective decoding. Compression must be lossless for retained data. When a capture limit requires sampling, omission or truncation, the retained record must identify the incomplete portion and applicable limit; it must preserve the operation result and available causal diagnostic. Truncated data must not be described as a complete trace or proof of absence. If retention fails after an operation, the operation result and the retention failure must remain distinct.

RFC 126 owns investigation meaning, evidence relationships and completeness. Expiring eligible operation history must not silently destroy a deliberately retained investigation, change the certainty of a result, or imply that unavailable historical payloads are still present. Compression does not grant new capture or export authority.

### Compressed representation and restoration

Compression follows RFC 124: payload digests cover uncompressed produced bytes, and compression is a storage or transport encoding chosen per store or registry that never enters artifact identity. This RFC does not reopen that choice or standardize a new codec contract. Codec levels and cold-encoding thresholds are measured tuning within that existing policy. Retained evidence must support lossless streaming compression. Eligible cold artifact backing must support a compressed representation when that representation provides a useful measured storage benefit; codecs need not be applied where encoding would increase allocation or impose an unjustified restoration cost. Compression must not replace collection of content that policy no longer requires.

Each encoded representation must identify its format/version, codec parameters, encoded digest and length, canonical content digest and length, and the bounded materialization needed for use. These facts are storage metadata, not a new compatibility identity. The original payload, archive and attestation identities must remain verifiable over their canonical bytes.

A decoded checksum match is not a substitute for the original trust admission. Storage conversion must retain the original source/asset provenance, receipt and attestation evidence. Corrupt content must fail verification rather than become a usable candidate with a recomputed identity.

Compression must publish a verified replacement atomically and preserve a valid prior representation until the replacement is committed. If another root or filesystem alias still retains an expanded copy, that allocation must remain accounted for; deleting one path is not proof it disappeared.

Restoration must reserve space before expanding content, use bounded streaming memory, enforce a maximum decoded length, and reject data exceeding the declared bound. The selected representation must remain leased until verification and use are complete. Concurrent compatible requests must join one restoration or otherwise avoid unbounded duplicate expansions. Cancellation, malformed input, insufficient space and process failure must leave retained backing valid and release reclaimable private state after ownership ends.

When a compiler requires decoded or unpacked content, restoration must produce one reusable shared materialization for the compatible selected unit and payload, managed by the shared store. This is a representation of the retained asset, not a per-consumer copy of its Loaf. Consumers must reference that shared materialization directly or through symlinks; concurrent requests must not create an expansion per project or fixture home. An expired hot expansion may be reconstructed from retained backing without re-downloading, re-compiling or re-resolving the asset solely because it is compressed. Historical report inspection must not materialize the full artifact closure as a side effect.

### Inspection, collection and explicit pruning

`oven store status` must expose aggregate policy and accounting; `oven inspect cache` must expose selected unit, root and availability facts. Both must offer human-readable and versioned machine-readable output. Root kind, referrers, last use, expiry, encoding, availability and protection must be observable without reading compiler-internal directories. Pressure diagnostics, inspection, and dry-run previews must expose compatible structured facts and action consequences, so an agent can inspect a shortfall, compare supported choices, and request a precise action through the same public service used by human-facing commands.

`oven store gc` must enforce the declared automatic derived-state policy. `--dry-run` must not create store layout, reclaim staging, refresh use records, change registrations or write migration state. It may coordinate a read-only snapshot through existing synchronization. If concurrent work changes eligibility, actual collection must recheck it before mutation; the preview is not an enduring deletion authorization.

`oven store prune --downloads` must require an explicit asset selection. A package name that identifies several variants must be resolved to a visible selection rather than silently selecting every target/profile. Loaves and their existing package and asset identities remain the basis for selection; this RFC must not introduce a separate named offline-working-set abstraction or resolver. Tags, if supported, may filter or annotate selections but must not establish new package identity, retention authority, or offline availability. A tag selection must resolve to visible concrete roots and variants before a destructive operation. The dry run must show which persistent roots would be removed, which content remains shared or pinned, and what offline availability would change. Actual pruning must protect live leases and either refuse affected active content or offer a clearly reported deferred outcome. Deferred removal must be visible and cancellable before it takes effect.

`oven store prune --project` must explicitly select the project receipt registrations being retired. Its preview must show the resulting executable availability and shared protection. Retirement must not change authored source, lockfiles, historical receipts or requested outputs, and must protect live operations. Project and download selectors must not be combined ambiguously.

The absence of `--downloads` must never authorize removal of persistent downloaded roots. Removing a download root does not necessarily reclaim its bytes: another download, current receipt, investigation or pin may still reach them. Pruning must report that result honestly.

`oven store pin` and `oven store unpin` must operate on retention roots with stable identities and discoverable ownership. A root whose required closure is unavailable must not be advertised as an executable reproduction pin; a historical-reference pin is a distinct capability. Pin removal must not imply removal of every underlying payload.

### Consumer acceptance

Conformance must be exercised through ordinary public project workflows, using a large consumer graph as well as a small application. The heavy workload must include many packages, shared native dependencies, host tools, several profiles/targets, selected tests, concurrent commands, repeated source edits and dependency update/undo. Incan compiler and toolchain development must also be a required real-world case study, exercised through the same public Oven ownership and lifecycle as independent consumer projects. Compiler-repository checks alone do not prove the consumer contract.

Capacity calibration begins with Incan compiler and toolchain builds. Record their current behavior, optimize compilation reuse and storage lifecycle through the ordinary Oven services, then measure the resulting retained footprint and temporary peaks under repeated work. Use that optimized baseline to propose capacity defaults, and validate or revise those candidates against supported small and large consumer projects before adopting them as general defaults. The current unoptimized compiler footprint must not become the storage allowance by default.

The compiler case study must attribute storage to compiler/sysroot owners, SDK units, shared dependencies, test harnesses, requested outputs, temporary attempts and retained evidence. It must compare matched source, toolchain, target, profile, feature and concurrency conditions before and after lifecycle changes, including unchanged repetitions, compiler edits, SDK or driver changes, test-batch selection, cancellation and return to inactive work. Compatible units must share content; misses caused by genuine changed compilation inputs must remain correct and explainable.

The case study must report unique retained allocation, transient peaks, duplicate placements, actual compilation/link/copy work, evidence size and recovery/iteration time. A lower directory total caused by moving bytes elsewhere, dropping verification inputs, retaining a compressed whole-build archive or performing a manual purge is not proof of a better lifecycle. Results must show a stable footprint under repeated work and improved ordinary reuse/recovery, using compact reproducible evidence. Measurements on existing active work must not themselves authorize cleanup of user-owned or concurrently used content.

Acceptance must establish JEC work counts through ordinary consumer and compiler-case-study operations: zero compiler/SDK/dependency compilation and no unnecessary relinking on an unchanged repeat; rebuilding only units and dependents invalidated by effective input changes; selection-bounded test preparation/execution; shared preparation across a full suite; and recovery without unrelated graph materialization. Deliberate compilation inside a test must remain distinguishable from preparation overhead. Genuine compatibility misses must stay correct and explainable; suppressing identity checks cannot satisfy a work-count target.

For a fixed graph, variant set, concurrency and policy, repeated edit/build/test cycles must reach a bounded derived footprint after eligible expiry and collection. Growth must be attributable to current live content, persistent roots, explicitly retained history or bounded reservations. It must not remain proportional to the number of superseded generations. Tests must reconcile unique allocation, retained roots, actual compilation/link counts and copy work, not infer success from cache-hit text.

Acceptance must prove expiry of an inactive project's current-build registration without an additional artifact TTL, renewed protection on successful use, no renewal from inspection, and recovery of only the requested affected units while independent downloaded roots and pins survive.

Acceptance must prove direct shared consumption of a downloaded Loaf across projects, checkouts, fixture homes and filesystem boundaries with zero per-consumer payload copies. It must test symlink retargeting, canonical-owner leases, unavailable symlink support and a single shared restoration for concurrent consumers.

Acceptance must prove the 24-hour maximum automatic history window, immediate retirement on a committed superseding result, no supersession by unrelated or incomplete operations, correct out-of-order concurrent completion, and continued protection by active readers or explicit retention roots.

Acceptance must prove offline use of retained downloads after automatic maintenance and expiry of their hot expansions; compatible second-project reuse; justified target/profile/feature misses; unchanged unit and trust identities through compression; bounded selective report inspection; and bit-identical restoration of retained canonical content.

Negative cases must include build/prune races, dead launchers with live descendants, reused process ids, interrupted publication/compression/restoration/migration, truncated or oversized encoded content, all capacity protected, clock changes, cross-filesystem copies, incomplete evidence and unknown legacy ownership. A failure must preserve previously usable content and make missing capability explicit.

Latency measurements must include planning, preparation, maintenance, restoration, execution and publication as applicable. Existing small-program and admitted-dependency latency contracts remain in force under their stated conditions. A large-project result must report its actual scale and work instead of borrowing a small-program deadline or hiding a full-store scan outside the reported interval.

## Design details

### Defaults and policy ownership

The canonical user/host configuration file is `.oven/config.toml` beneath the user home directory, conventionally `~/.oven/config.toml`. Its storage policy applies across that user's projects and ordinary Oven commands. It is not a second project manifest. A checkout-local `.oven/config.toml` must not implicitly create another shared-store root or budget allowance.

The effective policy must expose root selection, derived idle TTL, failed-attempt TTL, derived-retention bytes, concurrent temporary admission, free-space headroom and diagnostic-history TTL/bytes. These are user or host policy, not authored dependency or compiler-identity inputs, and must not live in `loaf.toml` as source semantics. Relative storage paths in the user configuration must resolve against the configuration directory, not the invoking project's working directory. An explicit operation override must be visible and must not silently rewrite persistent user policy. Inspection must identify the effective configuration file and the source of each policy value.

The optional `[storage]` table defines the following keys. Omitting the table or any key selects its documented default; it must not require a per-project setup step.

| Key                   | Type                      | Meaning                                                                               | Default                                               |
| --------------------- | ------------------------- | ------------------------------------------------------------------------------------- | ----------------------------------------------------- |
| `root`                | Nonempty path string      | Shared Loaf-store location; relative to the configuration directory when not absolute | Documented per-user shared store                      |
| `max-derived-bytes`   | Nonnegative integer bytes | Idle recoverable development-state retention target, across projects                  | Optimized compiler baseline, then consumer validation |
| `max-temporary-bytes` | Nonnegative integer bytes | Aggregate allowance for concurrent temporary allocation/reservations                  | Optimized compiler baseline, then consumer validation |
| `min-free-bytes`      | Nonnegative integer bytes | Filesystem free-space headroom to preserve during admission                           | Optimized compiler baseline, then consumer validation |
| `development-ttl`     | Duration string           | Inactivity limit for reusable derived state and current development protection        | `"7d"`                                                |
| `failed-attempt-ttl`  | Duration string           | Maximum retention of an ended private attempt                                         | `"24h"`                                               |
| `history-ttl`         | Duration string           | Maximum automatic diagnostic-history window; supersession retires it sooner           | `"24h"`                                               |
| `history-max-bytes`   | Nonnegative integer bytes | User-wide encoded automatic diagnostic-history ceiling                                | `268435456` (256 MiB)                                 |

Duration strings must contain a nonnegative integer followed by `s`, `m`, `h` or `d`, meaning seconds, minutes, hours or 24-hour days. Invalid values, unknown keys within `[storage]`, overflow and malformed configuration must produce an actionable diagnostic naming the file, key and accepted form. They must not silently fall back to another value. Other configuration tables retain their own owning contracts.

A zero derived-retention target keeps no unprotected idle allowance; it must not remove downloaded roots, pins or active content. Zero temporary allowance admits no positive temporary reservation. Zero free-space headroom adds no configured reserve but does not bypass actual filesystem admission. A zero history ceiling or TTL disables automatic historical payload retention; operation results remain observable and explicit retained evidence remains independently protected. A zero development or attempt TTL makes eligible ended state immediately expirable without bypassing live leases or other roots.

For example, the agreed retention defaults can be written explicitly without specifying capacity overrides:

```toml
[storage]
development-ttl = "7d"
failed-attempt-ttl = "24h"
history-ttl = "24h"
history-max-bytes = 268435456
```

This Draft proposes seven days for reusable derived state and current development protection, 24 hours for ended private attempts, and 24 hours or immediate supersession, capped at 256 MiB, for automatic diagnostic history. The derived and transient byte defaults require measurements against supported consumer closures before this RFC becomes Planned. Defaults must not be selected merely to fit one maintainer's compiler fleet. The policy must always expose protected bytes that can exceed an idle-retention target.

Store relocation and multiple user-selected stores must retain trust and ownership isolation. User-wide sharing does not mean a world-writable machine-wide cache or permission to consume another user's private content. An explicit isolated store is an understandable override, not a second lifecycle contract.

### Relationship to the existing RFCs

RFC 124 continues to own unit and payload identity, interoperability, candidate selection, conflict handling, shared storage, and compression as an identity-independent store or registry encoding. This RFC refines its reachability statement: a receipt retains executable content through a live retention registration; keeping a historical receipt as evidence does not register an immortal root. The corresponding RFC 124 clause must be aligned before this proposal becomes Planned. This proposal also narrows RFC 124's general materialization fallback for downloaded-Loaf consumption: consumers use admitted shared paths or symlinks, rather than persistent per-consumer hard links, reflinks or copies. Explicit publication of a user-requested distributable output remains a separate operation whose allocation and contents are observable; it must not be used to create a hidden dependency cache.

RFC 118 continues to own canonical command placement and aliases. The proposed store and inspection operations must be recorded there; an Incan convenience alias must use the same Oven service and policy. This RFC does not give the Incan CLI an independent cache manager.

RFC 125 continues to own registry authority, source publications, signed archives and asset admission. Its archive digest, unit identity, payload digest, and attestation contracts remain distinct and unchanged. Local representation follows RFC 124's existing storage-encoding rule; this RFC does not redefine the registry archive format or claim that RFC 125 limits compression to transport. Downloads retain roots even when the reusable units were originally baked locally.

RFC 126 continues to own durable investigations and evidence availability. Retained investigations register their required content through the same lifecycle, and report links preserve meaning after eligible content becomes unavailable. This RFC defines storage and availability mechanics, not a second interpretation of observations, hypotheses or verification.

RFC 061 and RFC 112 supply compression and coordinated publication capabilities. This RFC consumes those contracts without prescribing additional language syntax or granting a new native-authoring exception.

### Migration and mixed toolchains

Existing stores must remain readable while compatible entries are admitted into the shared lifecycle. Migration must adopt verified downloaded content in place or reference its existing admitted owner; adopting metadata must not copy Loafs from a pub cache into a new Oven cache. Migration must preserve identity, trust evidence and requested outputs and must not require a full second copy of every existing cache. An explicitly requested physical store relocation is a separate operation; any required transfer must account for temporary allocation before starting and support interruption without making the original unusable. Relocation must not become an implicit dependency-consumption fallback.

Unknown legacy ownership or download provenance must not be assumed disposable. Migration may classify such content conservatively and expose explicit adoption or pruning choices. It must not solve uncertainty by copying the entire legacy store into an unbounded evidence archive.

Older toolchains may continue using their known layout. Shared immutable content is permitted where the identity contract proves compatibility; new lifecycle metadata must not cause an older writer to mutate admitted immutable bytes or mistake compressed storage for executable input. Reclamation requires the applicable roots/leases to be known. Unsupported mixed-writer configurations must be diagnosed, not treated as permission to ignore their owners.

### Just Enough Compilation and artifact cost

JEC is the primary work and storage contract, following DD-0005. Ordinary operations must select the required Loaves, reuse compatible baked Loaves and use the RFC 124 identities of their compiled units to determine exactly which work remains. Baking and rebuilding must be limited to effective input changes or genuinely missing required content, and relinking must be limited to what the selected result requires. An unchanged repeat must perform zero compiler, SDK or dependency compilation and no unnecessary relinking, closure copying or restoration. A compiler edit alone must not justify preparing an entire SDK, dependency graph or test fleet again.

A selected test or fixture must prepare only its required units and execute only the selection. Full-suite work must share common preparation across cases, and compilation deliberately performed by a test must be reported separately from preparation of the compiler and test framework. Eager dependency preparation remains bound to the active selected graph under DD-0005; it does not authorize unrelated targets, a complete SDK or an unselected suite. Recovery after expiry must use JEC rather than restoring or rebuilding a whole historical environment.

Storage acceptance must first identify and eliminate unnecessary units, duplicate placements, repeated preparation and superseded generations. Compression applies to necessary retained content. Measuring or reducing the byte size of an individual required unit is secondary and must not substitute for JEC conformance. This RFC requires attribution of storage and repeated work, not an assumed diagnosis that a particular compiler backend is solely responsible. A large native unit and many duplicate generations are distinct costs. Compression measurements must report allocation, encoding/restore time and temporary-space/memory peaks on representative logs, metadata, native libraries and executables.

Changes to compilation profiles, feature closures, symbols, bitcode or unit boundaries must preserve their owning compiler and debugging contracts. Such changes may improve the consumer workload, but their measured tradeoffs and any required separate architectural decision are not replaced by this lifecycle RFC. In particular, dormant external-identity optimizations in RFC 124 are not a prerequisite for sharing identical compatible units safely.

### Prior art

uv demonstrates a shared concurrent cache with explicit cleanup operations and versioned cache buckets. Its documented `cache prune --ci` policy discards downloaded wheels while retaining source-built wheels, which is a useful example of role-sensitive retention but differs from this RFC's local downloaded-Loaf guarantee. Its guidance also warns that persistent-runner caches can grow unbounded. Sharing therefore needs a stated lifecycle rather than an assumption that a common directory is sufficient. See [uv caching](https://docs.astral.sh/uv/concepts/cache/) and [uv persistent-runner guidance](https://docs.astral.sh/uv/guides/integration/github/).

## Alternatives considered

### One bounded cache per checkout or test environment

This makes isolation simple but multiplies retained closures and allowances with project and attempt count. Explicit isolated environments remain available for tests that require them; they are unsuitable as the ordinary default for immutable dependencies.

### Automatically evict every idle artifact under one LRU budget

This gives a simple byte target but makes downloaded offline capability disappear as a side effect of unrelated development. Role-sensitive retention roots allow automatic derived-state cleanup without that behavior. Persistent data may therefore require deliberate pruning or a larger allowance.

### Retain every historical receipt as a permanent executable root

This preserves old executions by default but makes compact reports indirectly pin every compiler, dependency and output they ever name. Separate historical provenance and explicit reproduction roots preserve honest history without that allocation policy.

### Archive whole build directories as evidence

Compression can reduce some bytes, but the archive still duplicates unrelated inputs, retains disposable generations and requires large restoration. Small reports and references are the default; a complete retained execution closure is explicit, deduplicated and budget-visible.

### Compress everything immediately or only compress logs

Immediate compression adds work to hot paths and can expand incompressible content. Log-only compression leaves cold retained payloads unnecessarily expanded. The proposed model compresses retained evidence and supports measured cold-payload encoding while preserving directly usable hot selections.

### Move caches to another volume and leave their lifecycle unchanged

Relocation can be useful but cannot bound growth, remove abandoned generations or coordinate concurrent allocations. Cross-filesystem placement can also turn sharing into copying. Location is policy; lifecycle remains a product obligation.

## Drawbacks

Retention roots, reservations and encoded representations add metadata and recovery complexity. TTL bookkeeping can itself become costly unless warm selection avoids global scans. Conservative treatment of unknown legacy ownership can temporarily retain more data than a new installation would.

Persistent downloads and explicit pins mean the user can retain more than the idle derived target. Oven must explain that condition; it cannot both guarantee persistence and silently enforce a lower total by deleting those bytes. A required restoration can fail on a genuinely full disk even though compressed backing remains valid.

Compression spends CPU and may increase first-use latency. Some filesystems cannot expose exact unique allocation or reclaimed extents. Measurements and inspection must preserve those limits rather than presenting overly precise space savings.

Bounded diagnostic history means old unpinned details can become unavailable. Summaries, explicit retention/export and honest availability make the choice visible, but users needing durable binary reproduction must request it.

## Implementation architecture

This section is non-normative. The existing Oven store remains the physical owner of immutable content, publication, leases and reservations. Incan-authored control-plane policy operates on owned typed snapshots; it does not reconstruct compiler facts or create another artifact graph. An index of live roots and last successful selections can make collection incremental, while a versioned encoded-content descriptor permits hot and cold placements under one logical identity.

Existing Incan streaming compression and filesystem interop should be used where they satisfy the bounded operation. Any missing capability requires a minimal demonstrated and tracked limitation before a narrow native workaround. A resident service may improve coordination, but ordinary commands must retain a correct nonresident execution path.

## Layers affected

- **Oven store and execution lifecycle:** shared immutable selection, retention roots, leases, reservations, recovery, compression, restoration and honest accounting must obey one contract.
- **Oven control plane:** policy must distinguish persistent and disposable state, coordinate ordinary workflows and preserve exact unit/trust identity.
- **Registry and package consumption:** successful asset admission must establish persistent local retention for the available closure without duplicating shared units.
- **SDK and toolchain providers:** reusable units and compiler owners must participate in shared selection and lifecycle instead of requiring independent complete stores per consumer.
- **CLI, editor and automation surfaces:** inspection, progress, pruning, pins and availability must expose consistent service facts and actionable pressure outcomes.
- **Evidence and investigations:** compact history, explicit retained captures and selective compressed retrieval must preserve completeness and availability semantics.

No parser, type-system, formatter or source-language syntax change is proposed.

## Design Decisions

- Idle derived-retention and temporary allocation admission are separate user-wide controls. Exceeding the idle cache target does not by itself prohibit an operation that can safely reserve the required space. Concurrent work coordinates reservations and preserves configured free-space headroom.
- Persistent downloaded assets and explicit pins remain protected even when they exceed the idle derived-retention target. Their allocation and the reason for protection remain visible.
- Oven exposes versioned structured pressure diagnostics and action consequences for deliberate human and AI-agent decisions. Required and available space, reservations, protected roots, reclaimable content, uncertainty, applicable actions, and availability or rebuilding consequences must be inspectable without parsing prose. Actions recheck current state before mutation; a preview does not silently change policy or grant an enduring deletion authorization.
- A new caller decision may use an explained proceed/decline prompt in interactive human-facing execution. Machine-readable and noninteractive execution instead exit without prompting, with a nonzero status and structured decision-required details so an agent can choose a follow-up action. Explicit invocation mode governs this behavior; decision-required outcomes remain distinct from success and unrecoverable failures. The existing `--format json` convention explicitly selects noninteractive structured output; decision-required outcomes use exit code 3. Existing codes remain 0 for success, 1 for application failure, and 2 for usage error. RFC 118 records the shared semantics.
- Loaves and selected verified asset variants provide package organization and download selection. No separate named offline-working-set abstraction is introduced. Tags may be optional selection metadata, but this RFC does not require a tagging API or let tags create package identity, retention authority, or availability guarantees. Destructive selections still resolve to concrete roots and variants.
- Compression inherits RFC 124's existing store/registry encoding policy and identity rules, while RFC 125 continues to own archive verification and asset admission. RFC 145 adds retention, safe representation lifecycle, bounded restoration, and accounting. Codec level and cold-encoding thresholds are measured tuning rather than a new design decision.
- Persistent storage policy belongs in the canonical user/host Oven configuration surface. First optimize and measure the compiler's own builds; derive candidate capacity defaults from that baseline, then validate them against supported small and large consumer workloads with useful headroom. The default byte values remain unresolved pending that evidence.

## Unresolved questions

- What defaults for `max-derived-bytes`, `max-temporary-bytes` and `min-free-bytes` follow from the optimized compiler-build baseline and subsequent consumer validation? The calibration order, separate controls, coordinated reservations, protected-content accounting, user/host policy ownership, `~/.oven/config.toml` location and `[storage]` keys are settled; these three capacity values require measurement.

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
