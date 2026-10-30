# Bakery storage design

This page is the storage contract under [DD-0013: The Incan build engine](../../design_records/0013_the_incan_build_engine.md), accepted with the [build engine design](build_engine_design.md) on 2026-10-30. DD-0013 governs where the two differ. In particular, DD-0013 supersedes DD-0012's rule that a damaged record is a miss; §5.3 gives the recovery rules.

**Scope:** one embedded store per physical bakery, with Incan homes as virtual stores over it, and portable sealed record sets for Loaves and the registry.

## 1. Decision

Build an **indexed, immutable-record physical storage profile inside IncQL-DB's snapshot and transaction contract**. The bakery is its first compiler/build-system client, not a separate database. Keep the new substrate Incan-authored, temporarily behind the existing storage module boundary in Oven, with no compiler or Oven types crossing into that module.

The same store admits facts from three producers: a local computation, a dependency Loaf, and a toolchain or registry bundle. Admission has one implementation. Reading an admitted fact does not depend on which producer supplied it.

The dependency boundary is deliberately small: the substrate depends on model/schema contracts and filesystem/hash/I/O capabilities, not on IncQL's planner, Substrait, Protobuf tooling, or DataFusion. Full IncQL can query it through an optional adapter. The compiler does not have to initialize a query engine to read a signature, and indexed edge access does not wait for a new graph-traversal language syntax.

The implementation has four durable concepts:

1. **Typed records and immutable revisions:** independently addressable semantic facts, bindings, relationships, and receipts.
2. **Immutable physical objects:** indexed record segments, index pages, and content-named file payloads.
3. **An atomic snapshot manifest:** the roots that make a consistent set of those objects visible.
4. **Explicit roots and leases:** what must remain available, and to whom, during collection.

```text
Local computation        Sealed dependency Loaf       Toolchain / registry asset
       |                          |                              |
       +--------------------------+------------------------------+
                                  |
                    verify + admit missing objects
                                  |
                    IncQL-DB transaction publication
                                  |
                 one home: records, indexes, payloads
                       /          |           \
                 compiler       Oven       LSP / MCP
                 read scope   file lease   bounded query
```

This implements the design of DD-0012: Loaves arrive with frozen facts; project changes seed a ranked graph; consumers read shared facts; only missing computations run. Consumer-specific generic instantiations are new facts, not mutations of the supplying Loaf.

**Do not implement another list of complete dependency manifests in memory. Do not make every Loaf a separately searched database. Do not replace the compiler's type or dependency semantics with storage heuristics.**

### 1.1 What is adopted, and what is new

| Authority or existing state | Adopt | Add or change explicitly |
| --- | --- | --- |
| IncQL-DB whitepaper, RFC 067 | Record families, stable logical identity, immutable revisions, exact bindings, evidence, bounded snapshot reads; storage records decisions | Point-addressed record segments as an additional physical profile, alongside the columnar analytical direction |
| IncQL RFC 060 | Immutable applied state; predecessor validation; commit the result before advancing its state; compare incremental work against full-result work | Compiler adapters supply the affected closure and read witnesses; the store does not infer them |
| IncQL-DB foundation | Snapshot and parent IDs, staged manifests, atomic `CURRENT` replacement, file and directory synchronization, snapshot-bound payload references | Indexed opening instead of validating every historical generation; transaction-scoped publication instead of a session-long exclusive lock; leases, indexed segments, collection |
| DD-0012 | Per-declaration facets, name slots, ranked compiler DAG, early cutoff, frozen Loaves, both-direction edges, fingerprints, deterministic compaction | Generic storage mechanisms underneath; compiler rank and evaluation remain outside the substrate |
| RFC 124 / 125 | Full unit identity, payload identity, provenance separation, exact asset admission, receipt roots, source fallback | Portable sealed record-set import/export and selective namespace binding in the same home |
| Bakery segment slice | Sorted immutable segments, content naming, separate file payloads, claim/recheck, serialized manifest publication | Collision handling, authenticated lookup, error distinctions, durable publication, safe pins/GC, bounded compaction, a manifest-level locator |

The foundation is not yet the RFC 067 semantic store. Its implemented families and revisions are simpler registration records, and payloads are separate files. Conversely, RFC 067 and RFC 060 are Draft contracts, not implementation evidence.

The first bakery slice used file range reads. Later slices integrated bakery records and native consumers and added `Path.map_read()`. In that code, `MappedFile.read()` returns a **copy**, snapshots map all listed segments, `get()` returns `Option[bytes]`, and publication and collection still need the corrections below.

### 1.2 Non-negotiable distinctions

- **Logical record identity is not a content hash.** Equal values may belong to different records. Equal content can share a physical object without merging ownership or provenance.
- **A 128-bit fingerprint is not the full RFC 124 unit ID.** Keep the latter's full SHA-256 identity.
- **Damage is not absence.** A reproducible compiler fact can be recomputed after an integrity error; user memory cannot silently disappear. Never fall through a damaged newer entry to an older value.
- **An immutable snapshot is not a cache of decoded records.** Its handle pins roots; activation is separately scoped.
- **Compaction is not collection.** Compaction preserves its input's logical contents. Collection explicitly changes what remains retained.
- **Stored once is not zero temporary space.** Compaction, imports, retained old snapshots, and modified COW views have separately accounted costs.
- **The database is not the producer.** Schema checks and transaction validation belong here; compilation, compatibility judgment, registry trust policy, and agent admission decisions do not.

## 2. Atoms, Identity, and Relationships

### 2.1 One semantic fact, not one source file

An atom is the smallest **independently reusable and invalidatable typed record revision**. Physical index pages and data blocks are not semantic atoms.

| Compiler family | Logical record / activation boundary | Important dependencies |
| --- | --- | --- |
| Namespace descriptor | One namespace and its paged member index root | Selected package or project binding |
| Name slot | One `(namespace, spelling, lookup kind)`; present or explicitly absent | Namespace membership; exact resolution result |
| Import binding | One local import selecting an exact namespace/export | Selected Loaf root and the relevant name slot |
| Signature | One declaration or overload's checked public signature | Referenced types, constraints, defaults where semantically required |
| Layout / ABI fact | One type or callable under the exact ABI/target profile | Layout inputs, target ABI, producer identity |
| Default expression | One declaration parameter's checked default | Its checked references, not its neighbors' bodies |
| Checked body | One function/method body revision | Observed signatures, names, constants, defaults, and other actual semantic reads |
| Body IR | One function's IR under a lowering profile | Checked body and exact lowering inputs |
| Generic instance | One generic body plus canonical type/value arguments and relevant profile | Frozen generic body revision, argument identities, selected environment |
| Plan function | One lowered function under a code generation profile | Body IR / instance and exact backend inputs |
| CGU membership | One stable code-generation unit's member list, paged when large | Exact plan-function revisions; membership must not depend on discovery order |
| Native unit | One complete RFC 124 unit identity | Exact dependency external identities and all admitted producer inputs |
| Receipt / derivation | One output and its input witnesses | Exact revisions and payload digests actually used |
| Source projection | Source location and diagnostic/inspection information | Source revision; excluded from semantic reuse only where the compiler contract permits |

Large lists, relationship fan-out, and large bodies use bounded blocks referenced by one revision. Reading a member or one block must not deserialize its entire owner. A small record remains inline in a segment; an rlib, executable, dylib, source archive, or other file required by a native tool remains a **content-named payload file**. Putting an rlib inside a segment and extracting it on every build defeats both file sharing and activation granularity.

Preserve the foundation's snapshot-bound `PayloadRef` semantics: its snapshot/family/kind/payload ID resolves through a committed record to the full digest and length. Changing physical storage to a shared CAS path does not authorize a caller to bypass that binding. Equal payload bytes may share one file while different snapshots retain their own explicit references.

This is still the RFC 067 hierarchy:

```text
RecordFamily -> LogicalRecord -> Revision -> typed fields / payload references
                                     |
                                     +-> exact read witnesses / relationships
Snapshot ----------------------------+-> visible revision bindings
```

The substrate supports typed family codecs and schema IDs. It does not contain `Signature`, `Loaf`, `rustc`, or `CGU` definitions. Those are Incan-authored compiler/bakery models over the generic record contract. The record view is typed; the on-disk bytes are a portable encoding, not a dump of a Rust struct or a permanently resident model instance.

### 2.2 Six identities, deliberately separate

| Identity | Meaning |
| --- | --- |
| `RecordKey` | Canonical identity profile, family, origin scope, and logical identifier; no physical path or current generation |
| `RevisionId` | An exact immutable revision of that record, including its schema contract |
| `ValueDigest` | Full digest of the canonical semantic value; a 128-bit prefix accelerates comparisons |
| `DerivationKey` | Producer/profile identity plus the input identities and witnesses that justify reuse |
| `PayloadDigest` | Full SHA-256 of a native file or externally stored byte payload |
| `Location` | Segment/page digest and offset/length; replaceable by compaction without changing logical identity |

A fast key begins with a 128-bit fingerprint, but the leaf and record carry enough canonical identity to resolve collisions. Compare the full key; never choose an entry on the prefix alone. For early cutoff, compare the full semantic digest after the 128-bit check. Collision-injection tests must exercise this deliberately.

RFC 124 unit keys and file payload hashes remain full-width. Names, versions, feature summaries, and paths discover candidates; they cannot substitute for exact unit identity. Equal unit IDs with unequal native payloads are determinism conflicts: retain explicit machine-local variants and pin the chosen digest in receipts as RFC 124 requires; do not export them as interchangeable registry assets.

The current compiler's canonical identities and semantic digest machinery are the starting authority, not strings synthesized from pretty-printed types. The record codec must explicitly project positional source spans out of semantic keys while retaining meaningful expansion/overload identity. A versioned identity projection and golden fixtures are required; deleting every field called `span` is not a design.

### 2.3 Edges, witnesses, and negative reads

One authoritative edge record contains `reader revision`, `read kind`, `target logical key`, `observed revision or absence witness`, and `observed value digest`. Maintain forward and reverse indexes atomically from that record. They are two access paths, not two independently authored truths.

The reverse index is searchable by the **target logical key**, not only by the old revision ID. Otherwise replacing a revision would hide the readers that need invalidation. The snapshot's active reader revisions filter historical reverse edges. Index high-fan-out adjacency as paged ordered entries, not one giant serialized list.

An unsuccessful name lookup records a name-slot absence witness. Adding that name then invalidates the lookup. Range enumeration also records a range/index-generation witness so a concurrent insertion cannot escape validation. A missing physical file is never an absence witness.

If recomputation changes the read set but produces the same value, store the new derivation/witness set while reusing the equal value bytes. **Do not discard changed dependencies just because propagation stops.**

IncQL-DB allows generic typed graphs, including cycles. DD-0012's namespace/phase DAG is a compiler invariant. The compiler supplies the rank, detects forbidden cycles, and checks that every actual semantic read fits it. A newly discovered earlier dependency must cause validated rescheduling or a diagnostic; the store cannot silently pretend the one-sweep precondition held.

## 3. Physical Layout and Indexed Reads

### 3.1 Layout

Use the configured IncQL-DB directory as the root. The foundation uses `.inqldb` terminology and the later whitepaper uses `.incqldb`; this design does not rename existing stores. The bakery's existing home path is configured by Oven, not hard-coded into the substrate.

```text
<store>/
  FORMAT                         binary format / required-capability descriptor
  CURRENT                        checked pointer to one committed manifest
  snapshots/<generation>.manifest
  segments/<sha256>.seg           immutable base and delta record packs
  indexes/<sha256>.page           immutable COW directory pages
  payloads/<first-two>/<sha256>   files consumed directly by native tools
  staging/<transaction-id>/       isolated, exclusively owned preparation
  leases/<lease-id>               process-liveness lock + snapshot/root descriptor
  locks/publication              serializes commits and maintenance publication
  locks/lifecycle                short snapshot/root-registration handshake
  locks/claims/<bucket>           persistent lock identities, never unlinked in service
```

Logical family registration, revision bindings, source provenance, receipts, imported record sets, and durable roots are records behind indexed roots. They are not additional independent JSON catalogs. Export may use a transport wrapper, but the live lookup path reads binary pages and typed records.

This is a new physical format capability, not a byte-compatible reinterpretation of the foundation's `INCQLDB` format or the slice's `INCSEG` format. Preserve the existing snapshot/payload contract. Convert old stores through a verified streaming export/import into a new root, retaining the original until equality and recovery tests pass. Reject unknown required capabilities; never open an unsupported store as empty.

### 3.2 Record segments

A segment contains a versioned fixed header, a sorted paged record directory, record frames, and integrity metadata. Choose a little-endian, offset-based, uncompressed first profile:

- Header: magic, format version, identity/codec profile, section offsets and lengths, record count, directory root and integrity descriptor.
- Directory: ordered key fingerprints with full-key disambiguation, revision identity, frame offset/length, expected frame/block digest, and explicit value/tombstone kind.
- Frame: family/schema identity, logical key, revision metadata, typed field directory, fixed-width scalars and bounded relative-offset arrays/strings.
- Large values: paged block references with a digest per block and an authenticated root; an activation can request specific blocks.
- All offsets and lengths are checked with overflow-safe arithmetic before access. No native pointers, absolute paths, hash-map iteration order, timestamps introduced by the encoder, or uninitialized padding.

Typed accessors read scalar fields and bounded slices in place. A requested owned model may be decoded into the request arena. Do not use unsafe native-struct casts. JSON is not the primary record encoding. Compression can be added as a versioned block codec after measurement; whole-segment compression would undo selective mapping and shared clean-page access.

The format fixture must specify field tags, canonical ordering, string/number encoding, alignment, and page packing before two writers ship. Native compiler codecs that are still JSON can be admitted as explicitly versioned payloads during transition, but that is not proof of typed in-place access.

### 3.3 The manifest-level locator

**Choose a persistent, authenticated, copy-on-write B+tree directory.** Its leaves point directly to the selected frame or tombstone. Segment-local sorted directories remain useful for validation, import, compaction, and export; ordinary reads do not binary-search every base and delta.

Directory keys include the relevant index kind. The snapshot has roots for family/schema lookup, logical revision bindings, immutable record locations, forward/reverse edges, and retained roots. They can use the same page implementation. A lookup traverses only its selected root.

The immutable-location index is keyed by `(full RecordKey, RevisionId)`. A logical read first resolves the snapshot's binding to that exact revision; an already pinned reference bypasses only that resolution step. It never asks a home-global latest-value map. This is what permits two Loaf versions and their consumers to coexist without import-order behavior.

Use 4 KiB format pages initially, independently of the host's VM page size. Standalone immutable index pages are content-addressed files in the first implementation. Canonical compacted bases may embed their directory pages, addressed by segment plus offset. A node reference supports either form and carries its expected digest; embedded self-references use relative offsets, not the segment's as-yet-unknown content hash. This avoids circular hashes. It also gives the implementation a simple first COW writer without inventing a mutable index service.

Page-file metadata/open costs are real and measured. Packing more index pages is a compatible physical optimization only if it preserves bounded reads and digest validation. The directory is on disk, not a process-local dictionary rebuilt by scanning segment headers.

A manifest holds constant-size root descriptors and the root of a paged object inventory, not a fully decoded list of all segment handles. The base/delta inventory is for maintenance; it does not determine per-key read amplification. Neither opening a snapshot nor looking up a record walks its parent history.

Optional Bloom filters are accelerators only. A corrupt or missing filter cannot prove absence. Authenticated directory pages provide the actual positive/negative lookup result; data-frame hashes validate the returned record without hashing an entire segment per read. Verify whole-object digests during admission and audit.

### 3.4 Snapshot contents

A committed manifest contains the store identity, physical generation, parent manifest identity, logical view identity, schema/capability versions, directory roots, object-inventory root, durable-root-set root, transaction ID, and a digest of its canonical contents. `CURRENT` includes the generation and expected manifest digest.

A physical compaction creates a new manifest with a parent, but preserves the logical view identity and record/revision identities. A logical write creates a new logical view. Receipts must distinguish these identities so maintenance does not appear to be a semantic edit.

`SnapshotHandle` holds the store handle, manifest identity, selected roots, and its lease token. It does not eagerly map every segment. A handle promises a pinned view, not unlimited history retention after it is released.

### 3.5 Cost model, including the 6-16 ms figure

The working figure is **6-16 ms per memory activation**. No corresponding disk-index benchmark exists in the IncQL-DB foundation or the 2026-10-29 baseline measurement. Treat it as a reference workload figure to reproduce, not a measured latency for an individual compiler record read.

Let `h` be directory height, `r` the selected frame bytes, `P` the host page size, `u` the number of distinct pages touched, and `f` the pages not already resident. Then:

```text
cold lookup  = open/map metadata + f * measured page-in cost
               + page validation + selected-frame validation + typed access
warm lookup  = directory comparisons + validation of newly visited pages
               + selected-frame access + any explicitly requested decoding
read bytes   = directory pages actually visited + selected frame/block bytes
physical I/O = measured page-ins/read-ahead, not read bytes and not mmap length
```

At an illustrative fan-out near 32, one million entries need roughly four directory levels. A small cold lookup touches approximately four index pages plus its record pages, with additional filesystem metadata and snapshot-open work accounted separately. With upper pages warm, the new work is usually a leaf and the selected record. This is a model, not a timing claim; actual entry sizes and host VM pages must be recorded.

The current flat 48-byte sorted index for one million entries is about 48 MB and has about 20 binary-search probes per segment. Searching 17 runs can perform hundreds of probes. Mapping avoids wholesale copying, but does not remove that lookup amplification. The manifest-level locator is the remedy.

For a compile, measure `U`, the union of pages across its actual reads. Request-scoped mapping reuse and `read_many(keys)` should pay for shared upper pages once. Do not put a process/RPC/open-close round trip around every atom. Ten thousand serial reads at 6-16 ms each would cost 60-160 seconds; that is not an acceptable interpretation of the activation target.

The baseline std-manifest and fact phase is about 35 ms total. If the eventual trace shows 500 relevant reads, the entire replacement path must fit an average budget below 70 microseconds/read to improve that phase. The count is illustrative; measure it before assigning a latency budget. Publish p50/p95/p99 for cold-home admission, cold pages, warm pages in a new process, and repeat reads within one request separately.

## 4. Memory, Mapping, and Lifetime

### 4.1 Activation contract

The generic API vocabulary below is a design vocabulary, not existing Incan syntax or shipped APIs:

```text
open_snapshot(scope) -> Result[SnapshotHandle, StoreError]
begin_read(snapshot) -> ReadScope
lookup(scope, full_key) -> Found[RecordView] | Absent[Witness] | StoreError
read_many(scope, keys) -> bounded iterator of the same outcomes
read_field(view, field) -> scalar or borrowed bounded view
decode(view, family) -> explicitly owned typed model
open_payload(scope, exact_ref) -> leased immutable file handle/path
end_read(scope) -> release mappings, decoded arena, and temporary witnesses
```

`RecordView` cannot outlive its mapping/read scope. Returned graph/query answers own only the requested fields or keep an explicit bounded lease. Lazy iterators cannot retain an entire compiler session invisibly.

The first `MappedFile.read()` implementation returns copied `bytes`; it is useful for bounded reads but does **not** satisfy borrowed zero-copy access. In-place scalar readers and a lifetime-safe mapping-backed slice/view are the needed `std.fs`/language surface. If ownership cannot express that safely, track the concrete language gap; do not author a second Rust storage engine or label copies zero-copy.

File mappings require an immutable backing inode throughout use. Read-only mode bits alone do not make owner-writable files immutable, and replacing a pathname is different from truncating the mapped inode. The store never rewrites/truncates a published object, never gives native tools a writable canonical payload, and does not map untrusted files before admission. The platform wrapper must document this safety boundary. [memmap2 safety contract](https://docs.rs/memmap2/0.9.11/memmap2/struct.Mmap.html)

### 4.2 What can remain resident

Between requests, the storage client retains only a small snapshot descriptor/lease and fixed configuration. It retains no decoded Loaf manifests, `RustAbiCache` closure, Body IR forest, semantic graph, or global key directory.

Within a request, mappings and decoded records have explicit owners. Use a request-scoped byte-accounted LRU for mapping windows and validation results; evict unborrowed entries, and apply backpressure or return a bounded-query error when active borrows exhaust the working budget. This is a working-memory bound, not a database capacity limit. Large iterators release pages incrementally. Teardown drops the entire cache; no warm daemon cache of semantic objects is required.

Account mapped virtual address space, touched/resident file pages, and private copied/decoded bytes separately. A whole-file mapping does not mean the file is physically resident. Windowed mapping is a further platform capability if address-space/handle limits require it; it must not be simulated by copying an entire segment into a heap buffer.

Unmapping releases process mappings; it does not order the OS to evict clean cached pages. Other readers can reuse those pages. Never use `mlock` or eager prefaulting of a whole closure to make a benchmark look warm. [Linux mmap semantics](https://man7.org/linux/man-pages/man2/mmap.2.html)

The compiler still needs active parser/checker/lowering state and rustc's working set. The target is **zero idle storage-owned decoded graph state**, not literally zero process memory. A true held snapshot also retains its disk-visible contents. An idle worker should release broad inventory leases; it can hold a scoped snapshot, or an unpinned token that must be reopened and may have expired. Do not call the latter a live immutable snapshot.

### 4.3 Sharing and accounting

Map the **same canonical files** in each process. That gives the OS the opportunity to share clean file-backed pages; decoding each record into separate heaps still adds private memory. Copy-on-write clones share storage extents, but are not a promise of page-cache sharing between distinct inodes. Native reads should therefore use canonical payload paths where possible.

Counters per request: requested keys, distinct keys, hit/absence/damage counts, directory pages visited, frame bytes examined, bytes copied, decoded bytes, peak active views, mappings opened, and records recomputed. Export them out of band, not to stderr compared by contract tests.

Measure process RSS, private dirty/anonymous bytes, file-backed pages, faults, and physical I/O. On Linux use timestamp-aligned `smaps`/`smaps_rollup` observations and PSS, not the sum of RSS. On macOS record `vmmap` classifications and physical footprint with instrument availability stated. Do not present those platform metrics as interchangeable. [Linux per-mapping RSS/PSS accounting](https://man7.org/linux/man-pages/man5/proc_pid_smaps.5.html)

Run 1, 2, 4, and 8 processes reading the same key set, then disjoint sets. Measure simultaneous totals and retained private memory after teardown. The expected shared-page advantage is a hypothesis to prove on each supported filesystem/platform, not `N * file_size` savings asserted from the presence of mmap.

## 5. Writes, Claims, and Recovery

### 5.1 Many producers, one commit at a time

Keep a serialized publication point. Concurrent bakers perform expensive computation and staging outside it. This preserves the foundation's single-writer commit model while removing its session-lifetime writer exclusion.

The claim key is the **full derivation or unit identity**, not a package name and not an isolated 128-bit prefix. Claim/recheck prevents concurrent work on the same reusable result. Different keys can compute concurrently; their commits serialize briefly.

Persistent claim-lock identities must not be deleted and recreated while waiters exist. A fixed striped-lock pool is acceptable initially: hash collisions serialize unrelated claims but cannot alias their results. Hold at most one computation claim at a time, or enforce a total ordering. Dependency production precedes claiming the dependent computation; never deadlock two claims by recursively waiting in opposite orders.

If evaluation discovers an uncomputed dependency, release the dependent claim, satisfy that dependency, and recheck before retrying. A producer must not keep a striped claim while blocking on another stripe that may resolve to the same lock.

Kernel release of a dead process's advisory lock establishes liveness; a PID string or elapsed timeout does not. Every claimant rechecks a fresh committed snapshot after acquiring the claim. No claim is an authority to overwrite a revision or ignore transaction conflicts.

### 5.2 Transaction protocol

1. Acquire a snapshot/source-root lease and a uniquely owned transaction staging directory. Record the exact inputs/read witnesses and write preconditions. Stage newly computed records, edges, payloads, and receipts there.
2. Validate canonical keys, family/schema contracts, sizes, payload hashes, and reference closure. Build candidate delta/index pages without publishing. Compute digests while streaming rather than rereading large files unnecessarily.
3. Acquire `locks/publication`. Read and validate the latest `CURRENT` and manifest. Validate the transaction against this current state, including absent/range witnesses and expected prior logical revisions.
4. Rebase a disjoint insertion onto the latest roots. Retry a conflicting read/write transaction. Equal immutable facts are idempotent; unequal contents for the same revision/derivation produce an explicit conflict. Rebuild only affected COW directory paths.
5. If all intended facts, bindings, roots, and stable receipts already exist, return the existing result without writing a delta, new manifest, or updated access timestamp.
6. Synchronize every new object file. Install immutable content-named files without overwriting existing inodes; an existing object must match its identity or be reported damaged. Synchronize all newly created/changed parent directories.
7. Write and synchronize the complete new manifest, install it under its immutable name, and synchronize `snapshots/`.
8. Write and synchronize a temporary `CURRENT` beside the destination. Atomically replace `CURRENT`, then synchronize the store directory. Only now acknowledge a durable commit. The transaction ID makes a lost acknowledgment resolvable without repeating the write.
9. Release publication/claim locks and clean this transaction's staging area. Post-commit cleanup failure is not a rolled-back commit.

This provides snapshot reads and serializable publication only for transactions that supply complete read/write preconditions, including range/absence observations. The typed adapter must not hide reads from validation. The public write protocol is `begin(base)`, `stage`, `validate`, `commit`; it is not an unconditional append API that happens to serialize renames.

Prepare candidate directory changes and synchronize staging outside the publication lock where possible. A root change requires revalidation/rebase, not a blind switch. Commit work scales with the admitted change set; a bulk import may hold publication longer than a small bake. Measure that separately rather than promising constant-time publication of a million new facts. Readers on existing snapshots continue throughout.

Use positional/range reads or scoped mappings; do not share an unguarded seek cursor between concurrent readers. Single-object installation and directory synchronization must work on the actual supported filesystem, not only in a unit-test mock. A successful rename alone is not the durability contract. [Linux file and directory sync requirements](https://man7.org/linux/man-pages/man2/fsync.2.html)

For macOS power-loss durability, verify the actual `std.fs` host contract: `sync_all()` alone must not be assumed to provide the stronger drive-flush ordering described by `F_FULLFSYNC`. Qualify the supported durability profile and test it; a missing primitive is an Incan filesystem capability, not an Oven-specific escape hatch. [Apple fsync / F_FULLFSYNC contract](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fsync.2.html)

### 5.3 What readers see and what recovery does

| Failure point | Visible state and recovery |
| --- | --- |
| Baker dies while computing/staging | Old snapshot remains; kernel releases claim; only that dead transaction's staging is recoverable |
| Objects installed, no manifest/current switch | Objects are unreachable leftovers, not committed records; later explicit cleanup can reclaim them |
| Manifest installed, `CURRENT` not switched | Old snapshot remains; new manifest is not selected merely because its number is larger |
| `CURRENT` replaced, final durability acknowledgment lost | Recover by transaction ID and validated current history; report committed, absent, or outcome-unknown, never blindly replay |
| Missing/corrupt record or index page | `StoreError` naming the exact affected object; the owner's repair policy may recompute or refetch, never use a stale fallback |
| Corrupt `CURRENT` or selected manifest | Refuse opening that state; explicit repair selects a verified committed root. Do not return generation zero |
| ENOSPC / I/O failure | Preserve last committed snapshot, report failure, leave attributable staging; do not evict unrelated roots to force a commit |

Do not inherit the foundation's global staging cleanup into a concurrent writer design. Recovery touches only transactions whose ownership is provably dead. Do not delete active claim inodes. A bounded ordinary open validates the selected root and traversed pages; a separate audit walks all retained objects/history.

This supersedes DD-0012's rule that a damaged record is a miss (DD-0013, "Damaged records"). The generic API reports damage and never turns it into absence. Recovery belongs to the record's owner. A derived compiler fact is quarantined and recomputed from complete, authoritative inputs in a new explicit transaction. A damaged frozen Loaf is refetched, or replaced by a separately baked equivalent under the proper producer contract; it is never edited in place. If repair is impossible, the compile is refused.

## 6. How Loaves Add Their Data

This is part of the first implementation contract, not a packaging follow-up. Without it, the compiler's record reads would simply relocate the cost of loading complete dependency closures.

### 6.1 A Loaf contributes a sealed record set

Define a generic IncQL-DB **sealed record set**, used by a Loaf adapter. Its transport manifest identifies:

- The identity, schema, codec, and physical format profiles required to interpret it.
- The exact logical records/revisions exported, their canonical set root, and the namespace roots they expose.
- The canonical base segments and separate payload digests carrying those records.
- Exact external record-set/payload dependencies. A package does not recursively embed another package's entire base or native closure.
- The source/bake receipt, producer identity, and verification material. Local admission provenance is separate from the exported immutable values.
- Optional native units, each with full RFC 124 identity, exact payload digest, dependency external identities, target/domain, and binding to its source publication.

The **sealed set root identifies contents; the local snapshot generation does not**. The same set can be installed into different homes without rewriting embedded local IDs. This is a generic portable import capability, not a declaration that RFC 067 already defines global logical identity. Preserve explicit origin scope and identity-profile IDs; map foreign records through explicit bindings. Never merge two independently identified records merely because their values hash equally.

Source Loaves, portable checked records, and target-bound native assets are separate capabilities. A source publication need not contain every possible generic instantiation or target unit. A compatible sealed record set can satisfy semantic reads while a missing target-specific native unit still requires a bake. Conversely, an executable native asset is not proof that every compiler inspection record is present. The import receipt states which capabilities were admitted.

Each admitted capability must have its complete required reference closure. It is valid to admit the semantic set without an optional native asset; it is not valid to mark a native unit usable while its required native dependencies or receipt-bound payloads are absent.

### 6.2 Admission, once per missing content

```text
discover candidate
    -> verify source/registry authority and exact selected profile
    -> verify transport and canonical object digests
    -> validate family codecs, identity profiles, references, and required closure
    -> find already admitted objects and equal records
    -> stage only missing contents
    -> commit set membership + namespace bindings + receipt + retention root
    -> return a snapshot/root handle, not decoded dependency models
```

The adapter enforces signature/trust policy, exact native unit matching, and allowed namespace ownership. The generic importer enforces safe object admission, schema compatibility, identity collisions, reference completeness, atomic visibility, and idempotence. Importing never executes code supplied by the package. Reject path traversal, symlink escape, inconsistent lengths, duplicate-key disagreement, unsupported required codecs, and references not covered by the declared closure.

Verify the complete downloaded objects once while streaming into private staging. A successful exact import records the verified source and object identities. Subsequent selected reads validate their directory/frame/block path, not every byte of every transitive file. Native payloads retain RFC 124's verification-at-admission and revalidation-after-external-change contract. Stat information is a change detector, never semantic identity.

Install a whole canonical base directly when it is already known or all its record identities are new. When an incoming pack partially overlaps existing records, stream out only the missing records into an admitted pack and discard the redundant transport pack after verification/publication. The portable sealed-set root is over canonical records, not their local offsets, so that normalization does not change the exported set identity.

This choice makes the tradeoff explicit: **zero-copy whole-base attachment and perfect record-level deduplication cannot both be promised for arbitrary partially overlapping packs**. Favor deduplicated home storage; charge normalization to first admission. Repeated imports use the existing set root and perform neither normalization nor new publication. Publishers avoid most overlap by shipping their own facts and referencing dependency sets, rather than vendoring the transitive closure.

If two distinct record revisions carry equal large value bytes, their envelopes remain distinct while a shared content block may be reused. If equal identities claim unequal values, refuse the conflict. Do not implement last-import-wins.

### 6.3 Import is not activation or selection

**Admission makes bytes available. A plan binding makes a particular revision applicable. A read scope activates only what is requested.** These are three different operations.

An installed Loaf must not overwrite a global `std.foo` or `pub::foo` pointer for every project. Each selected plan/snapshot binds that import namespace to the exact sealed set and profile. Two package versions, feature projections, host/target domains, and toolchains can coexist in the same home. A manifest-level directory routes the exact key directly; readers never probe each installed Loaf in order.

Imported records are frozen. A consumer can read a supplied signature, layout, body, or ABI fact. It can compute a previously absent instantiation using that frozen generic body. That new instance belongs to the consumer's exact derivation profile and is independently reusable by later consumers selecting the same inputs. There is no mutation of the dependency's base and no automatic recomputation inside an already admitted Loaf.

Example:

```text
std.collections Loaf L1
  -> namespace root N1
  -> Deque signature S1
  -> extend generic body G1
  -> native units U1 ... (only the profiles actually supplied)

project A binding -> L1 / N1
  extend[int] -> derivation(G1, int, target, compiler profile)

project B binding -> L1 / N1
  same derivation -> same admitted fact, no second bake

project A upgrades to L2 / N2
  compare namespace/name-slot roots; seed changed bindings
  changed body G2 -> new instance only where it was actually read
  project B and its pinned receipts still refer to L1
```

An interface-compatible version is not permission to substitute an ordinary function's behavior. Native reuse still requires exact RFC 124 external identity. Until the compiler supplies a narrower valid hash, RFC 124's external-identity-equals-unit-identity fallback remains in force. Storage alone cannot promise that a private body edit avoids every downstream rustc rebuild.

### 6.4 Export, bundles, registry, and removal

Export starts from explicit package/receipt roots, not the whole home. Traverse the required record/payload closure and retain external set references. Canonically compact the package-owned records. Emit a sealed record-set manifest plus the exact optional native-unit manifests; do not embed home paths, process IDs, local generations, or local access history. Sign/package through the existing publication authority.

Toolchain prefill uses the **same importer** to admit the toolchain manifest's sets and units. The installed-toolchain record is a durable retention root. Equal facts already present are shared, even when another bundle or local bake supplied them. Bundle installation is not a second store tree copied into every project.

RFC 125 uses the same identities, object validation, and receipt relationships at registry scope. It does not turn this embedded substrate into a distributed database. Signed source publications and signed registry events remain the registry's authority; indexes and asset offers are projections. Independent registry workers stage work and submit admitted units to that registry's publication boundary. Consumers verify under the pinned registry trust identity before admission. An unavailable matching asset remains a source bake, not a near-match substitution.

Removing a package/toolchain removes its explicit root or plan binding. It does not directly delete its segments or payloads. Receipts, other installed Loaves, active snapshots, explicit pins, and native execution leases can still retain them. Collection determines which objects actually become unreachable. A second import or an unchanged build adds no access-time or fresh-receipt record merely to record that the same facts were found again.

## 7. Growth, Compaction, Collection, and Views

### 7.1 Growth accounting

Report separately:

```text
retained store bytes = unique retained values/records + required indexes
                      + explicit provenance/receipts + retained history
temporary bytes      = active staging + compaction output awaiting publication
                      + failed-transaction remnants awaiting recovery
view bytes           = view metadata + modified COW extents + explicit copy fallbacks
```

An unchanged rebuild with the same selected inputs, outputs, bindings, and roots writes **zero retained bytes** and creates no new manifest. A new project receipt, changed source-position projection, an explicit audit event, or a newly retained snapshot is new information and is accounted as such. Do not promise zero writes while also inventing a timestamped receipt on every invocation.

"Each fact once" means one canonical stored value/record selected by identity, not identical copies per project, worker, Loaf, or cache directory. Live old snapshots and temporary rewrite generations may physically retain older copies until their leases end. Measure that retention separately rather than concealing it as deduplication.

There is **no database size cap, Loaf count cap, or automatic size-based eviction**. Compaction and query working-memory budgets are not capacity limits. Low disk is reported; collection is explicit and root-based. Never remove live facts to get below an arbitrary store quota.

### 7.2 Deterministic compaction

Compaction consumes a pinned snapshot/partition, streams records in canonical key order, resolves physical supersession without dropping logical history required by that view, and writes a new immutable base. Do not build a dictionary of every value in the home.

Specify the canonical packing profile: sort order including collision tie-breaks, codec/schema versions, block sizing/alignment, deterministic split rules, optional compression implementation/version, and normalized padding. Given the same retained canonical record set and profile, the ordered base objects and base-root digest must be byte-identical, regardless of insertion order, transaction grouping, local generation, import order, or machine path.

Compaction does not make two semantically similar but differently versioned/provenanced record sets identical. It also does not make manifests with different parent/transaction history byte-identical. The equality claim applies to the **canonical base**. Location-only changes preserve the logical view identity.

Build the base outside `locks/publication`. At commit, replace only the exact input runs/partition covered by the pinned compaction input; preserve deltas committed since it began. If coverage cannot be proven, retry publication/compaction. A stale caller-supplied `keep` set must never remove a concurrent commit.

Schedule compaction by measured rewrite/index/fragmentation debt per partition, not total home size. An initial trigger may use delta bytes reaching the partition's base bytes, or the existing run-count trigger as a scheduling hint. It cannot refuse storage or silently discard roots. Keep a bounded merge buffer; external sorting and paged index construction permit inputs larger than RAM. Full canonical export is a deliberate O(retained bytes) operation, never an ordinary open.

### 7.3 Roots, snapshot pins, and safe collection

Distinguish **available derived facts** from **durably retained logical state**. The physical inventory is not itself a retention root, or collection could never free anything. A general IncQL-DB application retains its current logical collections and policy-required history. The bakery retains selected receipts, installed toolchains/Loaves, explicit pins, and active operations. The generic API accepts these roots without understanding their compiler meaning.

Roots are exact record/revision or sealed-set references. Trace their strong relationships, exact read witnesses, payload references, and required schema descriptors. An edge's retention strength is explicit; an audit pointer to a pruned predecessor may preserve its identity without retaining all ancestor payloads forever. The snapshot's parent ID is provenance, not an instruction to keep the entire database since creation.

A live snapshot protects its entire promised visible view, including pages and payloads it has not opened yet. A file descriptor for the manifest alone is not sufficient. A broad inventory snapshot therefore retains more than a scoped compilation snapshot; keep it only for the request that needs it. General durable records are not cache misses and cannot be collected without their explicit retention/tombstone policy.

Use the following conservative first GC protocol; optimize it only after proving equivalence:

1. Acquire `locks/publication` for the GC operation. Bakers can keep computing and staging, but canonical object installation, root mutation, and commits wait. This is an explicit maintenance pause for writers, not an unbounded reader lock.
2. Under a short `locks/lifecycle` handshake, capture durable roots, live snapshot/operation leases, and staging-source leases. A lease owns an OS liveness lock and an immutable root descriptor; dead-PID guesses are insufficient. Release the lifecycle lock before traversing records.
3. Mark reachable objects using bounded work storage. Include leased old layouts and current logical collections. Do not follow the inventory to every object merely because the inventory lists it.
4. Reacquire the lifecycle gate and incorporate any new lease roots registered since capture. If they add unmarked roots, release the gate and mark them, then retry. Released leases may conservatively remain marked for this pass.
5. Once all registered roots are covered, publish a pruned directory/manifest and the retained-snapshot registry while holding that short gate. An unretained historical snapshot becomes explicitly expired; it cannot be newly pinned after this point. A new snapshot registration must recheck the selected root under the gate, not trust a pointer read earlier.
6. Release the lifecycle gate. Delete only unreachable objects/obsolete layouts, then release the publication lock. Newly opened snapshots see the pruned roots; earlier leased snapshots remain protected. Deletion failures retain space and are reported, not treated as success.

Existing snapshot reads never acquire the writer lock. Snapshot **registration** has a small synchronization step and can retry around collection. This is deliberately weaker than claiming every operation is wait-free; the slice's completely unregistered lock-free reader cannot coexist safely with deleting files that it may lazily open later. GC does not hold the registration gate for a whole mark traversal. If repeated new broad pins prevent convergence, report collection deferred instead of blocking readers indefinitely or deleting live data.

All object installation occurs through publication, so GC cannot race with an untracked file being installed by an importer. Active staging directories are excluded from object sweeping. Recovery cleans abandoned staging only after proving its owner dead. A crash after publishing a pruned root but before unlinking leaves reclaimable extra files, not a damaged committed view.

If an index/schema/relationship required for tracing is corrupt, stop that collection or conservatively retain the affected set. Do not infer unreachable from unreadable. On platforms that disallow deleting mapped files, delay unlinking until leases close; mapped-reader deletion behavior is not a substitute for the lease protocol.

### 7.4 `target/` is a view

Native input consumers use leased canonical payload files directly whenever their tool contract permits it. When a private path is needed, create an explicit view whose receipt pins the source payload:

| Filesystem / requirement | View strategy | Cost and guarantee |
| --- | --- | --- |
| APFS, same supported volume | File clone | New file metadata, shared unchanged extents; writes allocate changed extents |
| Linux filesystem supporting reflinks | `FICLONE` through a portable Incan filesystem capability | Same-filesystem COW sharing; detect actual support, do not infer it from "Linux" |
| ext4 or another filesystem without clone support | Direct canonical read path, or a read-only indirection supported by the tool | No duplicate payload bytes; not a writable private clone |
| Writable private file required but clone unavailable, or cross-filesystem view | Explicit copy fallback, reported as copied bytes | Correct isolation, but stored-once/COW savings do not hold for that view |

APFS supports clone-style sharing; Linux's reflink operation is conditional on the filesystem and rejects cross-filesystem cloning. Therefore portable writable COW views on every filesystem are not an honest guarantee. [Apple APFS features](https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/APFS_Guide/Features/Features.html), [Linux FICLONE contract](https://man7.org/linux/man-pages/man2/FICLONE.2const.html)

Do not hardlink canonical payloads into a path a build tool may mutate. Permissions that the same user can change are not a COW boundary. Executables/dylibs also need their platform loader/signing/relative-library layout preserved; test relocated execution rather than assuming any content-hash filename is runnable. A view is disposable presentation, never compilation authority or a second payload cache.

## 8. The Compiler's Record-by-Record Path

### 8.1 One read path, cold and warm

The compiler owns `require(record)` and the evaluator registry; the substrate owns `lookup(record)` and transactions. Reads return a value plus a witness. Flow:

```text
begin compilation(selected input snapshot, exact plan profile)
  open bounded read scope
  for each needed declaration facet in validated dependency order:
    compute its exact requested identity / derivation
    lookup typed record
    if applicable:
      borrow the fields needed; attach exact witness to current derivation
    if absent or reproducible cache record damaged:
      ask producer under claim; recheck; evaluate using witnessed reads
      stage value + witnesses + forward/reverse edges
    if new value equals old value:
      retain new witnesses, stop semantic invalidation propagation here
  commit changed values, bindings, graph fragments and output receipts together
  release read scope and all compilation-owned decoded state
```

Checking cache applicability is itself indexed. Do not recursively revalidate every transitive dependency of every read. The selected input/Loaf snapshot is fixed, source changes provide seeds, the reverse-edge index finds readers, and rank orders the affected closure. Newly demanded work follows the same producer contract. A result computed under a stale mutable input is rejected at publication or explicitly retained only under its original immutable derivation, never bound to the current plan incorrectly.

An unchanged build can skip graph activation when its selected inputs have a valid unchanged witness. This does not prove that arbitrary edited filesystem sources are unchanged without observation: source hashes, reliable change notifications with overflow recovery, or another explicit freshness contract are still required. mtimes alone cannot become the authority merely to claim O(1).

For dependencies, Loaf namespace roots are already frozen and admitted. Resolve a binding, read the requested slot/signature, and load a body only for operations that actually require it, such as generic instantiation. Do not hydrate the Loaf to discover that a single export exists.

### 8.2 Replace these loads in order

Paths below are relative to the repository root. They are integration sites, not instructions to move compiler semantics into the database.

| Step | Existing surface | Replacement and proof |
| --- | --- | --- |
| 1. Export and ABI lookup | `loaves/compiler/incan_frontend/src/loaf_index.rs`, especially `read_current`, `read_rust_abi_cache`; `loaf_index/decoded_manifest.rs` | One package/namespace descriptor and selected export/ABI records. Import/codec validation happens on admission. Show relevant record count and latency replacing B0's 8 manifests + 7 facts files |
| 2. Stop daemon retention | `loaf_index/retained.rs` process-global `ARTIFACTS` / `ABI` stores | Request-owned views and arenas; idle state does not scale with number or size of Loaves previously queried |
| 3. Body and lowering reuse | Existing executable-representation/Body IR readers and `lower_program` | One checked-body/IR/instance/plan record at a time. Reuse generic bodies; bake only missing instances; attribute lowering and rustc metadata allocations separately |
| 4. Incremental dependencies | `loaves/kernel/incan_semantics_core/src/dependencies.rs` and semantic fact stores | Preserve checked owner/requirement semantics. Store exact typed reads, both-direction indexes, name-slot absence, and paged adjacency; no graph reconstruction from source text |
| 5. Bounded inspection | `workspaces/oven/src/graph_context_index.incn`, `workspaces/contracts/src/graph.incn`, compiler codegraph emission | Persist queryable family/name/edge indexes. Answer bounded pages from the pinned view; do not parse a whole JSON graph and apply LIMIT afterwards |

The current context index already stores some record ranges, but builds in-memory node/link/ranking structures. Moving payload bytes to files alone is insufficient. Search ranking metadata, namespace membership, and adjacency access must be indexed too. The contract runner must compare streamed/canonical records or bounded results without retaining several whole graph documents simultaneously.

### 8.3 Reuse compiler authority, not its current containers

The checked-export replay implementation already binds an exact checked identity to its shape and refuses ambiguous reconstruction. Preserve that invariant when splitting the manifest. Do not zip independently sorted export names and identities.

The existing semantic digest implementation already distinguishes structural meaning from positional information. Reuse its versioned semantics as the comparison authority; storage checksums separately protect physical bytes. Source/diagnostic projections keep location changes visible even when a semantic value is reused.

The checked-artifact codec can seed per-family serialization/validation, but a codec for an aggregate artifact is not automatically a random-access record codec. The implementation must inventory its actual entry points and expose one-family/one-revision encoding with golden round trips. The inspected code concretely provides `CheckedExportReplay`, semantic fact/dependency models, and semantic digests; this note does not claim that a complete fine-grained persisted read-witness API already exists.

Publish newly computed read witnesses and outputs together, following RFC 060's result-before-state rule. Never advance a "checked" or "applied" marker when only the input observation was stored. Keep full-result/cold compilation available as the oracle.

All new storage and adapter semantics are authored in Incan. Existing Rust implementation sites are integration evidence, not permission to write a replacement Rust subsystem. Where an existing compiler boundary cannot consume the proposed Incan views/records, identify and track that interop/ownership boundary before implementation. No Cargo-backed workaround is part of this plan.

### 8.4 Baseline targets and limits of the evidence

| Evidence | What record reads should improve | What it cannot honestly claim yet |
| --- | --- | --- |
| Std manifests/facts about 35 ms; checking closure adds about 7-16 MB | Selected record reads instead of aggregate decoding | Eliminating these loads does not remove the majority of tiny-compile memory |
| Tiny cold-path driver 250-253 MB lifetime max; about 183 MB by rustc's first pass | Bound activated IR/instance state and avoid duplicate retained representations | About 140 MB attributed jointly to lowering plus rustc session/rlib metadata is not yet split; it is not all database-removable |
| Lowering about 80 ms on the traced tiny case | Reuse applicable Body IR/plan records, measure actual activated functions | This is not a promised 80 ms saving; some lowering remains necessary |
| Idle worker about 2 GiB, runner 10.4 GiB, MCP 3.8 GiB in planning measurements | No idle closure retention; bounded graph queries and streamed consumers | These are plan-reported problem sizes, not all measured by the same baseline procedure |
| 47 ms unchanged rerun; 32-40 ms preparation; first binary launch 145-200 ms | Zero new storage facts on unchanged work, no per-atom process cost | Socket, startup and launch work is separate from storage |
| Census 322 s unprofiled; instrumented 344.5 / 353.9 s | End-to-end comparison using the same fixture/home/job conditions | The 843 s run split batches because instrumentation polluted stderr; it is not the performance baseline |

The baseline's 1,678 MB concurrent estimate sums lifetime maxima of overlapping processes and is an upper bound, not measured simultaneous physical memory. The 192.1 GB sum of maxima is not a resident working set. Use synchronized measurements for the new sharing claim. Timing subspans from different/traced runs also cannot simply be added as if they were one non-overlapping critical path.

## 9. Proof and Acceptance

Do not gate this on attractive timing alone. Each property needs an identity/correctness test and an efficiency measurement.

| Property | Required scenario | Passing evidence |
| --- | --- | --- |
| Computed once | Several processes request the same exact missing derivation; then a second checkout asks | One successful producer execution without injected failures; all consumers receive the same verified result. Different profiles do not alias |
| Crash qualification | Kill the claimant before and after durable publication | A replacement may repeat unfinished work; it never publishes a conflicting value or redoes an already committed result after recheck. Do not claim distributed/exactly-once computation across crashes |
| Stored once | Two Loaves share dependency sets/units; partial-overlap import; local bake then remote import | One selected canonical object per identity, no duplicate per-project native closure; import normalization and transient bytes reported |
| Frozen Loaves | Read L1, import L2, instantiate a generic, keep another L1 consumer live | Frozen records unchanged; bindings are snapshot-specific; only missing applicable instances compute |
| Correct identity | Relocation, source-position-only edit, feature/target/toolchain change, injected 128-bit collision | Reuse only where the compiler/unit contract allows; full-key checks prevent collision aliasing |
| Correct edges | Missing name becomes present, signature/default changes, changed read set with unchanged output | All and only justified readers re-evaluate; changed witnesses survive early cutoff; cold oracle agrees |
| No stale fallback | Damage latest record, directory separator, negative-lookup page, frame bytes, manifest, or payload | Explicit error; no older value/false absence accepted; compiler repairs only from authoritative inputs |
| Atomic publication | Kill/fail writes at every protocol boundary; ENOSPC; two disjoint and two conflicting writers | Readers see complete old or complete new state; output and witnesses agree; ambiguous acknowledgment resolved by transaction ID |
| Shared memory | 1/2/4/8 readers of equal and disjoint page sets | Timestamp-aligned shared/private/PSS or footprint evidence; copies/decoded arenas accounted separately |
| Nothing held idle | Thousands of varied requests with explicit scope teardown | Live storage-owned decoded bytes/views return to zero; idle footprint has no growth slope with prior graph/Loaf size; allocator/runtime residuals measured |
| Bounded activation | Fix K requested facts; increase unrelated store size by 10x and 100x | Returned/decoded records remain O(K); lookup pages grow only with directory height, not all segments or installed Loaves |
| No unchanged growth | Repeated unchanged build/import/reopen after warm-up | Identical `CURRENT`, inventory digest, retained file/byte counts; no time-based receipt or lock-file proliferation |
| Deterministic base | Same records inserted in different orders, batch sizes, homes and Loaf-import orders | Canonical base bytes/digests equal under the same format/codec profile; local manifests need not match |
| Safe collection | Hold old snapshot and payload lease, commit new facts, compact, collect, kill a baker, release roots | Only unreachable objects disappear; late lazy reads remain valid; no stale `keep` drops a new commit |
| View isolation | Modify/remove a writable target view on APFS and a non-clone filesystem | Canonical payload unchanged; clone vs copy behavior and allocated bytes explicitly reported |
| Bounded graph answers | Ask for one declaration/edge page against the large codegraph fixture | Bytes read/decoded/returned scale with query bounds and index work, not complete graph size; cursors remain snapshot-bound |
| Record-read semantic equality | Cold vs incremental for body/signature/layout/default/name/Loaf changes and repeated reverts | Equal canonical checked records, diagnostics/source projections, selected plans, and native outputs under the same reproducibility profile |

Verify power-loss/durable-order behavior on supported platforms in addition to process-kill tests; a process kill alone does not test a drive cache. Distinguish an injected filesystem model, real-process fault tests, and actual filesystem durability evidence in the report.

For all timings record compiler/toolchain revision, selected profiles, CPU/OS/filesystem, store size, read-set size, process count, page size, cache state, and instrumentation overhead. A fresh process is not necessarily a cold page cache. Use a dedicated cold-cache experiment/VM where available; otherwise label the condition accurately. Do not flush the user's live machine cache to manufacture a cold result.

## 10. Four Implementation Stages

These are boundaries of usable capability, not dates. DD-0013 orders them with the engine stages E1 to E7.

### S1. Durable generic record store

Keep the segment/payload shape. Add full-key collision checks, explicit absence/error/tombstone outcomes, frame integrity, validated transactions, durable manifest publication, and source/snapshot leases. Introduce the error and lifetime API now even if reads initially return bounded owned bytes. Disable unsafe collection until its protocol exists. No lifetime-exclusive writer session, silently empty corrupt store, or stale-value fallback passes this boundary.

**Exit:** concurrent disjoint/conflicting writes and crash protocol pass; foundation snapshot/payload semantics preserved; typed caller can distinguish absence from corruption; no authored Rust store logic. Current `map_read` is usable but not called zero-copy.

### S2. Indexed activation and sealed-set admission

This stage establishes toolchain prefill's storage path and unblocks keyed export and ABI lookup.

Implement the manifest-level COW directory, paged adjacency, mapping-backed typed views, bounded read scopes, deterministic streaming compaction, root-aware GC, and portable sealed-set import/export. First integrate selected Loaf exports/ABI records and toolchain prefill. Legacy aggregate codecs may serve only as explicit import converters, not the steady-state consumer path.

**Exit:** two independent Loaves and a toolchain bundle share contents in one home; repeat import is a no-op; a selected record read is independent of unrelated segments; GC respects old snapshots; byte-identical base and copied-byte counters verified. No B4 performance claim while it still hydrates whole manifests.

### S3. Compiler record reads and bounded graph consumers

S3 builds against the S1 API and the S2 index and lifetime behavior.

Land export and ABI lookup and the end of daemon retention (steps 1 and 2 of §8.2) before body and lowering integration. Then persist checked body/IR/instance/plan facts and witnessed dependencies, seed invalidation from exact input/Loaf changes, preserve early-cutoff witnesses, and replace whole-codegraph materialization in graph/MCP/contract consumers. Keep these substeps independently measurable.

**Exit:** cold/incremental semantic parity; one-sweep preconditions checked; unchanged work adds no facts; actual per-compile read set and idle-memory release demonstrated. Separately report remaining rustc metadata/native-unit limitations.

### S4. Distribution and efficiency qualification

Exercise the same admitted sets from local bake, toolchain bundle and registry. Verify trust/profile refusals, upgrades/removal, multiple platforms/filesystems, large stores, import normalization, compaction under writers, and full B0-comparable workloads. No Cargo path is introduced to obtain these proofs.

**Exit:** all applicable proof rows above have artifacts; unsupported filesystem/durability capabilities and remaining socket, startup and launch costs are named. Efficiency claims refer to those artifacts, not the design diagram or the existence of mmap.

### Alignment of the existing segment store

Preserve the existing `map_read` and native-consumer integration. Align the module around explicit `StoreError` versus absence, full identities, transaction validation, roots/leases, and the generic import boundary before more consumers depend on `Option[bytes]` plus implicit latest-generation lookup. Replace `compact(keep)` with content-preserving compaction and separate policy-driven collection. Keep all new semantics in Incan. Do not retain a permanent Oven-only storage fork.

## 11. References and open platform boundaries

### References

- [DD-0013: The Incan build engine](../../design_records/0013_the_incan_build_engine.md) and the [build engine design](build_engine_design.md): the engine this store serves.
- [DD-0012: Incremental compilation over a persistent semantic store](../../design_records/0012_incremental_compilation_over_a_semantic_store.md): per-declaration facets, name slots, the ranked graph, frozen Loaves and deterministic compaction.
- RFC 124 (Oven store unit identity and cross-plan artifact sharing) and RFC 125 (incan.pub Loaf registry and baked asset distribution).
- IncQL-DB: the whitepaper, IncQL RFC 067 (typed semantic memory store), IncQL RFC 060 (incremental transformation and temporal history), and the IncQL-DB foundation's snapshot and payload contract.
- The existing segment store in Oven and the `std.fs` mapping module, both under development on the bakery branch.

### Capabilities to verify or track before claiming completion

- Lifetime-safe mapped views/scalar access: the new bounded-copy `MappedFile.read()` does not by itself satisfy S2.
- Durable host flush and directory-publication ordering, including the supported macOS power-loss contract.
- Portable clone capability and truthful fallback reporting; no promise of reflinks on ordinary ext4 or across devices.
- Compiler-side consumption of Incan-authored record views without a new Rust semantic layer.
- Fine-grained family codecs and stable canonical-key projection, including generic/overload and source-location fixtures.
- Exact provenance of the 6-16 ms memory-activation figure; reproduce it before comparing it to compiler record lookup.

These are concrete acceptance inputs. They are not permission to work around an untracked compiler defect, and they do not block implementing the compatible S1 boundaries.
