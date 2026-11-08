# Crumb binary representation

Status: required invariants and encoding worklist. No byte-level codec profile is assigned by this foundation. In particular, crumb1 names value/hash semantics; it is not a complete binary layout.

A standalone crumb carries or is explicitly bound to enough admitted metadata to identify its nominal type, written shape and codec profile. Values sharing one written shape may share the description. A selected portion has an explicit boundary. A reader can skip unrelated values from their types and lengths; it must not instantiate a complete model to project a field.

Shared descriptions list field identities once in ascending byte order. Values are matched through that description, not display names or positional source declarations. A retained view keeps its backing alive. Scalar-inline, row-oriented and projected batch arrangements may be separate codec profiles over the same logical values. None acquires transaction or persistence guarantees just by being a Crumb.

## Sealed sets

A finite set of crumbs may be sealed. Its envelope ends with the number of values it holds and a hash over their value contributions in order, under its value profile. A reader of a sealed set checks both before it hands out any value, so a set cut short or spliced from two sets is refused instead of reading as a smaller set. A seal detects damage, not authorship: anyone can recompute it, and proving who wrote a set takes a signature over the seal. A stream that may yield more values later is not sealed until it ends. COMIC text is never sealed.

## Encoding decisions required before the first binary profile

| Item | Required specification and vectors |
| --- | --- |
| Envelope | Profile IDs, lengths, boundaries, unknown-profile refusal and record-set/stream framing. |
| Seal | Where the count and hash sit, the hash's preimage over value contributions, and refusal of an unsealed set where a caller requires a seal. |
| Scalars | Tags, byte order, integer widths/lengths, float normalization, decimals and exact strings/bytes. |
| Secret types | A withheld marker with no contents. No codec profile may write secret contents. |
| Description sharing | Dictionary scope, type/member identity encoding, recursive references and invalid-description refusal. |
| Collections | Counts, optional presence, enum/union selection, map/set order and duplicate handling. |
| Projection | Field/column ranges, empty values, range validation and retained backing. |
| Column encodings | For a set arranged by column: each column's encoding, named in its header and chosen per column among frame-of-reference bit-packing at an exact width, zigzag deltas, runs, and sorted dictionaries whose keys share prefixes and whose codes are bit-packed or run-coded. Whether a column or block keeps positional access, and the block size of its index when it does not. |
| Shape references | A crumb may name its description instead of carrying it, so crumbling a shape the reader already has costs no header. The reference binds the type's nominal identity and its description digest (see [hashes](hashes.md)), so it names exactly one description. A model hash is not a reference: `crm.Customer` and `billing.Customer` with the same fields share one. A store may index descriptions by model hash to find candidates, but a reader admits a value only under the description its reference names. The encoding of the reference, and the refusal when the reader doesn't hold that description or its digest doesn't match. How a reader obtains descriptions belongs to the store, such as CrumbDB. |
| Canonicality | Shortest numbers/lengths, unique descriptions, exact consumption and original-shape re-encoding. |
| Limits | Checks before allocation, shared nested budgets, decompression accounting and cycle refusal. |

No hidden fallback tries an older encoding after a decode failure. Unsupported and malformed are different failures. A framed next record is not rejected as trailing bytes; bytes left inside the selected record are.

Before admitting a codec profile, add literal byte vectors with independent derivations, truncated prefixes, oversized counts/lengths, duplicate and unsorted identities, unknown variants/profiles, cyclic containment, excess nesting and trailing-byte cases. Verify original-shape decode/re-encode byte equality. Adaptation to another shape has different bytes and is tested separately.

Compression and store deduplication are optional layers with their own limits and ownership. They must not change logical hashes or require a database to read a standalone record.
