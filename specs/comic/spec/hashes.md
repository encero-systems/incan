# Identity, equality and hashes

Status: value rules and member identity vectors are fixed for this foundation. The complete digest preimage grammar remains to be specified before object, canonical or model hash goldens can be claimed.

## Identity is not a hash of contents

A member identity is the first six bytes of SHA-256 over the UTF-8 bytes of its earliest name, with no terminator. A rename retains that earliest name. Identities must be unique within one written record or enum description. Sorting is lexicographic unsigned byte order. Display spelling, import aliases and declaration order do not select a field.

The member vectors in the corpus include a known six-byte collision. Width reduces accidental risk; it does not replace collision refusal when a description contains equal identities.

Nominal type identity belongs to the binding's identity scheme, which must identify its authority/package scope and earliest declaration name without depending on local import aliases. The neutral core carries that identity but assigns no privileged registry, package manager or language namespace. Matching structural shapes alone does not authorize reading one nominal type as another. An identifier or digest supplied by an untrusted producer is not authentication.

## Value profile crumb1

- Integers compare and contribute to hashes by mathematical value. A representation-preserving integer widening changes no value hash.
- Decimals compare and contribute by mathematical value too, whatever scale their type declares: `19.90` in a `decimal<10, 2>` field and `19.9` in a `decimal<10, 1>` field contribute alike. Text still writes a decimal at its type's scale, as `19.90d`.
- Binary32 contributes as the binary64 value it exactly equals.
- Positive and negative zero remain distinct. NaN signs and payloads normalize to one positive quiet NaN with no payload beyond its quiet bit; encodings restore that value.
- Strings have no Unicode normalization. Bytes compare bytewise.
- Optional wrapping preserves a non-absent value's contribution. Outer absence and present inner absence remain distinct at every nesting depth.
- Unions contribute their selected member's canonical type spelling and its value. Reordering alternatives changes neither.
- Maps and sets use profile-defined ordering, never user comparison code. The order is false before true, integer numeric order, string UTF-8/byte lexicographic order, absence before present values, sequence/tuple lexicographic order, enum variant identity then payload, union hashed member spelling then value, and record field identity order. A constrained newtype contributes its underlying value.
- A field at its declared absent value contributes nothing. This is outer absence for optionals or an empty collection whose description explicitly admits empty absence. A required field or an arbitrary nonempty constructor default cannot be omitted.
- A typed reference contributes the identity it names, never the target's current contents.
- A secret-typed field contributes only its withheld state, never its contents, so every value of it contributes the same. The model projection records the field and its secret type.

These rules do not redefine a programming language's equality operator. Stored comparisons must not execute it. Complete ordering of other kinds, including floats and cross-kind keys, must be specified before those key profiles are admitted; host-language ordering is not a fallback.

## Three projections

| Projection | Contents | Example consequence |
| --- | --- | --- |
| Object | All stored field contributions | Changing a received-at field changes this projection. |
| Canonical | Fields with canonical participation, recursively | A received-at field marked noncanonical does not affect this projection. |
| Model | Field identities, declared types, absent-value policies and canonical flags, recursively | A new optional field changes this projection even when its value is absent. |

Object and canonical projections do not include the model hash. Model projection excludes methods, documentation, declaration order, display paths and names. A recursive model already being folded contributes a back-reference by ancestor depth. Aliases fold as their target, newtypes as the underlying type, generics as concrete instantiations, and union members sort by their hashed spelling. Nominal identity is checked separately from model shape.

A renamed or reordered field changes none of these projections. An added absent optional changes the model projection but not the value projections. A changed absent-value policy changes the model projection, because it decides which values contribute nothing: an empty `tags` list contributes nothing when its description declares `tags: list<string> = []`, and contributes an empty list when it declares `tags: list<string>`. Removing a nonempty field produces an adapted value with different contributions; it cannot claim the original value's hashes. Hash equality only locates candidates where collisions matter; it does not prove equality or trust.

## Description digest

A description digest covers one complete description: its nominal identity and everything it declares about each member, including names, absent-value policies and properties. It names exactly one description, which is what a crumb's shape reference binds (see [binary](binary.md)). A model hash cannot do that, because it leaves out nominal identity and names: `crm.Customer` and `billing.Customer` with the same fields share a model hash, and so do two descriptions that differ only in a field's display name.

## Before digest vectors are frozen

Specify domain separation, exact scalar tags and lengths, canonical type spellings, numeric encodings, recursive references, reference identity encoding, decimal treatment, absent contribution framing, projection boundaries and the description digest's preimage. Specify how to distinguish an empty sequence, an empty record and empty bytes. Publish independently reviewed preimage bytes and SHA-256 results. A codec byte layout must not secretly become the hash preimage. Profile names accompany hashes, for example crumb1:sha256:…; profiles with different meanings are not comparable.
