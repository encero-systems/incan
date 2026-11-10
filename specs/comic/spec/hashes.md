# Identity, equality and hashes

Status: experimental value rules with an exact draft preimage grammar and independently checkable vectors below. These are format contracts, not production codec conformance.

## Identity is not a hash of contents

A member identity is the first six bytes of SHA-256 over the UTF-8 bytes of its earliest name, with no terminator. A rename retains that earliest name. Identities must be unique within one written record or enum description. Sorting is lexicographic unsigned byte order. Display spelling, import aliases and declaration order do not select a field.

The member vectors in the corpus include a known six-byte collision. Width reduces accidental risk; it does not replace collision refusal when a description contains equal identities.

Nominal type identity belongs to the binding's identity scheme, which must identify its authority/package scope and earliest declaration name without depending on local import aliases. The neutral core carries that identity but assigns no privileged registry, package manager or language namespace. Matching structural shapes alone does not authorize reading one nominal type as another. An identifier or digest supplied by an untrusted producer is not authentication.

A closed generic type is selected by its declaration identity followed by its concrete type arguments in parameter order. Each argument is itself a closed selector or a built-in structural spelling. Thus Box[i8] and Box[string] share a declaration identity but have different type selectors, and Box[Box[i8]] nests selectors. Parameter names do not participate; changing argument order does. Dictionary keys, nominal union selection and ordering, shape references and the nominal part of complete-description digests all use this closed selector. Nongeneric declarations have an explicit zero-argument count in the framed selector below.

## Value profile crumb1

- Integers compare and contribute to hashes by mathematical value. A representation-preserving integer widening changes no value hash.
- Decimals compare and contribute by mathematical value too, whatever scale their type declares: `19.90` in a `decimal<10, 2>` field and `19.9` in a `decimal<10, 1>` field contribute alike. Text still writes a decimal at its type's scale, as `19.90d`.
- Binary32 contributes as the binary64 value it exactly equals.
- Positive and negative zero remain distinct. NaN signs and payloads normalize to one positive quiet NaN with no payload beyond its quiet bit; encodings restore that value.
- Strings have no Unicode normalization. Bytes compare bytewise.
- Optional wrapping preserves a non-absent value's contribution. Outer absence and present inner absence remain distinct at every nesting depth.
- Unions contribute their selected member's canonical type spelling and its value. Reordering alternatives changes neither.
- Maps and sets use profile-defined ordering, never user comparison code. The order is false before true, integer numeric order, string UTF-8/byte lexicographic order, absence before present values, sequence/tuple lexicographic order, enum variant identity then payload, union closed selector (or non-nominal hashed spelling) then value, and record field identity order. A constrained newtype contributes its underlying value.
- A field at its declared absent value contributes nothing. This is outer absence for optionals or an empty collection whose description explicitly admits empty absence. A required field or an arbitrary nonempty constructor default cannot be omitted.
- A typed reference contributes the identity it names, never the target's current contents.
- A secret-typed field contributes only its withheld state, never its contents, so every value of it contributes the same. The model projection records the field and its secret type.

These rules do not redefine a programming language's equality operator. Stored comparisons must not execute it. The exact ordering below completes these rules for admitted kinds; host-language ordering is not a fallback.

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

## Digest preimages: crumb1 draft

This section specifies the first experimental digest grammar. It is independent of every binary codec and of COMIC whitespace. The accompanying literal preimages are reviewable vectors, not output captured from a production encoder. The profile must be versioned if an adopted interpretation changes.

### Framing and domains

All octets below are hexadecimal. `U(n)` is the shortest unsigned base-128 little-endian integer: seven payload bits per octet, low group first, with bit 7 set except in the last octet. Zero is `00`. Overlong encodings are refused. `Z(n) = U(2*n)` for nonnegative n and `U(-2*n-1)` otherwise. Arithmetic is exact and does not truncate to the host's integer size. `F(bytes) = U(byte_length) || bytes`; text in F is exact UTF-8 unless explicitly called an ASCII type spelling.

Every digest is SHA-256 of one complete preimage:

| Purpose | Preimage |
| --- | --- |
| Object | UTF-8 `crumb1`, then `00 01`, then `V_object(value)` |
| Canonical | UTF-8 `crumb1`, then `00 02`, then `V_canonical(value)` |
| Description | UTF-8 `crumb1`, then `00 05`, then `D(description)` |
| Model | UTF-8 `crumb1`, then `00 03`, then `F(S(type))` |
| Sealed set | UTF-8 `crumb1`, then `00 04`, then `U(count)`, then `F(V_object(value))` for every value in order |

The domain octet is included even when two projections happen to select identical fields. Equality comparisons use the same domain and profile. Member identities keep their existing unprefixed SHA-256 rule. Neither host-memory bytes nor encoded Crumb bytes are substituted for V or S.

A sealed set commits to order and multiplicity of value contributions, including noncanonical fields. It does not commit to a particular compression, integer width, nominal binding or byte layout; those are admitted separately. An empty sealed set has count zero. Its seal is not the digest of an empty record. A seal is not authentication or an exact-byte checksum.

### Value contributions

The following productions are prefix-decodable. Counts are numbers of elements, not byte lengths. Nested V uses the same projection as its parent. There are no padding octets or terminators beyond these productions.

| Kind | V |
| --- | --- |
| Absent optional | `00 || U(depth)`: None has depth 0; Some(None) has depth 1, and so on |
| Boolean | `01 || 00` for false; `01 || 01` for true |
| Integer | `02 || Z(value)`, independent of signedness and width |
| Float | `03 || eight big-endian IEEE binary64 octets`, with binary32 widened exactly and all NaNs normalized to `7ff8000000000000` |
| Decimal | `04 || Z(coefficient) || Z(scale)` after the normalization below |
| String | `05 || F(UTF-8)` |
| Binary | `06 || F(bytes)` |
| Withheld secret | `07`, with no payload |
| List | `08 || U(count) || V(item)...` |
| Tuple | `09 || U(count) || V(item)...` |
| Set | `0a || U(count) || V(item)...`, in the ordering below |
| Map | `0b || U(count) || (V(key) || V(value))...`, in key order |
| Record | `0c || U(present_contributing_fields) || (six-byte field identity || V(field))...`, in identity order |
| Enum | `0d || six-byte variant identity || V(payload_record)`; an empty payload is an empty record |
| Union | `0e || F(N(selected_member_type)) || V(member)` |
| Typed reference | `0f || F(target_type_identity) || F(target_identity)`; both identities are opaque binding-supplied bytes |

Present optional wrappers disappear only when their eventual contained value is not absence. Thus Some(Some(7)) contributes the integer 7, but Some(Some(None)) contributes `00 02`. An optional element inside a container is retained. At a record field, omission is decided first using that field's declared absent policy; only then is V emitted. A required optional field holding None contributes `00 00`.

A decimal means coefficient multiplied by 10 to the power negative scale. Remove trailing decimal zeros from a nonzero coefficient, decrementing scale for every removed zero. Normalize every decimal zero to coefficient 0, scale 0. For example 19.90 and 19.9 contribute coefficient 199, scale 1; 100 contributes coefficient 1, scale -2. This signed normalized scale is a hash representation, not a change to the declared decimal type or its text. Precision and declared scale remain in S. Decimal and integer contributions have different tags even when their numerical values coincide.

For a record, omit fields at their admitted absent value in both projections. In the canonical projection also omit `canonical=false` fields. Apply that projection recursively, including inside collections and enum payloads. An empty record, empty list, empty tuple, empty set, empty map, empty string and empty binary each have a different tag. A field's identity frames its contribution independently of its name, declaration position or encoded storage offset.

A reference's target-type identity is the binding's stable type identity, not its current display path. For nominal targets under this identity scheme it is K, including all closed arguments. Test notation represents these opaque bytes as UTF-8 strings unless an explicit hex payload is given. Changing a target's contents never changes V; changing either identity does. Aliases and newtypes contribute their underlying value. Withheld string and byte secrets share the marker contribution; their model types remain distinct. An Incan binding that forbids walking secret types still refuses before encoding; this format rule grants no permission to bypass it.

### Canonical type spelling S

S is ASCII with no whitespace. It represents shape, not a source-language display spelling. Scalar tokens are `boolean`, `i8`, `i16`, `i32`, `i64`, `i128`, `u8`, `u16`, `u32`, `u64`, `u128`, `fp32`, `fp64`, `string`, `binary`, `secret_string` and `secret_bytes`. `decimal<P,S>` uses canonical decimal integers, with positive precision and `0 <= S <= P` in this profile. No leading zeros or plus signs occur. A wider primitive profile must register unsupported scalar widths explicitly rather than silently substituting a machine type.

| Shape | S |
| --- | --- |
| List, set, optional | `list<T>`, `set<T>`, `option<T>` with T replaced by S(item); `T?` normalizes to `option<T>` |
| Map | `map<K,V>` |
| Tuple | `tuple<T1,T2,...>`; empty tuple is `tuple<>` |
| Record | `record{ID:T:C:A;ID:T:C:A;...}` with ID as 12 lowercase hexadecimal digits, T as S(field type), C `1` or `0` for canonical participation, and A `r`, `n` or `e` for required, outer-None or empty-collection absence |
| Enum | `enum{ID:record{...};ID:record{...};...}` with sorted variant IDs and payload records |
| Union | `union<T1,T2,...>` sorted by SHA-256 of each S(member), with the full spelling as a collision tie-breaker |
| Recursive edge | `^D`, where D is the zero-based distance to the referenced active record/enum ancestor |
| Typed reference | `reference`; nominal target constraints remain in the separately admitted binding description |

Fields and variants sort by their six-byte identities. Empty records and enums have empty braces. Optional nesting is not collapsed in S. Alias and newtype shapes fold as their underlying type; concrete generic instantiations substitute their actual types. Nominal names, display paths, documentation and nonempty construction defaults do not enter S. The admitted absent policy does enter S; optional type alone does not imply that the field may be omitted. Their exclusion is not permission to discard them from a complete description.

Track the active stack of record and enum definitions while folding. Container constructors do not push this stack. The nearest active definition is `^0`; its parent definition is `^1`. On first entering a definition, expand it; on an edge back to an active definition, emit the ancestor reference. A repeated nonrecursive definition expands again, so incidental dictionary sharing does not change a hash. An enum payload record is an anonymous structural record, not an additional nominal ancestor. Finite recursive shapes are supported; infinite value containment is refused.

For a union member's S, use the same enclosing active stack for every alternative, then sort the resulting spellings. Preserve the number of distinct admitted alternatives even if two nominal alternatives have identical structural spellings. Type admission, including nominal distinctions, precedes hashing; an identical structural model hash cannot replace that admission.

### Union selector spelling N

A value must distinguish two unrelated nominal union members even when they have the same shape. N therefore uses `record@HEX` or `enum@HEX` for a nominal member, where HEX is its six-byte declaration identity, written as 12 lowercase hexadecimal digits. For this identity scheme, hash the registry id, scoped package name, and earliest module path plus declaration name as UTF-8 strings, each followed by one zero byte, then take the first six SHA-256 bytes. None of the three strings may contain a zero byte. Imports and display aliases never enter this input; rename history supplies the earliest owner and path. A closed nominal with arguments uses `record@HEX<N(arg0),N(arg1),...>` or the enum equivalent. With zero arguments, omit the angle brackets; empty `<>` is noncanonical. It uses the scalar and container syntax of S otherwise, recursively replacing nominal occurrences with their closed selectors. Aliases and constrained newtypes fold as their underlying type as required by the value profile.

S is the structural model projection; N is the identity-bearing union discriminator, corresponding to stored type identity rather than a display path. Two distinct legal union alternatives whose N is identical after alias/newtype folding are not distinguishable under this value profile and must be refused at profile admission, not selected by declaration order. The caller may use an explicitly discriminated enum instead. This refusal does not affect a language's ability to declare such a union or use another profile.

### Closed nominal selector bytes K

`K = declaration_id[6] || U(argument_count) || F(N(arg0)) || ... || F(N(arg(argument_count-1)))`. N arguments are ASCII, closed, canonical spellings in parameter order. The six-byte declaration identity still uses the three zero-terminated strings above; do not hash arguments into it or truncate them. K itself is the framed selector, not another hash. Complete descriptions commit to these exact bytes through D; a selected union commits to N through its existing V domain. No new digest domain is introduced.

A nongeneric declaration has argument_count zero, so K ends in `00`. Explicitly supplying the empty argument list yields the same K; omitting that count is malformed. Nonempty arguments, their count, lengths and nesting cannot be confused with that zero-argument form. Aliases and constrained newtypes resolve to their underlying selector before K is made. Unresolved parameters and infinite argument expansion are not closed selectors and are refused. Recursive record fields refer to their finite closed nominal selector; they do not expand the record's fields into its arguments.

A dictionary sorts by K bytes and admits one complete-description revision per K. It may contain Box[i8] and Box[string] together. A union sorts nominal members by those same K bytes. These orderings run no user code. Bindings retain and validate declaration identity, kind, parameter arity and concrete argument constraints; untrusted selector bytes are no authority to invent an instantiation or resolve a nominal collision.

### Stored ordering

Ordering runs no user code and is not lexicographic comparison of V's variable-length integers. Within an admitted type, Booleans order false then true; integers order by exact mathematical value; strings order by their UTF-8 octets; binary orders by octets; decimals order by exact mathematical value without conversion to float. Floats order negative infinity, finite values in numerical order (negative zero before positive zero), positive infinity, then normalized NaN. All NaNs compare equal here.

Absence sorts before present values. Among nested absent states, smaller depth sorts first. Non-absent optional wrappers add no ordering distinction. Sequences and tuples compare elements lexicographically with the shorter equal prefix first. Records compare ordered sequences of contributing `(identity, value)` pairs under the selected projection; enums compare variant identity then payload. References compare target-type identity then target identity bytewise. Withheld values compare equal. Sets compare their sorted elements and maps their sorted key/value pairs.

Nominal union members compare their framed K selector bytes, then their contained value; the record/enum kind breaks a same-selector cross-kind tie. Non-nominal members compare SHA-256 of their ASCII N spelling, then the full spelling as a collision tie-breaker. In a mixed union, non-nominal selectors precede nominal selectors. Duplicate closed selectors for distinct members are refused at admission, not treated as the same member. Different argument lists of one declaration are distinct members; a declaration-identity collision between different declarations is still refused. Structural S alternatives continue to sort by their hashed structural spelling for model folding. Values of unrelated unwrapped kinds are not admitted as keys of one typed map/set; a union provides the explicit distinction. Duplicate map keys and set elements under stored comparison are refused before hashing. Duplicate detection and ordering use complete values after hash lookup, never digest equality alone.

Map/set duplicate admission uses complete object values. For a canonical projection, sort the projected set elements or projected map keys; equal projected map keys are ordered by their projected values. Keep multiplicity when distinct admitted object keys or elements project alike. Equal projected entries have identical contributions, so no further tie-break changes the bytes. Do not reject newly equal projected keys or merge them: this is a digest projection, not a replacement map. Thus changing a noncanonical key field cannot change canonical bytes merely by changing object-key ordering.

### Limits and unsupported types

The grammar specifies exact values, not unlimited resources. Callers bound preimage bytes, nesting, digit counts and temporary ordering memory; implementations may stream counts and contributions. Shared nested budgets are not reset. A limit refusal is distinct from invalid syntax and must not produce a prefix digest as a completed hash.

Date/time and interval value representations, constrained string/binary types and language-specific extensions need an admitted mapping to these contributions before this profile hashes them. A source-language type spelling alone is not such a mapping. Unknown mappings are refused; no fallback hashes Debug text. These extensions do not prevent hashing the explicitly covered core types.

### Independent vectors

The [digest corpus](../corpus/hash-preimages.json) supplies logical inputs, literal preimage hex and SHA-256 digests. Each derivation decomposes the literal bytes into annotated segments; those segments must concatenate exactly. A conformance implementation derives bytes from the logical input and checks both preimage and digest. Hashing the supplied hex alone proves only the digest arithmetic.

Vectors cover domains, empty kinds, scalar widths, signed zero, NaNs, exact integers, normalized decimals, optional depth, field omission, all projections, recursion, union selection, references and ordered seals. The member-identity corpus continues to check the independent earliest-name rule. The binary codec must run the same vectors without changing them.

## Complete-description preimage D

The description domain is distinct from the model domain. D encodes the complete admitted description as a metadata tree: null, Boolean, exact integer, string, binary, ordered list or string-keyed map. It carries the nominal identity, declared kind and path, every member's identity/name/type/absence/canonical setting, declaration order, rename facts and properties. Do not drop unknown properties from the digest; a reader either understands their profile or refuses admission. Property values with richer types must carry their type and exact data in an admitted metadata structure. D never evaluates a property, loads a type or executes validation.

| Metadata value | D |
| --- | --- |
| null | `00` |
| Boolean | `01 00` or `01 01` |
| Integer | `02 || Z(value)` |
| String | `03 || F(UTF-8)` |
| Binary | `04 || F(bytes)` |
| List | `05 || U(count) || D(item)...` |
| Map | `06 || U(count) || (F(UTF-8 key) || D(value))...`, keys sorted by exact UTF-8 bytes |

Map keys must be unique. Metadata lists preserve order. Strings are not normalized. A complete record description contains `nominal` (the binary closed selector K), `kind`, `path`, `fields` in declaration order and `properties`. Each field carries `id` (six binary bytes), `name`, `type` (N of the storage type), `absent` (`required`, `none` or `empty`), `canonical` and `properties`. An enum uses `variants`, each with `id`, `name`, payload `fields` and `properties`. Additional profile facts are retained in properties, including any richer declared-type constraints. A profile must not erase a constraint merely because storage uses its underlying type.

This metadata tree is a canonical description contract, not a runtime model instance or JSON wire format. It makes the full-description digest independent of a row codec's offsets, compression and dictionary numbering. An inline codec may deliberately carry D directly; another codec may encode the same description differently and still compute D for its digest. A description reference binds the complete K selector and this digest, never just its structural model hash.

The description vectors include a display rename, changed nominal owner and changed absent policy. Each changes D even when some leave the model projection unchanged. All use the same domain-prefix rule as the other hashes. The three-string nominal identity inputs have their own independent corpus operation; they do not use any crumb1 domain prefix.
