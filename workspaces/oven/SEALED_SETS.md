# Sealed Record Sets

`src/sealed_set.incn` is the initial binary transport and admission layer over the generic S1 segment store. It is
implemented in Incan. It does not change the segment format, select a project namespace, or decide whether a native
unit is eligible to execute. There is no CLI integration yet.

## Authority and Identity

A set contains explicit record and index members, exact references, payload descriptors, exported namespace roots,
a producer identity, and an exact reference to its publication receipt. Record keys remain complete logical keys;
they are not replaced by truncated digests or physical locations. Record values and native file payloads carry full
SHA-256 digests and byte lengths. The set root is the full SHA-256 of its canonical manifest.

The caller obtains the expected root from its verified publication authority. Hash verification proves content
integrity, not publisher authenticity, semantic correctness, native ABI compatibility, or permission to execute.
The producer adapter owns those checks. The initial profile accepts only `opaque-bytes/1` values, with an explicit
`record` or `index` kind. It does not claim to validate compiler record families or index algorithms.

An external reference names an exact set root, full record key, and value digest. Admission requires that exact set's
membership record, not merely a matching key somewhere in the destination. External payloads similarly require
membership and length in the named set. Dependencies must already be admitted; import does not fetch them.

## API

| Function | Inputs | Result | Responsibility |
| --- | --- | --- | --- |
| `export_set` | Snapshot, manifest request, new directory | `SealedSet` | Export only explicit members and local payloads; fill value digests and lengths from the snapshot. |
| `verify_set` | Transport directory, expected root | `SealedSet` | Verify canonical framing, local reference closure, record bytes, and shipped payloads without modifying a store. |
| `admit` | Store root, transport directory, expected root | `Admission` | Verify again, resolve external membership, and atomically publish missing retained facts and the installed-set root. |
| `release` | Store root, set root | Snapshot | Remove only the installed-set retention root; never delete shared facts or payloads. |

`Admission.records_added` counts new record/index values, not internal declarations or membership records.
`payloads_added` counts installed payload files. `changed` means a new snapshot was published. Repairing an absent
local payload from a valid transport can install bytes without changing the snapshot; a present damaged payload
is refused, not silently replaced. A repeat import with all objects intact leaves the snapshot and inventory unchanged.

## Binary Format

```text
set-directory/
  set.bin
  payloads/
    <full lowercase sha256>
```

The file contains, in order:

1. Eight magic/version bytes: `INCSET\x00\x01`.
2. The canonical manifest's byte length, as a little-endian unsigned 64-bit integer.
3. The canonical manifest.
4. Record values in ascending full-key order, with their exact lengths supplied by the manifest.

There is no JSON, native-endian field, machine address, home path, generation number, or physical offset in the
manifest. A string is a u64 byte length followed by strict UTF-8; a collection is a u64 count followed by its members.
Every length must also fit the signed host I/O API. Decoding bounds lengths and counts against the enclosing section
before reading. Trailing data and noncanonical encodings are refused.

The manifest fields are ordered as follows:

1. Identity profile `full-record-key/1`, codec profile `incql.sealed-binary/1`, and producer string.
2. Receipt reference: key, digest, external-set root (empty for a local reference).
3. Sorted dependency roots.
4. Namespace entries sorted by name: name followed by an exact reference.
5. Payload entries sorted by digest: digest, u64 length, external-set root.
6. Records sorted by full key: key, kind, schema, digest, u64 length, exact references, payload digests.

Payload digests are sorted. Exact references are sorted by the string
`decimal-length-of-set-root + ":" + set-root + key`; the set-root length is zero or 64. Each reference's three fields
are independently length-framed on the wire. Duplicate keys, namespace names, dependencies, payload descriptors,
and reference identities are rejected, including equal duplicates. No Unicode normalization or case folding changes
identity. The decoder regenerates the canonical manifest and compares its bytes before accepting its root.

Local payloads are ordinary files named by their full digest; external payloads are not shipped. Transport paths
come only from fixed names and validated digests. Symlinks at the transport root, manifest, payload directory, or
payload file are refused. The transport must remain private and immutable during verification/admission; these
path checks are not a descriptor-relative defense against an adversary concurrently replacing path components.

Export writes and syncs payloads first, then writes and syncs `set.pending`, publishes `set.bin` last, and syncs the
directory. An existing destination is never overwritten. Failed exports can leave an incomplete directory; its
contents are not a verified set. The parent directory's lifetime is the caller's responsibility.

## Admission and Retention

Admission uses S1's transaction API and absent-or-equal preconditions. An unequal value or immutable descriptor
under the same identity is a conflict. Multiple Loaves sharing complete record identities and payload digests retain
those objects once. Each set still has its own small manifest, membership, and retention facts.

Internal keys use the reserved `incql.sealed/1/` prefix:

| Suffix | Value |
| --- | --- |
| `set/<root>` | Canonical manifest, without another copy of record values. |
| `record/<full-key>` | Immutable record descriptor, including exact edges and payload references. |
| `member/<root>/<full-key>` | Value digest supplied by this set. |
| `payload/<root>/<digest>` | Payload length as u64. |
| `retained/<root>` | Explicit installed-set root. |

The global record descriptor binds each reference's full key and value digest, but not its supplying set. Shipping
the same target locally in one Loaf and by an exact dependency in another must not change record identity. Transport
manifests still bind those supplying sets and admission checks each membership. Duplicate identical logical targets
collapse in the global descriptor; conflicting digests for one immutable target are refused.

Verification precedes publication. Payload files are installed and reverified before the transaction commits. A
failure can leave unreachable content-addressed payloads, but cannot make a partial set visible in a snapshot.
Concurrent admissions from this module share an S1 claim; ordinary bakers still use S1's publication lock and
preconditions. Imported immutable identities and reserved keys must not be repurposed by other writers.

S1 currently stages equal values as commit guards and removes them before installing the retained delta. Thus
only missing facts enter retained segments, but transient staging is not proportional to missing facts alone.
External-set and exact referenced values are guarded too. S1 lacks a read-only precondition API; an absent guard can
restore its exact value. This is not a promise of serializability against arbitrary writers deleting immutable facts.

`release` removes reachability from the explicit installed-set root only. Dependency edges remain in immutable set
declarations, so another retained set can still reach shared contents. This module does not implement collection,
snapshot leases for collection, or physical deletion. S1 currently performs no garbage collection.

## Cost and Stage Boundaries

Verification reads every shipped record and payload. Repeated admission still verifies content and destination
payloads; it does not infer integrity from a marker. Export and verification hold one record value at a time, plus
the manifest. Admission accumulates an S1 transaction and can hold the selected closure's values and guards in
memory. There is no bounded-memory admission claim for arbitrarily large sets.

S2 owns streaming staging, read-only transaction guards, compact indexed lookup, snapshot-safe collection, and
accelerated revalidation without weakening identity. Producer adapters own compiler-specific schemas, publication
trust, native compatibility, and wiring Loaf/toolchain packaging to this API. The B4 record-read path is separate.

The module's test file covers canonical export, overlapping sets, repeat imports, conflicts, exact external
membership, payload integrity, index members, UTF-8 identities, malformed framing, symlinks, and root release.
Runtime acceptance requires running those tests on a compatible native generation; a type-check alone is not
evidence of crash safety, bounded memory, or concurrent publication performance. S2 needs process-level crash and
concurrent-import tests at its final storage boundary.
