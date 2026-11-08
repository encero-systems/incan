# COMIC and Crumb

Specified by Encero Systems.

COMIC means **Canonical Object Markup for Interoperable Crumbs**. COMIC is readable text; Crumb is compact binary storage of the same typed values. Neither is a database, a programming language or an instruction to execute code.

This folder is the experimental specification and conformance corpus. It is self-contained so it can move to a separate format repository without moving an implementation. No language implementation is normative. Language bindings describe how their own types map into this contract.

## Status

This first foundation fixes the existing value, identity and compatibility requirements as test data. It does **not** freeze a complete text grammar, binary layout or hash preimage encoding. The distinction is deliberate: a value rule can be settled before its byte spelling. The open encoding items are enumerated in [text](spec/text.md), [binary](spec/binary.md) and [hashes](spec/hashes.md). The corpus must not be advertised as full codec conformance until those items have exact vectors and an implementation runs them.

The initial corpus includes independently checkable member-identity vectors, typed-value comparisons, shape admission and evolution cases. Its JSON is test notation, not COMIC syntax or a proposed Crumb encoding. There are no fake binary goldens or object-hash digests inferred from illustrative text.

## Read in this order

1. [Typed values and shapes](spec/values.md): what a record contains.
2. [Identity and hashes](spec/hashes.md): what changes identity or equality.
3. [Compatibility](spec/evolution.md): what a reader may adapt.
4. [COMIC text](spec/text.md) and [Crumb bytes](spec/binary.md): representation boundaries and remaining choices.
5. [Conformance](spec/conformance.md): the executable meaning of the [corpus manifest](corpus/manifest.json).

[Profile registrations](profiles.json) identify the contracts named by tests. Unknown profiles are refused. Registrations are documentation, never permission to fetch code, contact a registry, open a database or execute a constructor. An application supplies its admitted descriptions and runtime implementations separately.

## Example

A delivery record holds a customer name, a list of parcels and an optional delivery note. The list keeps its order. A missing note may read as absent if its description allows that. A missing customer name is an error. A field rename preserves identity through its earliest name; changing its display name must not move parcel data into another field.

A program may inspect that record as COMIC, encode it as a standalone Crumb, or put it in a store. Store revisions, transactions, indexes, retention and network access belong to that store. A Crumb can exist without any store.

## License

This folder is distributed under the [Apache License 2.0](LICENSE).
