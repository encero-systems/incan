# Crumb row profile 1

Status: experimental codec `crumb-row/1`, numeric codec ID 1, with value profile `crumb1`, numeric value-profile ID 1. This is the first exact row layout, not a column codec or a performance-parity claim. The [hash rules](hashes.md) own value, shape and description digests. Changing these byte interpretations requires another codec ID.

## Envelope and stream boundaries

An envelope is these fields, concatenated without padding:

```text
43 52 4d 42 | L(codec_id) | L(value_profile_id) | flags | L(payload_bytes) | payload
 C  R  M  B   01 for row/1   01 for crumb1
```

`L` is the shortest unsigned base-128 little-endian length/count, at most 64 bits. Its continuation rule is the U rule in hashes.md. No overlong form, overflow or reserved bit is accepted. A host that cannot represent a valid length refuses it as a limit failure rather than truncating it. `B(bytes)` means `L(length) || bytes`.

Codec and value-profile IDs use L from the outset; they are not limited to one octet. Flags occupy exactly one octet. The only defined flag is bit 0, sealed. Every other flag bit is zero. Unknown codec/value profile IDs are unsupported, not a request to try a different decoder. The selected payload must be consumed exactly. Data after it is another envelope only when the caller selected a stream; it is trailing data when the caller requested one envelope.

The payload contains a description dictionary, a record count, that many records, then the seal if flagged:

```text
L(description_count) | descriptions...
L(record_count) | (L(description_index) | B(row))...
[ L(record_count) | 32-byte seal ]
```

Dictionary indexes start at zero. Root descriptions may be records or enums. For a root enum, row means the enum encoding below. A finite RecordSet is one envelope. A RecordStream can contain consecutive envelopes and reports its own transport completion separately; a paused transport is not an empty or completed stream.

## Complete descriptions and references

Dictionary entries sort by their complete closed nominal selector K, without duplicates. K contains the six-byte declaration identity, explicit argument count and framed recursive argument spellings defined in hashes.md. This first profile binds one written revision per K per envelope. Box[i8] and Box[string] have different K values and may coexist, including inside one containing record. A caller needing different revisions of the same closed type uses separate envelopes; it must not silently select one dictionary entry by name. Cyclic type references are allowed, but all named dependencies must be admitted before values are exposed. A type reference in a field uses its complete N selector, not a display path or dictionary slot number.

Each dictionary entry starts with one mode byte:

- `00`: inline, followed by `B(inline_description)`.
- `01`: reference, followed by B(K) and the 32-byte complete-description digest under crumb1's description domain.
- Any other mode is unsupported.

The reference contains no model hash. Resolve it only against caller-supplied admitted descriptions or an explicitly invoked store service. The codec itself never fetches a package or contacts a network. A missing description is `missing_description`; a supplied description with another closed selector or a mismatched complete-description digest is `description_mismatch`. No approximate model-hash match, alternate revision or guessed shape is a fallback.

### Inline description layout

Complete descriptions deliberately reuse crumb1's versioned D metadata encoding inside codec 1, but their wire member lists are in identity order. This is an explicit reuse of a hash-side metadata encoding by the codec; it does not make arbitrary physical row bytes a hash preimage. This is deliberately a different arrangement from the logical description's declaration-order lists; the description digest still uses D of the reconstructed complete logical description.

```text
B(D(header)) | members
```

For a record, header is the complete description map with only `fields` removed. For an enum, only `variants` is removed. Preserve every other header property. The core header includes kind, nominal (K as binary), path and properties. Unknown language kinds require an admitted profile mapping them to record or enum; this first profile's corpus uses core Record and Enum.

A record's members are:

```text
L(field_count) | (L(declaration_position) | B(D(complete_field)))...
```

Each complete field includes id, name, type, canonical, absent and properties. Fields occur once, in ascending six-byte id order. Positions must be a permutation of 0 through field_count minus one. Rebuild the logical declaration-order field list from those positions before calculating D. This preserves names, declaration order and properties without repeating field identities beside each record value.

An enum's members use the same rule for variants: count, then position and a framed variant. Each variant frame is `B(D(variant_header_without_fields)) | field_members`. Its fields follow the record member rule. Variant identities are unique and ascending; positions reconstruct declaration order. Empty payloads still have an empty field list.

D metadata has unique map keys in UTF-8 order, exact lengths and shortest numbers. Inline descriptions must be structurally valid: absent=none requires an outer option, absent=empty requires a collection, member identities are six bytes, K is well-framed and closed, and nominal selectors resolve to one admitted dictionary entry. Unknown types or unsupported property interpretations are refused. A description does not confer authority to execute anything.

## Row storage and projection

A record row has:

```text
presence_bitmap | offset_width | offsets[0..present_field_count] | concatenated_field_cells
```

Take fields in identity order. Only fields with an admitted absent policy have presence bits. Pack those bits low to high within each octet; the first eligible field is bit 0. The bitmap length is ceil(eligible_fields/8), and unused high bits must be zero. Required fields have no bit and always have a cell. Zero means the declared absent value and no cell; one means a cell follows in field order.

The offset_width is one octet, exactly 1, 2, 4 or 8, selecting that many little-endian octets per unsigned offset. It is the smallest width able to represent the final offset, including zero. Every offset uses that fixed width and is relative to the start of concatenated field cells. There are present_field_count plus one offsets. The first is zero, offsets never decrease, and the last equals the remaining row byte length. Equal offsets are allowed only when that field's typed cell legitimately has zero bytes. Validate the whole range table and reachable typed cells before exposing a borrowed view. No offset can point into metadata, another row or beyond its own frame.

A reader locates a projected field through the admitted shape, bitmap and offset table without constructing other field values or a model. The shape precomputes each field's required-field rank and eligible-bit position. A bitmap rank/popcount determines how many eligible fields before it are present; an optional rank index can bound that cost for wide rows. After that, the two offsets have directly computed addresses: earlier offsets do not need to be decoded. This is constant-time offset lookup, not a claim that an unindexed arbitrary-width bitmap has constant-time rank. Validation may inspect framing and skipped cell bytes; projection is not permission to trust unchecked offsets. Retained views keep their backing alive through the binding's ownership contract.

For an absent-policy optional field, the presence bit replaces only its outer option tag. A present cell contains the inner type's encoding. Thus a missing `option<option<i64>>` field has a zero bit; Some(None) has a one bit and an inner `00`; a required optional field still carries its own option tag. A field with an empty-collection absent policy must not set its bit and encode an empty collection: that is a noncanonical redundant representation. Required empty collections remain encoded.

An empty record row is `01 00`, width one and its single zero offset. Absent fields remain typed absent values, not decode failures. Decoding restores final stored state; the caller's reconstruction contract then enforces final validation without rerunning input normalization.

## Typed cells

A cell's type is in its description; no scalar tag is repeated for every value. Type expressions use N's canonical scalar/container spellings and `record@ID` / `enum@ID` nominal selectors. The `reference` spelling carries an inert target-type identity and target identity; any target constraint belongs to the admitted binding description. Profiles must provide a checked mapping for source-language type spellings before using this codec.

| Type | Cell bytes |
| --- | --- |
| boolean | `00` or `01` |
| i8/i16/i32/i64/i128 | Exactly 1/2/4/8/16 little-endian two's-complement octets |
| u8/u16/u32/u64/u128 | Exactly 1/2/4/8/16 little-endian unsigned octets |
| fp32/fp64 | Exactly 4/8 little-endian IEEE octets; positive quiet NaN only, signed zero retained |
| decimal<P,S> | Shortest Z(coefficient) at the declared scale S; check precision before returning a value |
| string | B(valid UTF-8), with no normalization |
| binary | B(bytes) |
| secret_string/secret_bytes | One `00` withheld marker; any other byte or contents are refused |
| option<T> | `00` for outer absence, or `01 || B(cell<T>)` for presence |
| list<T>/set<T> | L(count), a width byte, count+1 fixed-width offsets, then the element cells |
| tuple<T...> | The same indexed sequence, with count exactly equal to the declared arity |
| map<K,V> | L(entry_count), a width byte, 2*entry_count+1 fixed-width offsets, then alternating key and value cells |
| nominal record | Its record row |
| nominal enum | L(variant index in identity order), then its payload row |
| union | L(member index in the written description's admitted N ordering), then its selected member's cell |
| reference | B(target-type identity bytes) followed by B(target identity bytes); check any admitted binding constraint without following the reference |

Collection offsets obey the row rules and are relative to the collection's cell-data start. An empty sequence is `00 01 00` (count zero, width one, one zero offset). Maps and sets must be ordered and duplicate-free under crumb1 object comparison; do not invoke application equality. A union index uses the admitted N ordering, including closed K selector order, not source declaration order. Slots are relative to the written description bound in this envelope; adding a member in a later description does not reinterpret an old slot. Unknown enum variants or union indexes are malformed, not absent.

No scalar cell has unconsumed bytes. A zero-length string cell is invalid: the empty string needs its length byte `00`. A finite recursive value consumes nested frames; cyclic containment, an exceeded shared depth budget or an invalid nested frame is refused. Typed references may form cycles without being traversed.

The required property is original-shape decode/re-encode equality with the same dictionary mode choices, metadata and envelope boundaries. Adapted values, changing inline to referenced descriptions, or regrouping records have different bytes. Optional compression belongs to another profile or an outer bounded layer, not an unannounced decoder heuristic.

## Seals and safe exposure

A sealed envelope ends in L(record_count) and SHA-256 of the ordered seal preimage in hashes.md. The trailing count must equal the payload's leading count; the repetition is an explicit truncation/completion check, not a second source of cardinality. Derive the seal from decoded logical object contributions, never from these physical row bytes. Integer width, field offsets and description compression do not become accidental value semantics.

Before yielding any value from a sealed envelope, validate the dictionary, every selected row, counts, all framing and the seal. An implementation can perform a bounded first pass, retain the admitted backing, or spool it; it need not materialize all decoded models in memory. A wrong seal, missing trailer or count mismatch yields no records. Where a caller requires sealing, an unsealed envelope is refused even if all rows parse.

In an unsealed stream, completed admitted records may already have been yielded when a later record fails. Report that failure and its coverage; never call it ordinary end-of-stream. A finite envelope ending halfway through a length, cell or trailer is truncated. A complete next envelope is not trailing garbage under the stream API.

A seal detects altered value contributions, not authorship or every change of physical representation. Description identity, nominal admission, validation and external authentication remain their own checks.

## Bounds and refusal order

Check magic, profile IDs, flags and outer frame before dictionary parsing. Check description counts against available bytes and caller limits before reserving storage. Admit descriptions and all references before rows. Check count arithmetic, bitmap length, offset-table length and every offset before indexing. Then validate cells, completion and any seal. Shared byte, depth, digit, description, element and allocation budgets apply through all nested readers. Count arithmetic is checked, including count+1 and 2*entry_count+1.

The profile assigns no universal memory budget. Tests pass explicit limits and expect `limit_exceeded` separately from malformed input. A decompression wrapper must bound expanded bytes before this reader sees them. Failed acquisition, description lookup, validation or resource checks never return a partial model or an unchecked borrowed view.

## Columns remain separate

This profile specifies row storage and direct field/element projection. It does not specify a column codec. The separate [crumb-columns/1 draft](columns-v1.md) specifies exact-width frame-of-reference packing, deltas, runs, column-wide prefix-sharing dictionaries, encoding IDs and positional indexes. Its performance experiments are evidence for choosing that profile, not conformance vectors for this one.
