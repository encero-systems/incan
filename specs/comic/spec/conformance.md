# Foundation corpus

The [manifest](../corpus/manifest.json) lists every case file, its SHA-256 and its test operation. JSON is a language-neutral test notation here, not a candidate wire encoding. Exact integers and bit patterns are strings so a JSON parser cannot round them. Files are UTF-8. Case IDs are unique across the manifest.

## Operations

- **member_identity**: each case supplies earliest_name and expected six-byte lowercase hex. SHA-256 the exact UTF-8 name, take its first six bytes, and compare. No prefix, suffix or terminator is added. The known collision is intentional; an admission case separately refuses two colliding members in one description.
- **value_relation**: compare the two fully tagged values under crumb1; expect same or distinct value contribution, not a literal digest. Normalize NaN and representation-preserving width/optional wrappers as specified. Strings preserve code points. References compare their target identities without loading targets. No user equality code may run.
- **shape_admission**: validate a written description's ordered fields; expect accepted or the specified failure class and field path. IDs are explicit bytes; display names cannot repair duplicates.
- **adaptation**: read values under the written description into the requested description; expect typed field values or a failure. Source fields are in written identity order. Requested declaration order may differ. Absent values may be supplied only by the explicit policy. The original stored value is unchanged.

## Test notation

A scalar type is a string: bool, i32, i64, u32, u64, f32, f64, string or bytes. A compound type is an object with kind and item (option/list) or items (tuple). A description has nominal and fields. Each field has id, name, type, canonical and absent; absent is required, none or empty. Names are diagnostic labels. A schema with absent=none must have an option type; absent=empty requires a collection. Arrays of fields in written descriptions must be strictly ID-sorted. Requested descriptions express declaration order, which the adapter must not confuse with written slot order.

A tagged value has kind and its payload: int uses decimal value and type; float uses width and fixed-width hexadecimal bits; string uses value; bytes uses lowercase hex; none has no payload; some has value; list/tuple have items; reference has target_type and identity. References may carry observed_target for a test control: it is not part of the reference value and is never resolved. Representation-preserving wrappers can compare equal as contributions even when the input types differ. These tests do not grant structural compatibility between unrelated nominal models.

Adaptation inputs give values as an array aligned with written fields. Successful outputs give values aligned with requested fields. expected contains status and, on refusal, kind and path. path uses a field's identity, or /nominal for the record identity. A removed field is not returned; that does not mutate the source. comparison expectations describe the logical hash projections, not a promise that an unimplemented hash encoder has a particular digest.

## What is and is not proved

This foundation checks member identities, optional and float distinctions, Unicode preservation, references, descriptor ordering/collisions and common evolution decisions. It does not yet test a COMIC parser, a Crumb binary codec, record streaming, memory bounds or full recursive type support. Those remain required by [text](text.md), [binary](binary.md) and [values](values.md). No implementation is declared conformant by committing these files.

Implementations must run the data files directly. They may not copy a selection of cases into source and call that full conformance. Add an implementation receipt naming corpus revision, profile, selected cases, outcomes and known unsupported cases. A host may have stricter caller-selected resource limits; report limit refusals separately from malformed data.

## Independent checks

The email and customer identity vectors agree with the existing derivation examples; all vectors can be independently computed with any SHA-256 implementation. The pair fieldbbea15ef9eff and field3d4624c2d3a2 intentionally has equal six-byte prefixes. The signed-zero and NaN cases use explicit IEEE binary32/binary64 bits; the Unicode cases use distinct scalar sequences. These controls keep host-language equality, JSON numeric rounding and pretty text from becoming an accidental oracle.

## Self-description follow-up

When the text grammar is defined, profiles, the corpus manifest and case data also receive COMIC representations. Keep their independent JSON twins and test that each COMIC document reconstructs exactly its twin's typed values. Do not generate both expected and actual values solely through the same parser. Prose remains Markdown. Descriptions of the format's own profile, field and case models can follow once those models and their text grammar are precise; this foundation adds no guessed COMIC files.
