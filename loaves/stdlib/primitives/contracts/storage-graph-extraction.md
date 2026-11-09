## Storage and graphs are part of the extraction audit

Include RFC 165 (std.db) and RFC 166 (indexed std.graph) when deciding what should move out of the compiler and Oven. They are consumers of the primitive foundation, and potential owners of general mechanisms currently implemented for toolchain use. They do not belong wholesale inside std.primitives.

The existing RFC 165 layering already calls for one generic platform storage substrate beneath std.db. RFC 166 requires a storage-independent graph reader: std.db supplies an adapter; std.graph must not import std.db.

| Layer | Owns |
| --- | --- |
| std.primitives and existing platform facilities | Bytes, mappings, file coordination, durability mechanisms, hashing and resource ownership |
| Generic platform store | Physical indexes and records, publication, retained snapshots and storage lifecycle mechanics |
| std.db | Typed families/records, schema and codec contracts, transactions, typed indexes, projected batch access and read evidence |
| std.graph | Scoped graph reading, forward/reverse adjacency and bounded graph algorithms, usable without a database |
| Compiler and Oven | Meaning of compiler records and dependencies, semantic validity, producer order, scheduling, execution claims and reuse decisions |
| IncQL | Declarative query planning and execution over these capabilities |

### What already exists

Oven's store_index.incn implements immutable copy-on-write B+tree pages. segment_store.incn implements snapshot roots, retained mapped values, transactional publication, virtual views, leases and compaction. These are concrete reuse candidates, not evidence that the complete RFC 165 typed database already exists.

The same store module also contains execution-oriented coordination such as claims and pinned publication. Audit those individually: general locking and retention can be reused, while deciding that a computation owns a bake or is safe to reuse remains an engine responsibility. Do not bless every current store API as a public database API merely because it is already implemented.

Today's std.graph implements RFC 047's value-owned graph containers. Its successor/predecessor methods scan graph storage. RFC 166's indexed, pageable protocol must provide a different access path; wrapping those eager scans in a bounded return type would not meet its contract.

Compiler record_service.incn declares compiler stages, rank certificates and prepared reads. These are semantic/scheduling contracts, not generic graph or database models. Generic read evidence may support them, but must not replace their compiler-defined meaning.

### A concrete combined test drive

Use a project where module A imports module B.

1. Store the modules and their checked signatures as typed records in one snapshot.
2. Store the import relationship with explicit identity and provenance.
3. Read the modules depending on B through indexed reverse adjacency, without loading the whole project graph.
4. Edit only B's implementation body and publish a successor snapshot.
5. Let the compiler decide which answers depend on B's body and which depend only on its unchanged signature. Storage reports observations/changes; it does not infer language invalidation rules.
6. Keep an existing reader on the old snapshot consistent while a new reader observes the successor.
7. Request only selected fields in a batch, proving that a typed model API does not force full model decoding for every record.

Also test a previously empty dependency result followed by insertion of a relevant edge. Negative reads and empty adjacency are observations that can become stale. A corruption error must remain an error, not become “no dependent modules.”

This scenario exercises both extraction and consumer behavior: snapshot leases and borrowed batches for RFC 164, primitive resource boundaries for RFC 168, typed storage for RFC 165 and indexed graph access for RFC 166. No new executable API spellings are asserted by this narrative.

### Integration consequence

Track storage and graph reuse in the extraction inventory now. The existing primitive/store migration must preserve formats, identities, snapshot lifetimes and consumer-neutral boundaries so that std.db can reuse it.

Full RFC 165/166 implementations are now explicitly included in the [expanded integration plan](https://chatgpt.com/space/page_a3c2a8e51e588191b87600e0141a5613), slices 17–25, alongside compiler/runtime extraction in 11A, 11B and 13A. The ownership/primitives foundation retains its earlier qualification checkpoint. Unresolved draft contracts remain prerequisites for their dependent slices; scope inclusion does not fabricate design approval. Shared storage and graph mechanisms must serve compiler and application consumers without competing authoritative copies.

Evidence: workspaces/oven/src/store_index.incn, workspaces/oven/src/segment_store.incn, loaves/stdlib/data/src/graph.incn and loaves/compiler/internal_models/src/record_service.incn in the integration baseline; RFC 165 and RFC 166 in the locally available shared RFC branch.
