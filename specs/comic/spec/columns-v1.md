# Crumb column profile 1

Status: experimental codec `crumb-columns/1`, numeric codec ID 2, with value profile `crumb1`, numeric value-profile ID 1. This profile stores ordered records in field columns with bounded blocks and explicit encodings. It shares logical values, descriptions, hashes, evolution and seals with [crumb-row/1](row-v1.md); it does not claim measured performance parity with another format or database.

## Shared envelope and descriptions

Use row/1's `CRMB | L(codec_id) | L(value_profile_id) | flags | L(payload_bytes) | payload` envelope, shortest unsigned-u64 L, framed B, flags and exact-consumption rules, with codec ID 2. The description dictionary is byte-for-byte the row/1 dictionary: sorted unique closed selectors K, one revision per K, inline or caller-supplied complete-description reference. All nested dependencies are admitted before exposure; no implicit fetch occurs. Numeric codec ID 1 retains its existing interpretation.

The payload is:

```text
L(description_count) | descriptions...
L(record_count) | L(group_count)
row_prefix_table | group_offset_table | groups...
[ L(record_count) | 32-byte seal ]
```

Every table is `width | fixed_width_little_endian_entries`, with width exactly one octet, 1, 2, 4 or 8, the smallest able to hold the final entry, including zero. Counts determine table lengths; no count is repeated inside a table. Unused padding does not exist. An offset table begins with zero, is nondecreasing, and ends at the exact framed data length. All count/width arithmetic is checked before access or allocation.

There are group_count+1 entries in each table. Row prefixes start at zero, strictly increase for nonempty groups and end at record_count. Group offsets are relative to the start of the groups area. Its length is the last group offset; any seal follows that area, outside the last group. Each group has positive byte length. Zero records requires zero groups and two `01 00` tables; nonzero records requires at least one group.

A group's row count is the difference of its row-prefix entries. Its bytes are:

```text
L(description_index) | column_offset_table | columns...
```

A record description has one column per field in ascending field-identity order. An enum description has one column whose cell type is that complete nominal enum. The column table has column_count+1 entries, offsets relative to the start of column data. Its final offset must equal the remaining group frame. A zero-field record has no columns and table `01 00`; its row count still counts values and charges caller element budgets.

Groups preserve original record order. Each is a maximal consecutive run of records with the same dictionary index; adjacent groups cannot name the same index. Nonadjacent runs of the same type are allowed. Reordering groups to improve compression would change the input and seal. An empty group is never an alternate representation of zero values. A RecordSet is one envelope; a RecordStream may carry consecutive envelopes with independent dictionaries and completion. Splitting one set into multiple envelopes is an explicit caller operation, not byte-preserving re-encoding.

## Column blocks and projection

A group of n rows has b = ceil(n/256) blocks in every column. All but the last have exactly 256 **logical row positions**; the last has n modulo 256 positions, or 256 when divisible. The number 256 is a codec rule, not a universal memory budget. A future block-size interpretation requires another codec ID. Compute b without unchecked n+255 arithmetic.

A column is:

```text
dictionary_mode | [ B(column_dictionary) ] | block_offset_table | blocks...
```

dictionary_mode is one octet: 00 for no dictionary, 01 for a shared column dictionary; other modes are unsupported. A dictionary is allowed for string, binary or the integer/decimal stored cell types admitted by encoding 1 and must be used by at least one block encoded as 4 or 5. It contains exactly the distinct stored values across the complete column within this group, including values in blocks that choose another encoding. It has no format-imposed entry or byte cap; caller resource limits still apply. Blocks never start a fresh dictionary.

The table has b+1 entries, with offsets relative to the block-data start and its final offset equal to that column's remaining length. Blocks have positive byte lengths. Locating row r within its group selects block floor(r/256) and position r modulo 256. These addresses and the selected field column are computed from fixed-width tables; no preceding block or column must be decoded. Group lookup uses the admitted row-prefix table, for example a binary search. Initial directory and description validation has a cost; it is not claimed to be free.

A field block is:

```text
[presence_bitmap] | L(encoding_id) | encoding_payload
```

A field with absent=none or absent=empty has ceil(block_logical_rows/8) presence octets, low bit first, unused high bits zero. Required fields and the root-enum column have no bitmap. Bit zero means the declared absent value and consumes no encoded value. Bit one consumes one value, in row order. Thus a block has m stored values, its bitmap popcount or all its logical rows. Presence rank is bounded to 256 bits; an optional precomputed rank table may reduce its cost further.

An absent-policy option removes only its outer option wrapper from the stored cell type. A present Some(None) still stores the inner absence. A present empty collection under absent=empty is noncanonical. Required options/empty collections keep their row/1 cell encodings. A block with m=0 must use encoding 0 with its empty indexed payload `01 00`; no empty dictionary, run or numeric base is permitted instead.

The first column profile flattens **root record fields**, not arbitrary nested paths. A nested record, list, map, set, option or union remains its typed row/1 cell under encoding 0 or 3. This already permits projecting an outer field without reconstructing other fields. It does not promise scalar-column access inside a nested collection. Root enums likewise retain their complete typed enum cells. Recursive shredding and alternative null/definition-level schemes are separate future profile work, not implicit extensions of these bytes. Size results from experiments that shred nested paths do not transfer to this root-field-only profile.

## Encoding IDs and exact packed bits

| ID | Encoding | Admitted stored cell types |
| --- | --- | --- |
| 0 | Indexed typed cells | Every row/1 cell type |
| 1 | Frame-of-reference packed integers | i8/i16/i32/i64/i128, u8/u16/u32/u64/u128, decimal<P,S> coefficients |
| 2 | Zigzag-delta packed integers | Same as 1 |
| 3 | Runs of typed cells | Every row/1 cell type |
| 4 | Shared dictionary and packed codes | string, binary, or the integer/decimal types above |
| 5 | Shared dictionary and run-coded codes | Same as 4 |

Unknown IDs are unsupported. A known ID used for an incompatible type is malformed. The writer may choose any admitted encoding; it need not trial every candidate or select the smallest result. Each chosen encoding has exact canonical bytes. Column/1 bytes are not canonical across writers: two valid encoding choices for the same values can differ. Byte equality is not value equality. Original-shape re-encoding retains encoding choices, description modes and envelope boundaries. There is no claim that all physical encodings of one logical set are identical; crumb1 hashes compare logical values instead.

`PACK(w, values)` concatenates exactly w bits of each unsigned value, least-significant bit first, filling each octet from low bit to high bit. Its length is ceil(w*count/8); unused high bits of the final octet must be zero. Every value must fit in w bits. Width zero is valid only for all-zero values and has no data octets. Widths are L values, not floating-point logarithms; derive a minimal width by exact integer bit length. Apply shared limits before multiplying counts, reserving storage or shifting. A valid large exact number may exceed a host's chosen numeric/resource support; never silently truncate it.

### 0: indexed typed cells

Payload is an offset table with m+1 entries, followed by the m concatenated row/1 typed cells. The final offset equals cell-data length. Equal offsets are valid only when that exact typed cell legitimately has zero bytes. This also represents m=0 as `01 00`. Validate each reachable typed cell and exact consumption. Projection takes the selected two offsets without decoding earlier cells or constructing models.

### 1: frame of reference

For m>0:

```text
B(base_cell) | L(w) | PACK(w, residuals[0..m])
```

The base is the minimum stored value, encoded as its ordinary row/1 scalar cell. For decimals it is the coefficient at the declared scale, not a rounded decimal string or binary float. Each residual is value-base in exact mathematical integers. At least one residual is zero; otherwise the base is not minimal. Width is exactly bit_length(max residual), with bit_length(0)=0. Reconstructed values must fit the declared integer width or decimal precision. No wrapping arithmetic is allowed, including u64 extremes or signed subtraction across zero. A selected value requires the base and its directly addressed bits, not earlier residuals.

### 2: zigzag deltas

For m>0:

```text
B(first_cell) | L(w) | PACK(w, deltas[1..m])
```

The first value uses its ordinary row/1 scalar cell. Delta i is Z(value_i-value_(i-1)), using the same zigzag mapping as hashes.md but packing its unsigned result rather than U-encoding it. Width is exactly bit_length(max encoded delta), or zero when m=1 or all deltas are zero. Signed mathematical subtraction is required even for unsigned values; a difference between two u64 values can need 65 zigzag bits. Every reconstructed value must fit the declared type/precision. Reconstructing position j replays at most 255 deltas within its block; blocks restart independently. This is bounded positional access, not constant-time delta lookup.

### 3: runs

For m>0:

```text
L(run_count) | (L(run_length) | B(typed_cell))...
```

Lengths are positive, their checked sum equals m and run_count is between 1 and m. Each cell is one complete canonical row/1 typed cell. Adjacent runs must not contain identical typed-cell bytes; combine them into one run. Since the selected type and its canonical row-cell encoding are fixed, this comparison requires no application equality callback. Run lookup scans at most m <= 256 run entries in the selected block. Framing may be validated without creating the represented model. Empty or excess runs and surplus cell bytes are malformed.

### Shared column dictionary for 4 and 5

Let M be the number of present stored values in the entire column and d their distinct count. The column-level dictionary, outside all 256-row blocks, is shared by every code block in that column. The dictionary contains exactly those used keys, unique and sorted. String/binary keys are sorted lexicographically by unsigned raw octets; strings are their exact UTF-8 bytes, without normalization. Integer keys are sorted by mathematical value; decimal keys by coefficient at their single declared scale. Floating-point dictionaries are not admitted by this profile.

```text
L(d) | B(key_dictionary)
```

Require 1 <= d <= M. There is no 256-entry or 1 MB format limit. Repeated strings in distant blocks share the same code. Dictionary bytes and counts have their enclosing B boundary and caller-selected limits. The dictionary restarts every 16 keys; q=ceil(d/16). Both key forms below use this directory. Its bytes are:

```text
restart_offset_table | key_entries...
```

The table has q+1 entries, offsets relative to key_entries. The final offset is exactly the entries length. For string/binary keys, every entry is `L(prefix_length) | B(suffix_bytes)`. Entry positions 0,16,32,... have prefix_length=0 and carry the complete key. Other entries use exactly the longest common byte prefix with the preceding reconstructed key, followed by the remaining suffix. A byte prefix may end inside a multibyte UTF-8 character; validate the complete reconstructed key, not the isolated suffix, as UTF-8. Prefix length cannot exceed the predecessor length. Strict key ordering rejects duplicate keys regardless of their physical framing.

For integer/decimal keys, restart positions 0,16,32,... carry `B(typed_cell)`, a complete canonical row/1 value of the stored type. Every other position carries `B(positive_difference)`: the mathematical difference from its predecessor as unsigned little-endian octets, shortest representation, with at least one octet and a nonzero final octet. Zero, empty and padded differences are malformed. A difference is not a signed stored-type value or an L count: the full signed-i64 span needs the unsigned value 2^64-1, and a decimal difference may exceed u64. Reconstruct in exact arithmetic, then check each key against the declared stored type and precision. Restart keys are absolute and must also exceed the preceding key; restarting does not relax global order. Sparse, widely separated integers can therefore use short codes without requiring large frame-of-reference residuals. The stored type determines the key form; no second dictionary-kind tag is needed.

Restart offsets must point precisely at the designated key entries, not merely somewhere inside the entries area. A reader resolves dictionary code c by restart floor(c/16), then reconstructs at most 16 entries. It need not decode all earlier dictionary keys. Validation still checks the whole dictionary, exact offsets and that its keys equal the distinct present stored values of the entire column. A block need not use every key. Empty string/binary is a valid key; M=0 cannot carry a dictionary. Numeric lookup likewise reads one restart value and at most 15 positive differences, never every earlier key.

Encoding 4's complete block payload is:

```text
L(w) | PACK(w, codes[0..m])
```

w is exactly bit_length(d-1). For d=1, w=0 and every code is implicitly zero. Every code must be less than d. This block uses the enclosing column dictionary; it carries no repeated dictionary header. Index lookup is direct packed-bit access plus the bounded dictionary restart.

Encoding 5's complete block payload is:

```text
L(run_count) | (L(run_length) | L(code))...
```

Run lengths/counts obey encoding 3's positive/sum/minimal-run rules; adjacent codes differ. Codes must be less than d. Lookup scans at most 256 runs plus at most 16 dictionary entries. Codes use the complete column dictionary and may exceed 255; their L encoding and the packed width in encoding 4 follow d, not block cardinality.

## Validation, hashes and safe exposure

Validate the common envelope, complete dictionary and all group/column/block directories before accessing their ranges. Group row counts and block counts must agree in every column; one missing column or block is not an absent value. No frame can point into a sibling's metadata or past its own bound. Validate canonical numeric/prefix/run forms, presence rules and the row/1 cells they contain. An invalid unprojected cell cannot be smuggled into an admitted complete set by asking for another field.

Shared byte, allocation, description, element, digit, depth and work budgets apply throughout. Charge logical records even for zero-field types, reconstructed dictionary bytes rather than only compressed suffixes or differences, and expanded runs/collections. Check counts and multiplication before allocating offset tables or decoded output. Caller limits may refuse an otherwise valid set as limit_exceeded; malformed data stays distinguishable. A general compressor is an explicitly bounded outer layer, not another unannounced encoding ID.

The seal is unchanged: trailing L(record_count) and SHA-256 of ordered **logical object contributions**, not group bytes, dictionary codes, packed residuals or selected codec IDs. A reader reconstructs the logical order across columns for seal verification without requiring model instances. A sealed set exposes no records, field views or trusted projected batch until the entire set and seal are verified. It may retain backing or spool bounded input for this first pass. An unsealed stream reports a later failure and coverage explicitly, never ordinary completion. Integrity is not authorship or provider authorization.

A retained view obeys its binding's backing/lease contract. Packed, delta and dictionary values may require scalar decoding or temporary output; "columnar" is not a promise every projected value can borrow a native array directly. An independently retained mapping keeps its backing alive; a copied materialization has its own storage. Store snapshots and transactions remain outside this codec.

## Evidence and required tests

Compression experiments motivate per-column alternatives and bounded restart points. They do not select normative bytes, prove a decoder, establish full conformance or justify a universal performance claim. This draft's literal vectors use small independently specified values, byte boundaries and refusals. A production implementation must run these files directly plus row/1's shared descriptor/cell cases.

Required controls include width-zero and byte-crossing packed values, signed/unsigned extremes, negative deltas, overflow and nonminimal widths, truncated dictionaries, longest-prefix and restart violations, signed-extreme numeric differences, nonpositive/padded deltas and reconstructed numeric overflow, unused column keys and out-of-range dictionary codes, invalid UTF-8 reconstruction, run overflow, optional absence versus Some(None), rows crossing 256, dictionaries crossing 16, mixed-type groups, exact original-choice re-encoding and logical seal agreement with row/1. Large/nested/resource-accounting and retained-view proofs remain required beyond finite byte vectors.
